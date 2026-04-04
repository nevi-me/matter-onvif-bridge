//! go2rtc integration for RTSP-to-WebRTC media bridging.
//!
//! Provides:
//! - REST API client for stream registration and SDP exchange
//! - Stream manager that syncs camera registry with go2rtc
//! - WebRTC session for SDP/ICE negotiation
//! - go2rtc process lifecycle management

pub mod go2rtc_api;
pub mod go2rtc_manager;
pub mod stream_manager;
pub mod webrtc_session;
