//! go2rtc lifecycle management — external mode (Docker) or local subprocess.

use std::time::Duration;

use tokio::process::Command;
use tokio::time::sleep;
use tracing::{error, info, warn};

use crate::go2rtc_api::Go2RtcApi;

/// go2rtc process manager.
pub struct Go2RtcManager {
    api: Go2RtcApi,
    mode: Go2RtcMode,
    binary_path: String,
    api_port: u16,
    webrtc_port: u16,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Go2RtcMode {
    /// Connect to an already-running go2rtc (e.g., Docker Compose).
    External,
    /// Spawn go2rtc as a local subprocess.
    Local,
}

impl Go2RtcManager {
    pub fn new(
        host: &str,
        api_port: u16,
        webrtc_port: u16,
        mode: Go2RtcMode,
        binary_path: &str,
    ) -> Self {
        Self {
            api: Go2RtcApi::new(host, api_port),
            mode,
            binary_path: binary_path.to_string(),
            api_port,
            webrtc_port,
        }
    }

    /// Get a reference to the API client.
    pub fn api(&self) -> &Go2RtcApi {
        &self.api
    }

    /// Start or connect to go2rtc.
    pub async fn start(&self) -> Result<(), String> {
        match self.mode {
            Go2RtcMode::External => {
                info!("Connecting to external go2rtc...");
                self.wait_for_ready(Duration::from_secs(15)).await
            }
            Go2RtcMode::Local => {
                info!(binary = self.binary_path, "Starting local go2rtc...");
                self.spawn_local().await?;
                self.wait_for_ready(Duration::from_secs(15)).await
            }
        }
    }

    /// Poll the go2rtc API until it responds or timeout.
    async fn wait_for_ready(&self, timeout: Duration) -> Result<(), String> {
        let start = std::time::Instant::now();

        loop {
            if self.api.is_ready().await {
                info!("go2rtc is ready");
                return Ok(());
            }

            if start.elapsed() > timeout {
                return Err(format!(
                    "go2rtc not ready after {}s",
                    timeout.as_secs()
                ));
            }

            sleep(Duration::from_millis(500)).await;
        }
    }

    /// Spawn go2rtc as a local subprocess.
    async fn spawn_local(&self) -> Result<(), String> {
        // Write a minimal config
        let config = format!(
            "api:\n  listen: \":{}\"\nwebrtc:\n  listen: \":{}\"\nstreams: {{}}\n",
            self.api_port, self.webrtc_port
        );

        let config_path = std::env::temp_dir().join("go2rtc.yaml");
        std::fs::write(&config_path, config)
            .map_err(|e| format!("Failed to write go2rtc config: {e}"))?;

        let binary_path = self.binary_path.clone();
        let config_path_str = config_path.to_string_lossy().to_string();

        // Spawn in background
        tokio::spawn(async move {
            loop {
                info!("Spawning go2rtc process: {}", binary_path);
                let result = Command::new(&binary_path)
                    .arg("-config")
                    .arg(&config_path_str)
                    .kill_on_drop(true)
                    .spawn();

                match result {
                    Ok(mut child) => {
                        match child.wait().await {
                            Ok(status) => {
                                warn!(exit_code = ?status.code(), "go2rtc exited, restarting in 3s...");
                            }
                            Err(e) => {
                                error!("go2rtc process error: {e}");
                            }
                        }
                    }
                    Err(e) => {
                        error!("Failed to spawn go2rtc: {e}");
                    }
                }

                sleep(Duration::from_secs(3)).await;
            }
        });

        Ok(())
    }
}
