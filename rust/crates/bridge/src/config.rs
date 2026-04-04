use std::env;

#[derive(Debug, Clone)]
pub struct Config {
    pub onvif: OnvifConfig,
    pub go2rtc: Go2RtcConfig,
    pub matter: MatterConfig,
    pub log_level: String,
}

#[derive(Debug, Clone)]
pub struct OnvifConfig {
    pub username: String,
    pub password: String,
    pub discovery_interval_ms: u64,
    pub discovery_mode: DiscoveryMode,
    pub static_cameras: Vec<String>,
}

#[derive(Debug, Clone)]
pub enum DiscoveryMode {
    Auto,
    Static,
}

#[derive(Debug, Clone)]
pub struct Go2RtcConfig {
    pub mode: Go2RtcMode,
    pub path: String,
    pub host: String,
    pub api_port: u16,
    pub webrtc_port: u16,
}

#[derive(Debug, Clone)]
pub enum Go2RtcMode {
    Local,
    External,
}

#[derive(Debug, Clone)]
pub struct MatterConfig {
    pub port: u16,
    pub passcode: u32,
    pub discriminator: u16,
    pub storage_path: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            onvif: OnvifConfig {
                username: "admin".into(),
                password: "admin".into(),
                discovery_interval_ms: 60_000,
                discovery_mode: DiscoveryMode::Auto,
                static_cameras: Vec::new(),
            },
            go2rtc: Go2RtcConfig {
                mode: Go2RtcMode::External,
                path: "./bin/go2rtc".into(),
                host: "localhost".into(),
                api_port: 1984,
                webrtc_port: 8555,
            },
            matter: MatterConfig {
                port: 5540,
                passcode: 20202021,
                discriminator: 3840,
                storage_path: ".matter-storage".into(),
            },
            log_level: "info".into(),
        }
    }
}

impl Config {
    pub fn from_env() -> Self {
        let defaults = Self::default();

        let static_cameras_str = env::var("ONVIF_STATIC_CAMERAS").unwrap_or_default();
        let static_cameras: Vec<String> = if static_cameras_str.is_empty() {
            Vec::new()
        } else {
            static_cameras_str.split(',').map(|s| s.trim().to_string()).collect()
        };

        let discovery_mode = match env::var("ONVIF_DISCOVERY_MODE")
            .unwrap_or_default()
            .to_lowercase()
            .as_str()
        {
            "static" => DiscoveryMode::Static,
            _ => DiscoveryMode::Auto,
        };

        let go2rtc_mode = match env::var("GO2RTC_MODE")
            .unwrap_or_default()
            .to_lowercase()
            .as_str()
        {
            "local" => Go2RtcMode::Local,
            _ => Go2RtcMode::External,
        };

        Self {
            onvif: OnvifConfig {
                username: env::var("ONVIF_USERNAME").unwrap_or(defaults.onvif.username),
                password: env::var("ONVIF_PASSWORD").unwrap_or(defaults.onvif.password),
                discovery_interval_ms: env::var("ONVIF_DISCOVERY_INTERVAL")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(defaults.onvif.discovery_interval_ms),
                discovery_mode,
                static_cameras,
            },
            go2rtc: Go2RtcConfig {
                mode: go2rtc_mode,
                path: env::var("GO2RTC_PATH").unwrap_or(defaults.go2rtc.path),
                host: env::var("GO2RTC_HOST").unwrap_or(defaults.go2rtc.host),
                api_port: env::var("GO2RTC_API_PORT")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(defaults.go2rtc.api_port),
                webrtc_port: env::var("GO2RTC_WEBRTC_PORT")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(defaults.go2rtc.webrtc_port),
            },
            matter: MatterConfig {
                port: env::var("MATTER_PORT")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(defaults.matter.port),
                passcode: env::var("MATTER_PASSCODE")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(defaults.matter.passcode),
                discriminator: env::var("MATTER_DISCRIMINATOR")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(defaults.matter.discriminator),
                storage_path: env::var("MATTER_STORAGE_PATH")
                    .unwrap_or(defaults.matter.storage_path),
            },
            log_level: env::var("LOG_LEVEL").unwrap_or(defaults.log_level),
        }
    }
}
