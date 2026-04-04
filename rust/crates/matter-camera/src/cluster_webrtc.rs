//! WebRtcTransportProvider cluster (0x0553) — Matter 1.5 §4.22
//!
//! Enables WebRTC-based streaming for Matter camera devices.

use std::sync::{Arc, RwLock};

use rs_matter::attributes;
use rs_matter::commands;
use rs_matter::dm::{
    Access, Attribute, Cluster, Command, Dataver, Handler, InvokeContext, InvokeReply,
    NonBlockingHandler, Quality, ReadContext, ReadReply, Reply,
};
use rs_matter::error::{Error, ErrorCode};
use rs_matter::tlv::{TLVTag, TLVWrite};
use strum::FromRepr;
use tracing::debug;

use crate::types::CameraEndpointState;

pub const CLUSTER_ID: u32 = 0x0553;
const CLUSTER_REVISION: u16 = 1;

// ── Attribute enum ──

#[derive(Clone, Copy, Debug, Eq, PartialEq, FromRepr)]
#[repr(u32)]
pub enum Attributes {
    CurrentSessions = 0x0000,
}

rs_matter::attribute_enum!(Attributes);

// ── Command enum ──

#[derive(Clone, Copy, Debug, Eq, PartialEq, FromRepr)]
#[repr(u32)]
pub enum Commands {
    SolicitOffer = 0x0001,
    SolicitOfferResponse = 0x0002,
    ProvideOffer = 0x0003,
    ProvideOfferResponse = 0x0004,
    ProvideAnswer = 0x0005,
    ProvideIceCandidates = 0x0006,
    EndSession = 0x0007,
}

rs_matter::command_enum!(Commands);

// ── Cluster metadata ──

pub const WEBRTC_CLUSTER: Cluster<'static> = Cluster::new(
    CLUSTER_ID,
    CLUSTER_REVISION,
    0, // feature_map
    attributes!(
        Attribute::new(Attributes::CurrentSessions as _, Access::RV, Quality::NONE)
    ),
    commands!(
        Command::new(Commands::ProvideOffer as _, Some(Commands::ProvideOfferResponse as _), Access::WO),
        Command::new(Commands::SolicitOffer as _, Some(Commands::SolicitOfferResponse as _), Access::WO),
        Command::new(Commands::ProvideAnswer as _, None, Access::WO),
        Command::new(Commands::ProvideIceCandidates as _, None, Access::WO),
        Command::new(Commands::EndSession as _, None, Access::WO)
    ),
    |_, _, _| true,
    |_, _, _| true,
);

// ── Handler ──

pub struct WebRtcHandler {
    dataver: Dataver,
    state: Arc<RwLock<CameraEndpointState>>,
}

impl WebRtcHandler {
    pub fn new(dataver: Dataver, state: Arc<RwLock<CameraEndpointState>>) -> Self {
        Self { dataver, state }
    }
}

impl Handler for WebRtcHandler {
    fn read(&self, ctx: impl ReadContext, reply: impl ReadReply) -> Result<(), Error> {
        let attr = ctx.attr();

        if let Some(mut writer) = reply.with_dataver(self.dataver.get())? {
            if attr.is_system() {
                return WEBRTC_CLUSTER.read(attr, writer);
            }

            let state = self.state.read().map_err(|_| ErrorCode::Busy)?;

            match attr.attr_id.try_into()? {
                Attributes::CurrentSessions => {
                    let tag = writer.tag().clone();
                    let mut tw = writer.writer();
                    tw.start_array(&tag)?;
                    for session in &state.current_sessions {
                        tw.start_struct(&TLVTag::Anonymous)?;
                        tw.u16(&TLVTag::Context(1), session.id)?;
                        tw.u64(&TLVTag::Context(2), session.peer_node_id)?;
                        tw.u8(&TLVTag::Context(3), session.peer_fabric_index)?;
                        tw.u8(&TLVTag::Context(4), session.stream_usage as u8)?;
                        if let Some(vid) = session.video_stream_id {
                            tw.u16(&TLVTag::Context(5), vid)?;
                        }
                        if let Some(aid) = session.audio_stream_id {
                            tw.u16(&TLVTag::Context(6), aid)?;
                        }
                        tw.end_container()?;
                    }
                    tw.end_container()?;
                    drop(tw);
                    writer.complete()
                }
            }
        } else {
            Ok(())
        }
    }

    fn invoke(&self, ctx: impl InvokeContext, reply: impl InvokeReply) -> Result<(), Error> {
        let cmd = ctx.cmd();
        let data = ctx.data();
        let fields = data.structure()?;

        match cmd.cmd_id.try_into()? {
            Commands::ProvideOffer => {
                let sdp_offer = fields.find_ctx(1)?.utf8().unwrap_or("");
                let stream_usage = fields.find_ctx(2)?.u8().unwrap_or(3);

                let mut state = self.state.write().map_err(|_| ErrorCode::Busy)?;
                let session_id = state.next_session_id;
                state.next_session_id += 1;

                debug!(
                    session_id,
                    sdp_offer_len = sdp_offer.len(),
                    "ProvideOffer (stub — returning empty SDP answer)"
                );

                state.current_sessions.push(crate::types::WebRtcSession {
                    id: session_id,
                    peer_node_id: 0,
                    peer_fabric_index: cmd.fab_idx,
                    stream_usage: match stream_usage {
                        0 => crate::types::StreamUsage::Internal,
                        1 => crate::types::StreamUsage::Recording,
                        2 => crate::types::StreamUsage::Analysis,
                        _ => crate::types::StreamUsage::LiveView,
                    },
                    video_stream_id: None,
                    audio_stream_id: None,
                    metadata_options: None,
                });
                self.dataver.changed();

                let mut writer = reply.with_command(Commands::ProvideOfferResponse as _)?;
                let tag = writer.tag().clone();
                let mut tw = writer.writer();
                tw.start_struct(&tag)?;
                tw.u16(&TLVTag::Context(0), session_id)?;
                tw.utf8(&TLVTag::Context(1), "v=0\r\n")?; // Stub SDP answer
                tw.end_container()?;
                drop(tw);
                writer.complete()
            }
            Commands::SolicitOffer => {
                debug!("SolicitOffer not implemented");
                Err(ErrorCode::CommandNotFound.into())
            }
            Commands::ProvideAnswer => {
                debug!("ProvideAnswer not implemented");
                Err(ErrorCode::CommandNotFound.into())
            }
            Commands::ProvideIceCandidates => {
                let session_id = fields.find_ctx(0)?.u16().unwrap_or(0);
                debug!(session_id, "ProvideIceCandidates (stub — ignoring)");
                Ok(())
            }
            Commands::EndSession => {
                let session_id = fields.find_ctx(0)?.u16().unwrap_or(0);
                let reason = fields.find_ctx(1)?.u8().unwrap_or(11);

                let mut state = self.state.write().map_err(|_| ErrorCode::Busy)?;
                state.current_sessions.retain(|s| s.id != session_id);
                self.dataver.changed();

                debug!(session_id, reason, "EndSession");
                Ok(())
            }
            Commands::SolicitOfferResponse | Commands::ProvideOfferResponse => {
                Err(ErrorCode::CommandNotFound.into())
            }
        }
    }
}

impl NonBlockingHandler for WebRtcHandler {}
