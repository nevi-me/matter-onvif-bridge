//! Custom Matter cluster handlers that aren't (yet) in upstream rs-matter.
//!
//! Currently just OccupancySensing (0x0406), driven by ONVIF MotionAlarm
//! events. The Matter 1.5 camera clusters (CameraAvStreamManagement 0x0551,
//! WebRTCTransportProvider 0x0553) live in upstream rs-matter as of PR #423
//! and are wired directly from `crates/bridge`.

pub mod cluster_occupancy;

pub use cluster_occupancy::{
    MotionState, OccupancyDataver, OccupancyHandler, OCCUPANCY_CLUSTER,
};
