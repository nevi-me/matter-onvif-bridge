//! Shared types for ONVIF camera representation.

use serde::{Deserialize, Serialize};

/// Information about a discovered ONVIF camera.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CameraDevice {
    /// Unique camera identifier (serial number or host:port fallback)
    pub id: String,
    /// Hostname or IP address
    pub host: String,
    /// ONVIF service port
    pub port: u16,
    /// Device information from ONVIF GetDeviceInformation
    pub device_info: DeviceInfo,
    /// Media profiles available on the camera
    pub profiles: Vec<MediaProfile>,
    /// Primary RTSP stream URI
    pub stream_uri: String,
}

/// ONVIF device information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub manufacturer: String,
    pub model: String,
    pub firmware_version: String,
    pub serial_number: String,
    pub hardware_id: String,
}

/// A media profile from the camera.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaProfile {
    pub token: String,
    pub name: String,
    pub video_encoder: Option<VideoEncoderConfig>,
    pub audio_encoder: Option<AudioEncoderConfig>,
    pub ptz_config_token: Option<String>,
}

/// Video encoder configuration from an ONVIF profile.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoEncoderConfig {
    pub codec: String,
    pub width: u16,
    pub height: u16,
    pub frame_rate: u16,
    pub bitrate: u32,
    pub quality: f32,
}

/// Audio encoder configuration from an ONVIF profile.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioEncoderConfig {
    pub codec: String,
    pub bitrate: u32,
    pub sample_rate: u32,
}
