mod config;

use tracing::info;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_env("LOG_LEVEL")
                .unwrap_or_else(|_| EnvFilter::new(&config::Config::default().log_level)),
        )
        .init();

    let cfg = config::Config::from_env();
    info!("Matter-ONVIF Bridge (Rust) starting");
    info!(
        "ONVIF discovery mode: {:?}, go2rtc mode: {:?}",
        cfg.onvif.discovery_mode, cfg.go2rtc.mode
    );
    info!(
        "Matter port: {}, passcode: {}, discriminator: {}",
        cfg.matter.port, cfg.matter.passcode, cfg.matter.discriminator
    );

    // Phase 1: Matter bridge with stub endpoints
    // Phase 2: ONVIF discovery + camera registry
    // Phase 3: go2rtc media pipeline

    info!("Scaffold complete — no bridge logic yet. See docs/rust-rewrite-plan.md for roadmap.");
    Ok(())
}
