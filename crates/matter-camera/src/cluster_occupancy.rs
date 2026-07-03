//! OccupancySensing cluster (0x0406) — Matter 1.4 §2.7
//!
//! Backed by ONVIF MotionAlarm events. The cluster is only added to a camera
//! endpoint when the underlying ONVIF device advertises a MotionAlarm topic
//! (see slot pool split in `bridge::main`).

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;

use rs_matter::attributes;
use rs_matter::commands;
use rs_matter::dm::{
    Access, Attribute, Cluster, Handler, InvokeContext, InvokeReply, MatchContext,
    NonBlockingHandler, Quality, ReadContext, ReadReply, Reply, WriteContext,
};
use rs_matter::error::{Error, ErrorCode};
use strum::FromRepr;

pub const CLUSTER_ID: u32 = 0x0406;
const CLUSTER_REVISION: u16 = 5;

/// Matter 1.4 OccupancySensing attribute IDs.
#[derive(Clone, Copy, Debug, Eq, PartialEq, FromRepr)]
#[repr(u32)]
pub enum Attributes {
    Occupancy = 0x0000,
    OccupancySensorType = 0x0001,
    OccupancySensorTypeBitmap = 0x0002,
}

rs_matter::attribute_enum!(Attributes);

const SENSOR_TYPE_PIR: u8 = 0;
const SENSOR_TYPE_BITMAP_PIR: u8 = 0b0000_0001;
const FEATURE_MAP_PIR: u32 = 0b0000_0001;

pub const OCCUPANCY_CLUSTER: Cluster<'static> = Cluster::new(
    CLUSTER_ID,
    CLUSTER_REVISION,
    FEATURE_MAP_PIR,
    attributes!(
        Attribute::new(Attributes::Occupancy as _, Access::RV, Quality::NONE),
        Attribute::new(Attributes::OccupancySensorType as _, Access::RV, Quality::FIXED),
        Attribute::new(
            Attributes::OccupancySensorTypeBitmap as _,
            Access::RV,
            Quality::FIXED
        )
    ),
    commands!(),
    &[],
    |_, _, _| true,
    |_, _, _| true,
    |_, _, _| true,
);

/// Thread-safe motion flag shared between the ONVIF event pump and the
/// Matter-side `OccupancyHandler`.
#[derive(Clone, Default)]
pub struct MotionState(Arc<AtomicBool>);

impl MotionState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_detected(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }

    /// Returns `true` if the value changed.
    pub fn set(&self, v: bool) -> bool {
        self.0.swap(v, Ordering::AcqRel) != v
    }
}

/// rs-matter's `Dataver` is single-threaded (Cell behind a NoopRawMutex), so
/// we can't share it with the tokio motion-pump thread. Instead the dataver
/// counter is an `AtomicU32` cloned via `Arc`; the bridge thread calls
/// [`OccupancyDataver::bump`] when a new event arrives.
#[derive(Clone, Default)]
pub struct OccupancyDataver(Arc<AtomicU32>);

impl OccupancyDataver {
    pub fn new(initial: u32) -> Self {
        Self(Arc::new(AtomicU32::new(initial)))
    }

    pub fn get(&self) -> u32 {
        self.0.load(Ordering::Acquire)
    }

    pub fn bump(&self) -> u32 {
        self.0.fetch_add(1, Ordering::AcqRel).wrapping_add(1)
    }
}

pub struct OccupancyHandler {
    dataver: OccupancyDataver,
    motion: MotionState,
}

impl OccupancyHandler {
    pub fn new(dataver: OccupancyDataver, motion: MotionState) -> Self {
        Self { dataver, motion }
    }
}

impl Handler for OccupancyHandler {
    fn read(&self, ctx: impl ReadContext, reply: impl ReadReply) -> Result<(), Error> {
        let attr = ctx.attr();
        let dv = self.dataver.get();
        if let Some(writer) = reply.with_dataver(dv)? {
            if attr.is_system() {
                return OCCUPANCY_CLUSTER.read(attr, writer);
            }

            match attr.attr_id.try_into()? {
                Attributes::Occupancy => {
                    let bits: u8 = if self.motion.is_detected() { 0b0000_0001 } else { 0 };
                    writer.set(bits)
                }
                Attributes::OccupancySensorType => writer.set(SENSOR_TYPE_PIR),
                Attributes::OccupancySensorTypeBitmap => writer.set(SENSOR_TYPE_BITMAP_PIR),
            }
        } else {
            Ok(())
        }
    }

    fn write(&self, _ctx: impl WriteContext) -> Result<(), Error> {
        Err(ErrorCode::AttributeNotFound.into())
    }

    fn invoke(&self, _ctx: impl InvokeContext, _reply: impl InvokeReply) -> Result<(), Error> {
        Err(ErrorCode::CommandNotFound.into())
    }

    fn bump_dataver(&self, _ctx: impl MatchContext) {
        self.dataver.bump();
    }
}

impl NonBlockingHandler for OccupancyHandler {}
