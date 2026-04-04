//! Bridges ONVIF discovery and go2rtc media to the Matter camera endpoint states.
//!
//! Runs on a separate tokio runtime thread that:
//! 1. Starts go2rtc manager (wait for readiness)
//! 2. Runs ONVIF WS-Discovery loop
//! 3. Feeds discovered cameras into the CameraRegistry
//! 4. Registers RTSP streams in go2rtc via StreamManager
//! 5. Populates pre-allocated camera endpoint states with real ONVIF data
//! 6. Stores the go2rtc API and slot map for WebRTC command handling

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use matter_camera::types::{CameraEndpointState, VideoResolution, VideoSensorParams};
use media::go2rtc_api::Go2RtcApi;
use media::go2rtc_manager::{Go2RtcManager, Go2RtcMode};
use onvif_client::discovery::{DiscoveryConfig, DiscoveryEvent, DiscoveryMode};
use onvif_client::registry::{CameraRegistry, RegistryEvent};
use onvif_client::types::CameraDevice;
use tokio::sync::mpsc;

use crate::config::{self, Config};

const MAX_CAMERAS: usize = 16;

/// Shared state accessible from the Matter handler thread for WebRTC negotiation.
#[derive(Clone)]
pub struct MediaBridge {
    /// go2rtc API client for SDP exchange.
    pub api: Go2RtcApi,
    /// Maps camera ID → endpoint slot index (0-based, endpoint = slot + 2).
    pub slot_map: Arc<RwLock<HashMap<String, usize>>>,
    /// Maps endpoint slot index → stream name for go2rtc.
    pub stream_names: Arc<RwLock<HashMap<usize, String>>>,
}

/// Start the ONVIF + go2rtc bridge on a separate tokio runtime thread.
///
/// Returns a `MediaBridge` that can be used by the Matter WebRTC handler
/// to perform SDP negotiation via go2rtc.
pub fn start_onvif_bridge(
    cfg: &Config,
    camera_states: &[Arc<RwLock<CameraEndpointState>>],
    registry: CameraRegistry,
) -> MediaBridge {
    let go2rtc_api = Go2RtcApi::new(&cfg.go2rtc.host, cfg.go2rtc.api_port);

    let media_bridge = MediaBridge {
        api: go2rtc_api.clone(),
        slot_map: Arc::new(RwLock::new(HashMap::new())),
        stream_names: Arc::new(RwLock::new(HashMap::new())),
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

    let states = camera_states.to_vec();
    let registry_clone = registry.clone();
    let bridge_clone = media_bridge.clone();
    let onvif_username = cfg.onvif.username.clone();
    let onvif_password = cfg.onvif.password.clone();

    std::thread::Builder::new()
        .name("onvif-media-bridge".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("Failed to create tokio runtime");

            rt.block_on(async move {
                log::info!("ONVIF/media bridge thread started");

                // 1. Start go2rtc
                if let Err(e) = go2rtc_manager.start().await {
                    log::error!("Failed to start go2rtc: {e}");
                    // Continue anyway — go2rtc may come up later
                }

                log::info!("go2rtc started, launching stream manager and ONVIF discovery");

                // 2. Spawn stream manager (registers RTSP streams in go2rtc)
                let api_for_streams = go2rtc_api.clone();
                let registry_for_streams = registry_clone.clone();
                tokio::spawn(async move {
                    media::stream_manager::run_stream_manager(
                        &registry_for_streams,
                        api_for_streams,
                        &onvif_username,
                        &onvif_password,
                    )
                    .await;
                });

                // 3. Spawn ONVIF discovery
                let (discovery_tx, mut discovery_rx) = mpsc::channel(64);
                tokio::spawn(onvif_client::discovery::run_discovery(
                    discovery_config,
                    discovery_tx,
                ));

                // 4. Process discovery events → registry → state population
                let mut registry_rx = registry_clone.subscribe();
                let mut next_slot: usize = 0;

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
                                    if next_slot < MAX_CAMERAS {
                                        let slot = next_slot;
                                        next_slot += 1;

                                        // Update slot maps
                                        let stream_name = sanitize_stream_name(&camera.id);
                                        if let Ok(mut map) = bridge_clone.slot_map.write() {
                                            map.insert(camera.id.clone(), slot);
                                        }
                                        if let Ok(mut map) = bridge_clone.stream_names.write() {
                                            map.insert(slot, stream_name);
                                        }

                                        populate_camera_state(&states[slot], &camera);
                                        log::info!(
                                            "Camera '{}' ({}) → endpoint {}, stream registered",
                                            camera.device_info.model,
                                            camera.id,
                                            slot + 2,
                                        );
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
                                            populate_camera_state(&states[slot], &camera);
                                        }
                                    }
                                }
                                RegistryEvent::Removed(id) => {
                                    if let Ok(mut map) = bridge_clone.slot_map.write() {
                                        if let Some(slot) = map.remove(&id) {
                                            if let Ok(mut state) = states[slot].write() {
                                                *state = CameraEndpointState::default();
                                            }
                                            if let Ok(mut names) = bridge_clone.stream_names.write() {
                                                names.remove(&slot);
                                            }
                                            log::info!("Camera {} removed from endpoint {}", id, slot + 2);
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

/// Populate a camera endpoint's cluster state from ONVIF device data.
fn populate_camera_state(state_lock: &Arc<RwLock<CameraEndpointState>>, camera: &CameraDevice) {
    let Ok(mut state) = state_lock.write() else {
        log::error!("Failed to lock camera state for writing");
        return;
    };

    // Mark slot as occupied
    state.occupied = true;

    // BDBI fields from ONVIF device info
    state.vendor_name = camera.device_info.manufacturer.clone();
    state.product_name = camera.device_info.model.clone();
    state.serial_number = camera.device_info.serial_number.clone();
    state.hardware_version_string = camera.device_info.hardware_id.clone();
    state.software_version_string = camera.device_info.firmware_version.clone();
    state.unique_id = camera.id.clone();
    state.node_label = format!(
        "{} {}",
        camera.device_info.manufacturer, camera.device_info.model
    );

    // Video params from first profile
    if let Some(profile) = camera.profiles.first() {
        if let Some(ve) = &profile.video_encoder {
            state.video_sensor_params = VideoSensorParams {
                sensor_width: ve.width,
                sensor_height: ve.height,
                hdr_capable: false,
                max_fps: ve.frame_rate,
            };
            state.viewport = VideoResolution {
                width: ve.width,
                height: ve.height,
            };
            state.current_frame_rate = ve.frame_rate;
            state.max_encoded_pixel_rate =
                ve.width as u32 * ve.height as u32 * ve.frame_rate as u32;
        }
    }

    state.max_concurrent_video_encoders = camera.profiles.len().max(1) as u8;
    state.max_network_bandwidth = 10_000;
}

/// Sanitize camera ID into a go2rtc-compatible stream name.
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
