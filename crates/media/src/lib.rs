//! go2rtc integration for RTSP-to-WebRTC media bridging.
//!
//! Provides:
//! - REST API client for stream registration and SDP exchange
//! - Stream manager that syncs camera registry with go2rtc
//! - go2rtc process lifecycle management
//!
//! SDP/ICE negotiation lives in `bridge::hooks::BridgeWebRtcHooks` since
//! the upstream `WebRTCTransportProvider` cluster handler drives it.

pub mod go2rtc_api;
pub mod go2rtc_manager;
pub mod stream_manager;
