//! Custom Matter cluster implementations for camera devices.
//!
//! Implements the camera-specific clusters not yet available in rs-matter:
//! - CameraAvStreamManagement (0x0551)
//! - WebRtcTransportProvider (0x0553)

pub mod cluster_av_stream;
pub mod cluster_webrtc;
pub mod types;
