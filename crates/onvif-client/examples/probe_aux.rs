//! Probe a TP-Link VIGI (or any ONVIF) camera for auxiliary command support.
//!
//! Auxiliary commands are how ONVIF exposes ancillary actuators — spotlights,
//! IR illuminators, wipers, washers, heaters. The camera lists the tokens it
//! supports in `Device.GetServiceCapabilities` under `<tds:Misc
//! AuxiliaryCommands="..."/>`, but `oxvif::Capabilities` doesn't surface that
//! field, so we just try a series of well-known + vendor-likely tokens and
//! report which ones the camera accepts. A `Sender not Authorized` /
//! `Action not supported` / SOAP fault is the camera saying "no".
//!
//! Usage:
//!   ONVIF_USERNAME=... ONVIF_PASSWORD=... \
//!     cargo run --release --example probe_aux -- <host> [port]
//!
//! Defaults to port 2020 (VIGI's ONVIF default).

use std::env;

use oxvif::OnvifSession;

/// ONVIF spec tokens + plausible vendor variations. `tt:` is the ONVIF Topic
/// namespace alias used in spec-defined commands; vendors sometimes invent
/// their own tokens too.
const TOKENS: &[&str] = &[
    // ONVIF spec-defined
    "tt:Light|On",
    "tt:Light|Off",
    "tt:IrLamp|Auto",
    "tt:IrLamp|On",
    "tt:IrLamp|Off",
    "tt:Wiper|On",
    "tt:Wiper|Off",
    "tt:Washer|On",
    "tt:Heater|On",
    "tt:Heater|Off",
    // Spotlight / floodlight / whitelight tokens commonly seen on Reolink,
    // Dahua, Hikvision, TP-Link
    "tt:Whitelight|On",
    "tt:Whitelight|Off",
    "tt:WhiteLight|On",
    "tt:WhiteLight|Off",
    "tt:Spotlight|On",
    "tt:Spotlight|Off",
    "tt:Floodlight|On",
    "tt:Floodlight|Off",
    // Some firmwares accept the suffix variants
    "tt:Light|Auto",
    "tt:IrLamp|Day",
    "tt:IrLamp|Night",
];

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: probe_aux <host> [port]");
        std::process::exit(2);
    }
    let host = &args[1];
    let port = args.get(2).map(String::as_str).unwrap_or("2020");
    let user = env::var("ONVIF_USERNAME").map_err(|_| "ONVIF_USERNAME not set")?;
    let pass = env::var("ONVIF_PASSWORD").map_err(|_| "ONVIF_PASSWORD not set")?;

    let device_url = format!("http://{host}:{port}/onvif/device_service");
    println!("=== {device_url} ===\n");

    let session = OnvifSession::builder(&device_url)
        .with_credentials(user, pass)
        .with_clock_sync()
        .build()
        .await?;

    let caps = session.capabilities();
    println!("Service URLs advertised:");
    println!("  device:    {:?}", caps.device.url.as_deref());
    println!("  media:     {:?}", caps.media.url.as_deref());
    println!("  imaging:   {:?}", caps.imaging.url.as_deref());
    println!("  ptz:       {:?}", caps.ptz.url.as_deref());
    println!("  events:    {:?}", caps.events.url.as_deref());
    println!("  analytics: {:?}", caps.analytics.url.as_deref());
    println!();

    let info = session.get_device_info().await?;
    println!("Device:");
    println!("  manufacturer: {}", info.manufacturer);
    println!("  model:        {}", info.model);
    println!("  firmware:     {}", info.firmware_version);
    println!("  serial:       {}", info.serial_number);
    println!("  hardware_id:  {}", info.hardware_id);
    println!();

    println!("Trying auxiliary command tokens (each '=> OK' = accepted by camera):\n");
    for token in TOKENS {
        match session.send_auxiliary_command(token).await {
            Ok(reply) => {
                let r = reply.trim();
                if r.is_empty() {
                    println!("  {:24}  => OK  (empty reply)", token);
                } else {
                    println!("  {:24}  => OK  reply={}", token, r);
                }
            }
            Err(e) => {
                let msg = format!("{e}");
                let short: String = msg
                    .lines()
                    .next()
                    .unwrap_or("")
                    .chars()
                    .take(140)
                    .collect();
                println!("  {:24}  => ERR {}", token, short);
            }
        }
    }

    Ok(())
}
