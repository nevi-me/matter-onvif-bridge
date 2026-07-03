//! Bridges ONVIF discovery + go2rtc media to the Matter camera endpoints.
//!
//! Runs on a separate tokio runtime thread that:
//! 1. Starts go2rtc manager (waits for readiness)
//! 2. Runs ONVIF WS-Discovery loop
//! 3. Feeds discovered cameras into the `CameraRegistry`
//! 4. Registers RTSP streams in go2rtc via `StreamManager`
//! 5. Pre-seeds each camera's AV stream into its slot's
//!    `CameraAvStreamHandler::add_preallocated_video`
//! 6. Populates the bridge-side `CameraSlot` (BDBI labels, motion flag)
//! 7. Spawns a motion pump per camera advertising MotionAlarm events

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use async_channel::Sender;
use matter_camera::{MotionState, OccupancyDataver};
use media::go2rtc_api::Go2RtcApi;
use media::go2rtc_manager::{Go2RtcManager, Go2RtcMode};
use onvif_client::discovery::{DiscoveryConfig, DiscoveryEvent, DiscoveryMode};
use onvif_client::motion::{spawn_motion_pump, MotionPumpConfig};
use onvif_client::registry::{CameraRegistry, RegistryEvent};
use onvif_client::types::CameraDevice;
use rs_matter::dm::clusters::app::cam_av_stream::{StreamUsageEnum, VideoCodecEnum, VideoStream};
use tokio::sync::mpsc;

use crate::config::{self, Config};
use crate::slot_persistence::SlotMap;
use crate::{MAX_CAMERAS, WITH_OCCUPANCY_CAMERAS};

/// Cross-thread message: "seed slot N's CameraAvStreamHandler with this
/// pre-allocated VideoStream." Sent from the ONVIF bridge thread, consumed
/// by the seeding task running on the rs-matter executor (main thread). We
/// can't send the handler refs themselves because they use `NoopRawMutex`
/// and are `!Sync`.
pub type SeedTx = Sender<(usize, VideoStream)>;

/// Slim per-slot state held by the bridge. Cluster state (video/audio
/// streams, WebRTC sessions, etc.) is owned by the upstream rs-matter
/// handlers — this struct only carries what `BridgedDeviceBasicInformation`
/// needs to answer reads about the bridged camera.
#[derive(Debug, Clone, Default)]
pub struct CameraSlot {
    pub occupied: bool,
    pub node_label: String,
    pub vendor_name: String,
    pub product_name: String,
    pub serial_number: String,
    pub hardware_version_string: String,
    pub software_version_string: String,
    pub unique_id: String,
    pub supports_ptz: bool,
}

/// Shared state accessible from the Matter handler thread (today: the
/// `BridgeWebRtcHooks` look up `stream_names` to find the right go2rtc
/// stream for an incoming SDP offer).
#[derive(Clone)]
pub struct MediaBridge {
    /// camera id → slot index
    pub slot_map: Arc<RwLock<HashMap<String, usize>>>,
    /// slot index → go2rtc stream name
    pub stream_names: Arc<RwLock<HashMap<usize, String>>>,
}

