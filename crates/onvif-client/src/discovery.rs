//! ONVIF camera discovery — WS-Discovery + static camera list.
//!
//! Mirrors the TypeScript OnvifDiscovery class with two modes:
//! - **auto**: WS-Discovery multicast + static camera list, periodic rescanning
//! - **static**: One-time connection to pre-configured camera list only

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use crate::client::connect_camera;
use crate::types::CameraDevice;

/// Number of consecutive missed scans before a camera is considered lost.
const MISS_THRESHOLD: u32 = 3;

/// WS-Discovery probe timeout.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Events emitted by the discovery loop.
#[derive(Debug, Clone)]
pub enum DiscoveryEvent {
    CameraFound(CameraDevice),
    CameraLost(String),
    CameraUnreachable(String),
    Error(String),
}

/// Discovery configuration.
#[derive(Debug, Clone)]
pub struct DiscoveryConfig {
    pub username: String,
    pub password: String,
    pub scan_interval: Duration,
    pub mode: DiscoveryMode,
    pub static_cameras: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DiscoveryMode {
    Auto,
    Static,
}

/// A host:port pair for an ONVIF device.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct DeviceAddress {
    hostname: String,
    port: u16,
}

/// Run the ONVIF discovery loop, sending events to the provided channel.
///
/// This function runs forever (in auto mode) or once (in static mode).
pub async fn run_discovery(config: DiscoveryConfig, tx: mpsc::Sender<DiscoveryEvent>) {
    match config.mode {
        DiscoveryMode::Static => {
            info!("Starting ONVIF in static mode (no WS-Discovery, no periodic rescanning)");
            scan_static_only(&config, &tx).await;
        }
        DiscoveryMode::Auto => {
            info!(
                interval_ms = config.scan_interval.as_millis(),
                "Starting ONVIF discovery in auto mode"
            );
            run_auto_loop(config, tx).await;
        }
    }
}

/// Static mode: connect once to the static camera list.
async fn scan_static_only(config: &DiscoveryConfig, tx: &mpsc::Sender<DiscoveryEvent>) {
    let devices = parse_static_cameras(&config.static_cameras);
    if devices.is_empty() {
        warn!("Static mode enabled but no static cameras configured");
        return;
    }

    info!(
        total = devices.len(),
        "Connecting to static cameras"
    );

    let mut connected = 0;
    for device in &devices {
        match connect_camera(
            &device.hostname,
            device.port,
            &config.username,
            &config.password,
        )
        .await
        {
            Ok(camera) => {
                connected += 1;
                let _ = tx.send(DiscoveryEvent::CameraFound(camera)).await;
            }
            Err(e) => {
                error!(
                    host = device.hostname,
                    port = device.port,
                    err = %e,
                    "Failed to connect to static camera"
                );
            }
        }
    }

    info!(connected, total = devices.len(), "Static camera scan complete");
}

/// Auto mode: WS-Discovery + static cameras, periodic rescanning.
async fn run_auto_loop(config: DiscoveryConfig, tx: mpsc::Sender<DiscoveryEvent>) {
    let mut discovered: HashMap<String, CameraDevice> = HashMap::new();
    let mut missed_scans: HashMap<String, u32> = HashMap::new();

    loop {
        if let Err(e) = scan_once(&config, &tx, &mut discovered, &mut missed_scans).await {
            error!(err = %e, "ONVIF scan failed");
            let _ = tx.send(DiscoveryEvent::Error(e)).await;
        }

        tokio::time::sleep(config.scan_interval).await;
    }
}

