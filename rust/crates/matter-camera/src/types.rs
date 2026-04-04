//! Shared types for camera clusters (Matter 1.5 camera device type definitions).

use serde::{Deserialize, Serialize};

// ── CameraAvStreamManagement enums ──

/// VideoCodecEnum (enum8) — Matter 1.5 §4.20.5.1
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum VideoCodec {
    H264 = 0,
    Hevc = 1,
    Vvc = 2,
    Av1 = 3,
}

/// AudioCodecEnum (enum8) — Matter 1.5 §4.20.5.2
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum AudioCodec {
    Opus = 0,
    AacLc = 1,
    G711A = 2,
    G711U = 3,
}

/// StreamUsageEnum (enum8) — used by both clusters
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum StreamUsage {
    Internal = 0,
    Recording = 1,
    Analysis = 2,
    LiveView = 3,
}

/// TwoWayTalkSupportTypeEnum (enum8)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum TwoWayTalkSupport {
    NotSupported = 0,
    HalfDuplex = 1,
    FullDuplex = 2,
}

/// TriStateAutoEnum (enum8) — for NightVision, NightVisionIllum
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum TriStateAuto {
    Off = 0,
    On = 1,
    Auto = 2,
}

/// WebRtcEndReasonEnum (enum8)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum WebRtcEndReason {
    IceFailed = 0,
    IceTimeout = 1,
    UserHangup = 2,
    UserBusy = 3,
    Replaced = 4,
    NoUserMedia = 5,
    InviteTimeout = 6,
    AnsweredElsewhere = 7,
    OutOfResources = 8,
    MediaTimeout = 9,
    LowPower = 10,
    Unknown = 11,
}

// ── CameraAvStreamManagement structs ──

/// VideoResolutionStruct
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoResolution {
    pub width: u16,
    pub height: u16,
}

/// VideoSensorParamsStruct
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoSensorParams {
    pub sensor_width: u16,
    pub sensor_height: u16,
    pub hdr_capable: bool,
    pub max_fps: u16,
}

/// AudioCapabilitiesStruct
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioCapabilities {
    pub max_number_of_channels: u8,
    pub supported_codecs: Vec<AudioCodec>,
}

/// SnapshotParamsStruct
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotParams {
    pub resolution: VideoResolution,
    pub max_frame_rate: u16,
    pub image_codec: VideoCodec,
}

/// VideoStreamStruct — allocated video stream
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoStream {
    pub video_stream_id: u16,
    pub stream_usage: StreamUsage,
    pub video_codec: VideoCodec,
    pub min_frame_rate: u16,
    pub max_frame_rate: u16,
    pub min_resolution: VideoResolution,
    pub max_resolution: VideoResolution,
    pub min_bit_rate: u32,
    pub max_bit_rate: u32,
    pub min_fragment_len: u16,
    pub max_fragment_len: u16,
    pub reference_count: Option<u8>,
}

/// AudioStreamStruct — allocated audio stream
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioStream {
    pub audio_stream_id: u16,
    pub stream_usage: StreamUsage,
    pub audio_codec: AudioCodec,
    pub channel_count: u8,
    pub sample_rate: u32,
    pub bit_rate: u32,
    pub bit_depth: u8,
    pub reference_count: Option<u8>,
}

/// SnapshotStreamStruct — allocated snapshot stream
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotStream {
    pub snapshot_stream_id: u16,
    pub image_codec: VideoCodec,
    pub frame_rate: u16,
    pub min_resolution: VideoResolution,
    pub max_resolution: VideoResolution,
    pub quality: u8,
    pub reference_count: Option<u8>,
}

// ── WebRtcTransportProvider structs ──

/// WebRtcSessionStruct
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebRtcSession {
    pub id: u16,
    pub peer_node_id: u64,
    pub peer_fabric_index: u8,
    pub stream_usage: StreamUsage,
    pub video_stream_id: Option<u16>,
    pub audio_stream_id: Option<u16>,
    pub metadata_options: Option<u8>,
}

/// IceServerStruct
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IceServer {
    pub urls: Vec<String>,
    pub username: Option<String>,
    pub credential: Option<String>,
}

// ── Camera endpoint state ──

/// Full state for a single camera endpoint's clusters.
/// This is shared between CameraAvStreamManagement and WebRtcTransportProvider handlers.
#[derive(Debug, Clone)]
pub struct CameraEndpointState {
    // CameraAvStreamManagement attributes
    pub max_concurrent_video_encoders: u8,
    pub max_encoded_pixel_rate: u32,
    pub video_sensor_params: VideoSensorParams,
    pub night_vision_capable: bool,
    pub min_viewport: VideoResolution,
    pub microphone_capabilities: Vec<AudioCapabilities>,
    pub speaker_capabilities: Vec<AudioCapabilities>,
    pub two_way_talk_support: TwoWayTalkSupport,
    pub supported_snapshot_params: Vec<SnapshotParams>,
    pub hdr_capable: bool,
    pub max_network_bandwidth: u32,
    pub current_frame_rate: u16,
    pub allocated_video_streams: Vec<VideoStream>,
    pub allocated_audio_streams: Vec<AudioStream>,
    pub allocated_snapshot_streams: Vec<SnapshotStream>,
    pub ranked_video_stream_priorities: Vec<u16>,
    pub soft_recording_privacy_mode_enabled: bool,
    pub soft_livestream_privacy_mode_enabled: bool,
    pub hard_privacy_mode_on: bool,
    pub night_vision: TriStateAuto,
    pub night_vision_illum: TriStateAuto,
    pub awb_enabled: bool,
    pub auto_shutter_speed_enabled: bool,
    pub auto_iso_enabled: bool,
    pub viewport: VideoResolution,

    // Stream ID counters
    pub next_video_stream_id: u16,
    pub next_audio_stream_id: u16,
    pub next_snapshot_stream_id: u16,

    // WebRtcTransportProvider attributes
    pub current_sessions: Vec<WebRtcSession>,
    pub next_session_id: u16,
}

impl Default for CameraEndpointState {
    fn default() -> Self {
        Self {
            max_concurrent_video_encoders: 1,
            max_encoded_pixel_rate: 0,
            video_sensor_params: VideoSensorParams {
                sensor_width: 1920,
                sensor_height: 1080,
                hdr_capable: false,
                max_fps: 30,
            },
            night_vision_capable: false,
            min_viewport: VideoResolution {
                width: 320,
                height: 240,
            },
            microphone_capabilities: Vec::new(),
            speaker_capabilities: Vec::new(),
            two_way_talk_support: TwoWayTalkSupport::NotSupported,
            supported_snapshot_params: Vec::new(),
            hdr_capable: false,
            max_network_bandwidth: 10_000,
            current_frame_rate: 30,
            allocated_video_streams: Vec::new(),
            allocated_audio_streams: Vec::new(),
            allocated_snapshot_streams: Vec::new(),
            ranked_video_stream_priorities: Vec::new(),
            soft_recording_privacy_mode_enabled: false,
            soft_livestream_privacy_mode_enabled: false,
            hard_privacy_mode_on: false,
            night_vision: TriStateAuto::Off,
            night_vision_illum: TriStateAuto::Off,
            awb_enabled: true,
            auto_shutter_speed_enabled: true,
            auto_iso_enabled: true,
            viewport: VideoResolution {
                width: 1920,
                height: 1080,
            },
            next_video_stream_id: 1,
            next_audio_stream_id: 1,
            next_snapshot_stream_id: 1,
            current_sessions: Vec::new(),
            next_session_id: 1,
        }
    }
}