/// Spawn the ONVIF + go2rtc bridge on a dedicated tokio thread.
///
/// `go2rtc_api` and `stream_names` are owned by the caller so the WebRTC
/// hooks can share the same `Arc`s (they look up the go2rtc stream name
/// for an incoming SDP offer in the shared map).
pub fn start_onvif_bridge(
    cfg: &Config,
    slots: &[Arc<RwLock<CameraSlot>>],
    motion_states: &[MotionState],
    occupancy_datavers: &[OccupancyDataver],
    seed_tx: SeedTx,
    registry: CameraRegistry,
    go2rtc_api: Go2RtcApi,
    stream_names: Arc<RwLock<HashMap<usize, String>>>,
) -> MediaBridge {
    let media_bridge = MediaBridge {
        slot_map: Arc::new(RwLock::new(HashMap::new())),
        stream_names,
    };

    let discovery_config = DiscoveryConfig {
        username: cfg.onvif.username.clone(),
        password: cfg.onvif.password.clone(),
        scan_interval: Duration::from_millis(cfg.onvif.discovery_interval_ms),
        mode: match cfg.onvif.discovery_mode {
            config::DiscoveryMode::Static => DiscoveryMode::Static,
            config::DiscoveryMode::Auto => DiscoveryMode::Auto,
        },
        static_cameras: cfg.onvif.static_cameras.clone(),
    };

    let go2rtc_mode = match cfg.go2rtc.mode {
        config::Go2RtcMode::External => Go2RtcMode::External,
        config::Go2RtcMode::Local => Go2RtcMode::Local,
    };

    let go2rtc_manager = Go2RtcManager::new(
        &cfg.go2rtc.host,
        cfg.go2rtc.api_port,
        cfg.go2rtc.webrtc_port,
        go2rtc_mode,
        &cfg.go2rtc.path,
    );

    let slots_owned = slots.to_vec();
    let motion_states_owned = motion_states.to_vec();
    let occupancy_datavers_owned = occupancy_datavers.to_vec();
    let registry_clone = registry.clone();
    let bridge_clone = media_bridge.clone();
    let onvif_username = cfg.onvif.username.clone();
    let onvif_password = cfg.onvif.password.clone();
    let camera_names = cfg.onvif.camera_names.clone();
    let storage_dir = std::path::PathBuf::from(&cfg.matter.storage_path);

    std::thread::Builder::new()
        .name("onvif-media-bridge".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("Failed to create tokio runtime");

            rt.block_on(async move {
                log::info!("ONVIF/media bridge thread started");

                if let Err(e) = go2rtc_manager.start().await {
                    log::error!("Failed to start go2rtc: {e}");
                }
                log::info!("go2rtc started, launching stream manager and ONVIF discovery");

                let api_for_streams = go2rtc_api.clone();
                let registry_for_streams = registry_clone.clone();
                let stream_user = onvif_username.clone();
                let stream_pass = onvif_password.clone();
                tokio::spawn(async move {
                    media::stream_manager::run_stream_manager(
                        &registry_for_streams,
                        api_for_streams,
                        &stream_user,
                        &stream_pass,
                    )
                    .await;
                });

                let (discovery_tx, mut discovery_rx) = mpsc::channel(64);
                tokio::spawn(onvif_client::discovery::run_discovery(
                    discovery_config,
                    discovery_tx,
                ));

                let mut registry_rx = registry_clone.subscribe();
                let mut slot_map = SlotMap::load(
                    &storage_dir,
                    MAX_CAMERAS,
                    WITH_OCCUPANCY_CAMERAS,
                );
                let mut motion_tasks: HashMap<usize, tokio::task::JoinHandle<()>> = HashMap::new();
                // The upstream `add_preallocated_video` has no remove counterpart,
                // so we only seed a slot once. Subsequent reconnections of the
                // same camera reuse the existing pre-allocated stream.
                let mut seeded: HashSet<usize> = HashSet::new();

                loop {
                    tokio::select! {
                        Some(event) = discovery_rx.recv() => {
                            match event {
                                DiscoveryEvent::CameraFound(camera) => {
                                    registry_clone.add_camera(camera);
                                }
                                DiscoveryEvent::CameraLost(id) => {
                                    registry_clone.remove_camera(&id);
                                }
                                DiscoveryEvent::CameraUnreachable(_) | DiscoveryEvent::Error(_) => {}
                            }
                        }
                        Ok(event) = registry_rx.recv() => {
                            match event {
                                RegistryEvent::Added(camera) => {
                                    let slot = slot_map.assign(&camera.id, camera.supports_motion);
                                    if let Some(slot) = slot {
                                        let stream_name = sanitize_stream_name(&camera.id);
                                        if let Ok(mut map) = bridge_clone.slot_map.write() {
                                            map.insert(camera.id.clone(), slot);
                                        }
                                        if let Ok(mut map) = bridge_clone.stream_names.write() {
                                            map.insert(slot, stream_name);
                                        }

                                        let friendly_name = camera_names
                                            .get(&camera.device_info.serial_number)
                                            .or_else(|| camera_names.get(&camera.id))
                                            .or_else(|| camera_names.get(&camera.host))
                                            .cloned();
                                        populate_camera_slot(
                                            &slots_owned[slot],
                                            &camera,
                                            friendly_name.as_deref(),
                                        );

                                        if seeded.insert(slot) {
                                            seed_av_streams(slot, &camera, &seed_tx);
                                        }

                                        log::info!(
                                            "Camera '{}' ({}) → endpoint {} (motion={}, ptz={}), stream registered",
                                            friendly_name.as_deref().unwrap_or(&camera.device_info.model),
                                            camera.id,
                                            slot + 2,
                                            camera.supports_motion,
                                            camera.supports_ptz,
                                        );

                                        if camera.supports_motion && slot < WITH_OCCUPANCY_CAMERAS {
                                            if let Some(events_url) = camera.events_url.clone() {
                                                let pump_cfg = MotionPumpConfig {
                                                    host: camera.host.clone(),
                                                    port: camera.port,
                                                    username: onvif_username.clone(),
                                                    password: onvif_password.clone(),
                                                    events_url,
                                                    label: friendly_name.clone().unwrap_or_else(|| {
                                                        format!(
                                                            "{} {} @ {}",
                                                            camera.device_info.manufacturer,
                                                            camera.device_info.model,
                                                            camera.host
                                                        )
                                                    }),
                                                };
                                                let motion_state = motion_states_owned[slot].clone();
                                                let dataver_for_pump = occupancy_datavers_owned[slot].clone();
                                                let handle = spawn_motion_pump(
                                                    pump_cfg,
                                                    move |motion| {
                                                        if motion_state.set(motion) {
                                                            dataver_for_pump.bump();
                                                        }
                                                    },
                                                );
                                                motion_tasks.insert(slot, handle);
                                            }
                                        }
                                    } else {
                                        log::warn!(
                                            "No endpoint slots left for camera {} (max {})",
                                            camera.id,
                                            MAX_CAMERAS
                                        );
                                    }
                                }
                                RegistryEvent::Updated(camera) => {
                                    if let Ok(map) = bridge_clone.slot_map.read() {
                                        if let Some(&slot) = map.get(&camera.id) {
                                            let friendly_name = camera_names
                                                .get(&camera.device_info.serial_number)
                                                .or_else(|| camera_names.get(&camera.id))
                                                .or_else(|| camera_names.get(&camera.host))
                                                .map(String::as_str);
                                            populate_camera_slot(
                                                &slots_owned[slot],
                                                &camera,
                                                friendly_name,
                                            );
                                        }
                                    }
                                }
                                RegistryEvent::Removed(id) => {
                                    if let Ok(mut map) = bridge_clone.slot_map.write() {
                                        if let Some(slot) = map.remove(&id) {
                                            if let Ok(mut s) = slots_owned[slot].write() {
                                                *s = CameraSlot::default();
                                            }
                                            if let Ok(mut names) = bridge_clone.stream_names.write() {
                                                names.remove(&slot);
                                            }
                                            if let Some(handle) = motion_tasks.remove(&slot) {
                                                handle.abort();
                                            }
                                            // NB: pre-allocated AV streams are not removed —
                                            // the upstream cluster has no remove API. If the
                                            // camera reconnects we'll reuse the existing
                                            // stream; if it stays gone forever the cluster
                                            // attribute will reference a stale stream until
                                            // the bridge restarts.
                                            log::info!(
                                                "Camera {} removed from endpoint {}",
                                                id,
                                                slot + 2
                                            );
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            });
        })
        .expect("Failed to spawn ONVIF/media bridge thread");

    media_bridge
}

fn populate_camera_slot(
    slot_lock: &Arc<RwLock<CameraSlot>>,
    camera: &CameraDevice,
    friendly_name: Option<&str>,
) {
    let Ok(mut slot) = slot_lock.write() else {
        log::error!("Failed to lock camera slot for writing");
        return;
    };

    slot.occupied = true;
    slot.vendor_name = camera.device_info.manufacturer.clone();
    slot.product_name = camera.device_info.model.clone();
    slot.serial_number = camera.device_info.serial_number.clone();
    slot.hardware_version_string = camera.device_info.hardware_id.clone();
    slot.software_version_string = camera.device_info.firmware_version.clone();
    slot.unique_id = camera.id.clone();
    slot.node_label = friendly_name.map(str::to_string).unwrap_or_else(|| {
        format!(
            "{} {}",
            camera.device_info.manufacturer, camera.device_info.model
        )
    });
    slot.supports_ptz = camera.supports_ptz;
}

fn seed_av_streams(slot: usize, camera: &CameraDevice, seed_tx: &SeedTx) {
    let Some(profile) = camera.profiles.first() else {
        log::warn!(
            "Camera {} has no media profiles — skipping AV stream seed",
            camera.id
        );
        return;
    };
    let Some(ve) = profile.video_encoder.as_ref() else {
        log::warn!(
            "Camera {} profile {} has no video encoder — skipping AV stream seed",
            camera.id,
            profile.token
        );
        return;
    };

    let codec = match ve.codec.to_ascii_uppercase().as_str() {
        "H265" | "HEVC" => VideoCodecEnum::HEVC,
        "VVC" => VideoCodecEnum::VVC,
        "AV1" => VideoCodecEnum::AV1,
        _ => VideoCodecEnum::H264,
    };

    let stream = VideoStream {
        video_stream_id: 0, // overwritten by handler
        stream_usage: StreamUsageEnum::LiveView,
        video_codec: codec,
        min_frame_rate: 1,
        max_frame_rate: ve.frame_rate.max(1),
        min_width: 320,
        min_height: 240,
        max_width: ve.width,
        max_height: ve.height,
        min_bit_rate: 200_000,
        max_bit_rate: (ve.bitrate as u32).saturating_mul(1000).max(500_000),
        key_frame_interval: 2000,
        watermark_enabled: None,
        osd_enabled: None,
        reference_count: 0,
    };

    if let Err(e) = seed_tx.try_send((slot, stream)) {
        log::warn!(
            "Failed to enqueue AV stream seed for camera {} slot {}: {}",
            camera.id,
            slot,
            e
        );
    } else {
        log::info!(
            "Queued AV stream seed for camera {} slot {} ({}x{}@{}, codec={:?})",
            camera.id,
            slot,
            ve.width,
            ve.height,
            ve.frame_rate,
            codec
        );
    }
}

fn sanitize_stream_name(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}
