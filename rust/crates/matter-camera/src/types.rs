//! Shared types for camera clusters.
//!
//! These mirror the Matter 1.5 camera device type definitions.
//! See the TypeScript cluster definitions in src/matter/clusters/ for reference.

use serde::{Deserialize, Serialize};

/// Video codec identifiers (CameraAvStreamManagement)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum VideoCodec {
    H264 = 0,
    Hevc = 1,
    Vp8 = 2,
    Vp9 = 3,
    Av1 = 4,
}

/// Audio codec identifiers (CameraAvStreamManagement)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum AudioCodec {
    Opus = 0,
    AacLc = 1,
    G711a = 2,
    G711u = 3,
}

/// Video resolution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoResolution {
    pub width: u16,
    pub height: u16,
}

/// Video sensor parameters reported by the camera
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoSensorParams {
    pub sensor_width: u16,
    pub sensor_height: u16,
    pub max_fps: u16,
}

/// A video stream allocation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoStream {
    pub stream_id: u16,
    pub codec: VideoCodec,
    pub resolution: VideoResolution,
    pub frame_rate: u16,
    pub bitrate: u32,
}

/// An audio stream allocation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioStream {
    pub stream_id: u16,
    pub codec: AudioCodec,
    pub sample_rate: u32,
    pub bitrate: u32,
    pub channel_count: u8,
}

/// WebRTC session state
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum WebRtcStreamUsage {
    Recording = 0,
    LiveView = 1,
    Analysis = 2,
}

/// A WebRTC session tracked by WebRtcTransportProvider
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebRtcSession {
    pub session_id: u16,
    pub peer_node_id: u64,
    pub peer_fabric_index: u8,
    pub stream_usage: WebRtcStreamUsage,
    pub video_stream_id: Option<u16>,
    pub audio_stream_id: Option<u16>,
}