/// Perform a single discovery scan.
async fn scan_once(
    config: &DiscoveryConfig,
    tx: &mpsc::Sender<DiscoveryEvent>,
    discovered: &mut HashMap<String, CameraDevice>,
    missed_scans: &mut HashMap<String, u32>,
) -> Result<(), String> {
    // Probe via WS-Discovery
    let probed = probe().await;
    let static_devices = parse_static_cameras(&config.static_cameras);
    let devices = merge_devices(&probed, &static_devices);

    info!(
        probed = probed.len(),
        static_count = static_devices.len(),
        total = devices.len(),
        "ONVIF devices to connect"
    );

    let mut found_ids = HashSet::new();

    // Connect sequentially to avoid overwhelming cameras
    for device in &devices {
        match connect_camera(
            &device.hostname,
            device.port,
            &config.username,
            &config.password,
        )
        .await
        {
            Ok(camera) => {
                found_ids.insert(camera.id.clone());
                missed_scans.remove(&camera.id);

                if !discovered.contains_key(&camera.id) {
                    let _ = tx.send(DiscoveryEvent::CameraFound(camera.clone())).await;
                }
                discovered.insert(camera.id.clone(), camera);
            }
            Err(e) => {
                if e.contains("refused") {
                    debug!(
                        host = device.hostname,
                        port = device.port,
                        "Device refused connection"
                    );
                } else if e.contains("Fault") || e.contains("Authorized") {
                    warn!(
                        host = device.hostname,
                        port = device.port,
                        "ONVIF auth failed — check credentials"
                    );
                } else {
                    debug!(
                        host = device.hostname,
                        port = device.port,
                        err = %e,
                        "Failed to connect to ONVIF device"
                    );
                }
            }
        }
    }

    // Detect cameras that disappeared — use grace period before removal
    let ids_to_check: Vec<String> = discovered.keys().cloned().collect();
    for id in ids_to_check {
        if !found_ids.contains(&id) {
            let misses = missed_scans.entry(id.clone()).or_insert(0);
            *misses += 1;

            if *misses >= MISS_THRESHOLD {
                discovered.remove(&id);
                missed_scans.remove(&id);
                let _ = tx.send(DiscoveryEvent::CameraLost(id.clone())).await;
                info!(camera_id = id, "Camera removed after {} missed scans", MISS_THRESHOLD);
            } else {
                let _ = tx.send(DiscoveryEvent::CameraUnreachable(id.clone())).await;
                debug!(
                    camera_id = id,
                    missed = *misses,
                    threshold = MISS_THRESHOLD,
                    "Camera missed scan"
                );
            }
        }
    }

    info!(
        connected = found_ids.len(),
        total = discovered.len(),
        "ONVIF scan complete"
    );

    Ok(())
}

/// Run WS-Discovery probe and extract host:port from XAddr URLs.
async fn probe() -> Vec<DeviceAddress> {
    match oxvif::discovery::probe(PROBE_TIMEOUT).await {
        devices if !devices.is_empty() => {
            debug!(count = devices.len(), "WS-Discovery probe complete");
            devices
                .iter()
                .filter_map(|d| {
                    d.xaddrs.first().and_then(|xaddr| {
                        url::Url::parse(xaddr).ok().map(|u| DeviceAddress {
                            hostname: u.host_str().unwrap_or("").to_string(),
                            port: u.port().unwrap_or(80),
                        })
                    })
                })
                .collect()
        }
        _ => {
            debug!("WS-Discovery probe returned no results");
            Vec::new()
        }
    }
}

/// Parse static cameras from "host:port,host:port,..." strings.
fn parse_static_cameras(entries: &[String]) -> Vec<DeviceAddress> {
    entries
        .iter()
        .filter(|s| !s.is_empty())
        .map(|entry| {
            let parts: Vec<&str> = entry.splitn(2, ':').collect();
            DeviceAddress {
                hostname: parts[0].to_string(),
                port: parts.get(1).and_then(|p| p.parse().ok()).unwrap_or(80),
            }
        })
        .collect()
}

/// Merge probed and static devices, deduplicating by hostname.
/// Static cameras take priority (user-provided ports are more reliable).
fn merge_devices(probed: &[DeviceAddress], static_devices: &[DeviceAddress]) -> Vec<DeviceAddress> {
    let mut seen = HashMap::new();

    // Static cameras first (higher priority)
    for device in static_devices {
        seen.entry(device.hostname.clone())
            .or_insert_with(|| device.clone());
    }
    for device in probed {
        seen.entry(device.hostname.clone())
            .or_insert_with(|| device.clone());
    }

    seen.into_values().collect()
}
