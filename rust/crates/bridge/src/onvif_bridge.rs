//! Bridges ONVIF discovery to the Matter camera endpoint states.
//!
//! Runs the ONVIF discovery loop on a tokio runtime (separate thread)
//! and populates the pre-allocated camera endpoint states with real data.

use std::sync::{Arc, RwLock};
use std::time::Duration;

use matter_camera::types::{
    CameraEndpointState, VideoResolution, VideoSensorParams,
};
use onvif_client::discovery::{DiscoveryConfig, DiscoveryEvent, DiscoveryMode};
use onvif_client::registry::{CameraRegistry, RegistryEvent};
use onvif_client::types::CameraDevice;
use tokio::sync::mpsc;

use crate::config::{self, Config};

const MAX_CAMERAS: usize = 16;

/// Start the ONVIF discovery bridge on a separate tokio runtime thread.
///
/// This spawns a background thread running tokio that:
/// 1. Runs ONVIF WS-Discovery (or static-only) loop
/// 2. Feeds discovered cameras into the CameraRegistry
/// 3. Populates pre-allocated camera endpoint states with real ONVIF data
pub fn start_onvif_bridge(
    cfg: &Config,
    camera_states: &[Arc<RwLock<CameraEndpointState>>],
    registry: CameraRegistry,
) {
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

    let states = camera_states.to_vec();
    let registry_clone = registry.clone();

    std::thread::Builder::new()
        .name("onvif-bridge".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("Failed to create tokio runtime for ONVIF");

            rt.block_on(async move {
                let (discovery_tx, mut discovery_rx) = mpsc::channel(64);

                // Spawn discovery loop
                tokio::spawn(onvif_client::discovery::run_discovery(
                    discovery_config,
                    discovery_tx,
                ));

                // Subscribe to registry events for state population
                let mut registry_rx = registry_clone.subscribe();

                // Process discovery events → registry, and registry events → state population
                let mut next_slot: usize = 0;
                let mut slot_map: std::collections::HashMap<String, usize> =
                    std::collections::HashMap::new();

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
                                DiscoveryEvent::CameraUnreachable(id) => {
                                    log::debug!("Camera {} unreachable", id);
                                }
                                DiscoveryEvent::Error(e) => {
                                    log::error!("Discovery error: {}", e);
                                }
                            }
                        }
                        Ok(event) = registry_rx.recv() => {
                            match event {
                                RegistryEvent::Added(camera) => {
                                    if next_slot < MAX_CAMERAS {
                                        let slot = next_slot;
                                        slot_map.insert(camera.id.clone(), slot);
                                        next_slot += 1;
                                        populate_camera_state(&states[slot], &camera);
                                        log::info!(
                                            "Camera '{}' ({}) assigned to endpoint {}",
                                            camera.device_info.model,
                                            camera.id,
                                            slot + 2, // ep 2..17
                                        );
                                    } else {
                                        log::warn!(
                                            "No more endpoint slots for camera {} (max {})",
                                            camera.id,
                                            MAX_CAMERAS
                                        );
                                    }
                                }
                                RegistryEvent::Updated(camera) => {
                                    if let Some(&slot) = slot_map.get(&camera.id) {
                                        populate_camera_state(&states[slot], &camera);
                                    }
                                }
                                RegistryEvent::Removed(id) => {
                                    if let Some(&slot) = slot_map.get(&id) {
                                        // Reset endpoint state to defaults
                                        if let Ok(mut state) = states[slot].write() {
                                            *state = CameraEndpointState::default();
                                        }
                                        log::info!(
                                            "Camera {} removed from endpoint {}",
                                            id,
                                            slot + 2,
                                        );
                                    }
                                    slot_map.remove(&id);
                                }
                            }
                        }
                    }
                }
            });
        })
        .expect("Failed to spawn ONVIF bridge thread");
}

/// Populate a camera endpoint's cluster state from ONVIF device data.
fn populate_camera_state(
    state_lock: &Arc<RwLock<CameraEndpointState>>,
    camera: &CameraDevice,
) {
    let Ok(mut state) = state_lock.write() else {
        log::error!("Failed to lock camera state for writing");
        return;
    };

    // Extract video params from the first profile
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
    state.max_network_bandwidth = 10_000; // Default 10 Mbps
}
