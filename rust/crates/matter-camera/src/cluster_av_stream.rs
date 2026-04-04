//! CameraAvStreamManagement cluster (0x0551)
//!
//! Manages audio/video stream lifecycle for Matter camera devices.
//! Reference: Matter 1.5 Application Cluster Specification, Section 4.20
//!
//! TODO (Phase 1):
//! - Define attribute IDs, command IDs
//! - Implement DataModelHandler for read/write/invoke
//! - TLV encode/decode all request/response structs

/// Cluster ID for CameraAvStreamManagement
pub const CLUSTER_ID: u32 = 0x0551;

/// Attribute IDs
pub mod attr {
    pub const MAX_CONCURRENT_VIDEO_ENCODERS: u32 = 0x0000;
    pub const MAX_ENCODED_PIXEL_RATE: u32 = 0x0001;
    pub const VIDEO_SENSOR_PARAMS: u32 = 0x0002;
    pub const NIGHT_VISION_CAPABLE: u32 = 0x0003;
    pub const MIN_VIEWPORT: u32 = 0x0004;
    pub const SUPPORTED_VIDEO_STREAM_CONFIGS: u32 = 0x0005;
    pub const SUPPORTED_AUDIO_STREAM_CONFIGS: u32 = 0x0006;
    pub const SUPPORTED_SNAPSHOT_CONFIGS: u32 = 0x0007;
    pub const HDR_CAPABLE: u32 = 0x0008;
    pub const MAX_NETWORK_BANDWIDTH: u32 = 0x0009;
    pub const CURRENT_FRAME_RATE: u32 = 0x000A;
    pub const ALLOCATED_VIDEO_STREAMS: u32 = 0x000B;
    pub const ALLOCATED_AUDIO_STREAMS: u32 = 0x000C;
    pub const ALLOCATED_SNAPSHOT_STREAMS: u32 = 0x000D;
}

/// Command IDs
pub mod cmd {
    pub const AUDIO_STREAM_ALLOCATE: u32 = 0x0000;
    pub const AUDIO_STREAM_DEALLOCATE: u32 = 0x0001;
    pub const VIDEO_STREAM_ALLOCATE: u32 = 0x0002;
    pub const VIDEO_STREAM_MODIFY: u32 = 0x0003;
    pub const VIDEO_STREAM_DEALLOCATE: u32 = 0x0004;
    pub const SNAPSHOT_STREAM_ALLOCATE: u32 = 0x0005;
    pub const SNAPSHOT_STREAM_DEALLOCATE: u32 = 0x0006;
    pub const SET_STREAM_PRIORITIES: u32 = 0x0007;
    pub const CAPTURE_SNAPSHOT: u32 = 0x0008;
}
