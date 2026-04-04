//! Smoke test: discover and connect to ONVIF cameras using config from .env
//!
//! Reads ONVIF_USERNAME, ONVIF_PASSWORD, and ONVIF_STATIC_CAMERAS from the
//! workspace root .env file. Run from the workspace root:
//!
//!   cargo run -p onvif-client --example test_connect

use std::time::Duration;

#[tokio::main]
async fn main() {
    // Load .env from workspace root (rust/../.env)
    // CARGO_MANIFEST_DIR = rust/crates/onvif-client, .env is at repo root
    let env_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../.env");
    match dotenvy::from_path(&env_path) {
        Ok(_) => println!("Loaded {}", env_path.display()),
        Err(e) => println!("Warning: could not load {}: {e}", env_path.display()),
    }

    let username = std::env::var("ONVIF_USERNAME").unwrap_or_else(|_| "admin".into());
    let password = std::env::var("ONVIF_PASSWORD").unwrap_or_else(|_| "admin".into());
    let static_cameras = std::env::var("ONVIF_STATIC_CAMERAS").unwrap_or_default();

    // ── WS-Discovery ──
    println!("=== WS-Discovery probe (5s timeout) ===");
    let discovered = oxvif::discovery::probe(Duration::from_secs(5)).await;
    println!("Found {} devices:", discovered.len());
    for d in &discovered {
        let xaddr = d.xaddrs.first().map(String::as_str).unwrap_or("?");
        println!("  {}", xaddr);
    }
    println!();

    // ── Build camera list: merge static + discovered ──
    let mut cameras: Vec<(String, u16)> = Vec::new();
    let mut seen = std::collections::HashSet::new();

    // Static cameras first (higher priority)
    for entry in static_cameras.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let parts: Vec<&str> = entry.splitn(2, ':').collect();
        let host = parts[0].to_string();
        let port: u16 = parts.get(1).and_then(|p| p.parse().ok()).unwrap_or(80);
        if seen.insert(host.clone()) {
            cameras.push((host, port));
        }
    }

    // Add discovered cameras not already in static list
    for d in &discovered {
        if let Some(xaddr) = d.xaddrs.first() {
            if let Ok(url) = url::Url::parse(xaddr) {
                let host = url.host_str().unwrap_or("").to_string();
                let port = url.port().unwrap_or(80);
                if seen.insert(host.clone()) {
                    cameras.push((host, port));
                }
            }
        }
    }

    if cameras.is_empty() {
        println!("No cameras found. Set ONVIF_STATIC_CAMERAS in .env or ensure cameras are on the network.");
        return;
    }

    println!("=== Connecting to {} cameras ===\n", cameras.len());

    // ── Connect to each camera ──
    let mut ok = 0;
    let mut fail = 0;
    for (host, port) in &cameras {
        print!("{host}:{port} ... ");
        match onvif_client::client::connect_camera(host, *port, &username, &password).await {
            Ok(cam) => {
                ok += 1;
                println!("{} {} ({})", cam.device_info.manufacturer, cam.device_info.model, cam.id);
                for p in &cam.profiles {
                    print!("  {} ({})", p.name, p.token);
                    if let Some(ve) = &p.video_encoder {
                        print!(" {}x{}@{}fps {}kbps {}", ve.width, ve.height, ve.frame_rate, ve.bitrate, ve.codec);
                    }
                    println!();
                }
                println!("  RTSP: {}", cam.stream_uri);
            }
            Err(e) => {
                fail += 1;
                println!("FAILED: {e}");
            }
        }
        println!();
    }

    println!("=== {ok} connected, {fail} failed, {} total ===", ok + fail);
}
