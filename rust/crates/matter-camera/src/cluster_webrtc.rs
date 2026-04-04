//! WebRtcTransportProvider cluster (0x0553)
//!
//! Enables WebRTC-based streaming for Matter camera devices.
//! Reference: Matter 1.5 Application Cluster Specification, Section 4.22
//!
//! TODO (Phase 1):
//! - Define attribute IDs, command IDs
//! - Implement DataModelHandler for read/write/invoke
//! - TLV encode/decode all request/response structs

/// Cluster ID for WebRtcTransportProvider
pub const CLUSTER_ID: u32 = 0x0553;

/// Attribute IDs
pub mod attr {
    pub const CURRENT_SESSIONS: u32 = 0x0000;
}

/// Command IDs
pub mod cmd {
    pub const SOLICIT_OFFER: u32 = 0x0001;
    pub const PROVIDE_OFFER: u32 = 0x0003;
    pub const PROVIDE_ANSWER: u32 = 0x0005;
    pub const PROVIDE_ICE_CANDIDATES: u32 = 0x0006;
    pub const END_SESSION: u32 = 0x0007;
}
