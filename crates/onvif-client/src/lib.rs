//! ONVIF camera discovery and client using oxvif.
//!
//! Provides:
//! - WS-Discovery for automatic camera detection
//! - Per-camera ONVIF sessions for device info, profiles, stream URIs
//! - Camera registry with broadcast events

pub mod client;
pub mod discovery;
pub mod registry;
pub mod types;
