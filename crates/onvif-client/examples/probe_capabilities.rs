//! Probe ONVIF imaging capabilities on all cameras.
//!
//! Checks what each camera exposes via the Imaging Service:
//! IR cut filter, brightness, contrast, white balance, exposure, etc.
//!
//!   cargo run -p onvif-client --example probe_capabilities

use oxvif::OnvifClient;
use std::time::Duration;

#[tokio::main]
async fn main() {
    let env_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../.env");
    match dotenvy::from_path(&env_path) {
        Ok(_) => println!("Loaded {}", env_path.display()),
        Err(e) => println!("Warning: could not load {}: {e}", env_path.display()),
    }

    let username = std::env::var("ONVIF_USERNAME").unwrap_or_else(|_| "admin".into());
    let password = std::env::var("ONVIF_PASSWORD").unwrap_or_else(|_| "admin".into());
    let static_cameras = std::env::var("ONVIF_STATIC_CAMERAS").unwrap_or_default();

    // Build camera list from static config
    let mut cameras: Vec<(String, u16)> = Vec::new();
    for entry in static_cameras.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let parts: Vec<&str> = entry.splitn(2, ':').collect();
        let host = parts[0].to_string();
        let port: u16 = parts.get(1).and_then(|p| p.parse().ok()).unwrap_or(80);
        cameras.push((host, port));
    }

    // Also try WS-Discovery
    println!("=== WS-Discovery probe (5s) ===");
    let discovered = oxvif::discovery::probe(Duration::from_secs(5)).await;
    println!("Discovered {} devices\n", discovered.len());

    let mut seen = std::collections::HashSet::new();
    for (h, _) in &cameras {
        seen.insert(h.clone());
    }
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
        println!("No cameras found. Set ONVIF_STATIC_CAMERAS in .env");
        return;
    }

    println!("=== Probing {} cameras ===\n", cameras.len());

    for (host, port) in &cameras {
        println!("────────────────────────────────────────");
        println!("Camera: {host}:{port}");
        println!("────────────────────────────────────────");

        let device_url = format!("http://{host}:{port}/onvif/device_service");
        let client = OnvifClient::new(&device_url).with_credentials(&username, &password);

        // Sync clock
        let client = match client.get_system_date_and_time().await {
            Ok(dt) => client.with_utc_offset(dt.utc_offset_secs()),
            Err(e) => {
                println!("  SKIP: clock sync failed: {e}\n");
                continue;
            }
        };

        // Device info
        match client.get_device_info().await {
            Ok(info) => println!(
                "  Device: {} {} (FW: {}, SN: {})",
                info.manufacturer, info.model, info.firmware_version, info.serial_number
            ),
            Err(e) => println!("  Device info failed: {e}"),
        }

        // Get capabilities
        let caps = match client.get_capabilities().await {
            Ok(c) => c,
            Err(e) => {
                println!("  SKIP: GetCapabilities failed: {e}\n");
                continue;
            }
        };

        let media_url = match caps.media.url.as_deref() {
            Some(u) => u,
            None => {
                println!("  SKIP: no media service URL\n");
                continue;
            }
        };

        // ── Services list ──
        println!("\n  ONVIF Services:");
        match client.get_services().await {
            Ok(services) => {
                for svc in &services {
                    println!("    {} → {}", svc.namespace, svc.url);
                }
            }
            Err(e) => println!("    GetServices failed: {e}"),
        }

        // ── Imaging service ──
        let imaging_url = match &caps.imaging.url {
            Some(u) => {
                println!("\n  Imaging service: {u}");
                u.clone()
            }
            None => {
                println!("\n  Imaging service: NOT AVAILABLE");
                println!("  (Camera does not expose ONVIF Imaging Service)\n");
                continue;
            }
        };

        // ── Video sources (needed for imaging calls) ──
        let video_sources = match client.get_video_sources(media_url).await {
            Ok(vs) => vs,
            Err(e) => {
                println!("  GetVideoSources failed: {e}\n");
                continue;
            }
        };

        if video_sources.is_empty() {
            println!("  No video sources found\n");
            continue;
        }

        for vs in &video_sources {
            println!(
                "\n  VideoSource: token={:?}  {}x{} @ {}fps",
                vs.token, vs.resolution.width, vs.resolution.height, vs.framerate
            );

            // ── Current imaging settings ──
            println!("\n  Current Imaging Settings:");
            match client.get_imaging_settings(&imaging_url, &vs.token).await {
                Ok(settings) => {
                    println!("    Brightness:      {:?}", settings.brightness);
                    println!("    Color Saturation:{:?}", settings.color_saturation);
                    println!("    Contrast:        {:?}", settings.contrast);
                    println!("    Sharpness:       {:?}", settings.sharpness);
                    println!("    IR Cut Filter:   {:?}", settings.ir_cut_filter);
                    println!("    White Balance:   {:?}", settings.white_balance_mode);
                    println!("    Exposure Mode:   {:?}", settings.exposure_mode);
                }
                Err(e) => println!("    GetImagingSettings FAILED: {e}"),
            }

            // ── Imaging options (supported ranges/modes) ──
            println!("\n  Imaging Options (what the camera supports):");
            match client.get_imaging_options(&imaging_url, &vs.token).await {
                Ok(opts) => {
                    if let Some(ref r) = opts.brightness {
                        println!("    Brightness range:       {:.1} – {:.1}", r.min, r.max);
                    }
                    if let Some(ref r) = opts.color_saturation {
                        println!("    Color Saturation range: {:.1} – {:.1}", r.min, r.max);
                    }
                    if let Some(ref r) = opts.contrast {
                        println!("    Contrast range:         {:.1} – {:.1}", r.min, r.max);
                    }
                    if let Some(ref r) = opts.sharpness {
                        println!("    Sharpness range:        {:.1} – {:.1}", r.min, r.max);
                    }
                    if !opts.ir_cut_filter_modes.is_empty() {
                        println!("    IR Cut Filter modes:    {:?}", opts.ir_cut_filter_modes);
                    } else {
                        println!("    IR Cut Filter modes:    NOT SUPPORTED");
                    }
                    if !opts.white_balance_modes.is_empty() {
                        println!("    White Balance modes:    {:?}", opts.white_balance_modes);
                    }
                    if !opts.exposure_modes.is_empty() {
                        println!("    Exposure modes:         {:?}", opts.exposure_modes);
                    }
                }
                Err(e) => println!("    GetOptions FAILED: {e}"),
            }

            // ── Focus/imaging status ──
            println!("\n  Imaging Status:");
            match client.imaging_get_status(&imaging_url, &vs.token).await {
                Ok(status) => println!("    {:?}", status),
                Err(e) => println!("    GetStatus FAILED: {e}"),
            }
        }

        println!();
    }

    println!("=== Done ===");
}
