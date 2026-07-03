//! Hooks impls bridging the upstream rs-matter camera clusters
//! (CameraAvStreamManagement 0x0551, WebRTCTransportProvider 0x0553) to
//! ONVIF + go2rtc.
//!
//! The cluster handlers themselves live in `rs_matter::dm::clusters::app`;
//! this module supplies the side-effecting application logic:
//!
//! * [`BridgeCamAvHooks`] is a no-op — go2rtc owns the RTSP profiles, so we
//!   pre-seed the cluster's `AllocatedVideoStreams` from ONVIF discovery and
//!   reject runtime allocate/modify/deallocate (cameras have a fixed encoder
//!   set in our deployment).
//! * [`BridgeWebRtcHooks`] forwards `ProvideOffer` to go2rtc's `/api/webrtc`
//!   endpoint, buffers the SDP answer keyed by Matter session id, and
//!   enqueues an [`OutboundWork::Answer`] so the handler can push it back to
//!   the controller via `WebRTCTransportRequestor::Answer`.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use async_channel::{Receiver, Sender};
use rs_matter::dm::clusters::app::cam_av_stream::{
    CamAvError, CameraAvStreamHooks, VideoStream,
};
use rs_matter::dm::clusters::app::webrtc_prov::{
    AnswerOutcome, OfferParams, OutboundWork, SolicitOutcome, WebRtcError, WebRtcHooks,
};
use rs_matter::dm::clusters::decl::globals::{ICECandidateStruct, WebRTCEndReasonEnum};
use rs_matter::tlv::TLVArray;
use tracing::{debug, info, warn};

use media::go2rtc_api::Go2RtcApi;

// ── CameraAvStreamManagement (0x0551) hooks ──

/// `CameraAvStreamHooks` impl that rejects all runtime stream lifecycle
/// changes. Streams are pre-seeded by `crates/bridge/src/onvif_bridge.rs`
/// via `CameraAvStreamHandler::add_preallocated_video` once ONVIF discovery
/// resolves a camera's media profiles.
pub struct BridgeCamAvHooks;

impl CameraAvStreamHooks for BridgeCamAvHooks {
    async fn allocate_video(&self, _stream: &VideoStream) -> Result<(), CamAvError> {
        // Controllers should not be allocating streams on us — the encoder
        // set is fixed by the camera firmware. Refuse gracefully so chip-tool
        // gets a clean status code instead of a panic.
        Err(CamAvError::ResourceExhausted)
    }

    async fn modify_video(
        &self,
        _video_stream_id: u16,
        _watermark_enabled: Option<bool>,
        _osd_enabled: Option<bool>,
    ) -> Result<(), CamAvError> {
        Ok(())
    }

    async fn deallocate_video(&self, _video_stream_id: u16) -> Result<(), CamAvError> {
        // Pre-allocated streams have no underlying lifecycle in our setup;
        // accept the spec command but don't actually tear anything down.
        Ok(())
    }
}

// ── WebRTCTransportProvider (0x0553) hooks ──

/// State shared between [`BridgeWebRtcHooks`] and the rest of the bridge:
/// the go2rtc REST client, the slot → stream-name lookup populated by
/// `onvif_bridge`, and per-session SDP answer buffering.
pub struct BridgeWebRtcHooks {
    /// Endpoint slot index, 0..MAX_CAMERAS. Used to look up the go2rtc
    /// stream name in `stream_names`.
    slot: usize,
    /// go2rtc REST client (cheap to clone — `reqwest::Client` is `Arc`-internal).
    api: Go2RtcApi,
    /// Slot → go2rtc stream name. Populated by `onvif_bridge::start_onvif_bridge`
    /// when a camera is bound to a slot, cleared on disconnect.
    stream_names: Arc<RwLock<HashMap<usize, String>>>,
    /// SDP answers buffered between `on_offer` (where we receive them from
    /// go2rtc) and `take_answer_sdp` (where the handler copies them out
    /// into the outgoing `WebRTCTransportRequestor::Answer` payload).
    answers: std::sync::Mutex<HashMap<u16, String>>,
    outbound_tx: Sender<OutboundWork>,
    outbound_rx: Receiver<OutboundWork>,
}

impl BridgeWebRtcHooks {
    pub fn new(
        slot: usize,
        api: Go2RtcApi,
        stream_names: Arc<RwLock<HashMap<usize, String>>>,
    ) -> Self {
        let (outbound_tx, outbound_rx) = async_channel::unbounded();
        Self {
            slot,
            api,
            stream_names,
            answers: std::sync::Mutex::new(HashMap::new()),
            outbound_tx,
            outbound_rx,
        }
    }

    fn lookup_stream_name(&self) -> Option<String> {
        self.stream_names
            .read()
            .ok()
            .and_then(|m| m.get(&self.slot).cloned())
    }
}

impl WebRtcHooks for BridgeWebRtcHooks {
    async fn on_solicit_offer(
        &self,
        _session_id: u16,
        _params: &OfferParams,
    ) -> Result<SolicitOutcome, WebRtcError> {
        // We're a Provider only — go2rtc cannot initiate offers, so the
        // controller-pull SolicitOffer flow is unsupported. Reject with
        // INVALID_COMMAND so chip-tool sees a clean status.
        Err(WebRtcError::InvalidCommand)
    }

    async fn on_offer(
        &self,
        session_id: u16,
        sdp: &str,
        _params: &OfferParams,
    ) -> Result<AnswerOutcome, WebRtcError> {
        let stream_name = self.lookup_stream_name().ok_or_else(|| {
            warn!(slot = self.slot, "on_offer: no go2rtc stream registered");
            WebRtcError::InvalidInState
        })?;

        // go2rtc's exchange_sdp uses reqwest, which requires a tokio reactor.
        // We're running on the rs-matter executor (futures-lite/async-io), so
        // drive the call on a dedicated thread with its own current-thread
        // tokio runtime. Blocks the rs-matter task for ~100–500 ms — fine for
        // a 6-camera home setup; flagged as TODO to migrate off reqwest.
        let api = self.api.clone();
        let sdp_owned = sdp.to_string();
        let stream_owned = stream_name.clone();
        let answer_res = std::thread::scope(|scope| {
            scope
                .spawn(move || -> Result<String, String> {
                    let rt = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .map_err(|e| format!("tokio runtime: {e}"))?;
                    rt.block_on(api.exchange_sdp(&stream_owned, &sdp_owned))
                })
                .join()
                .unwrap_or_else(|_| Err("sdp exchange thread panicked".into()))
        });

        let answer = answer_res.map_err(|e| {
            warn!(slot = self.slot, session_id, err = %e, "go2rtc SDP exchange failed");
            WebRtcError::Failure
        })?;

        debug!(
            slot = self.slot,
            session_id,
            sdp_answer_len = answer.len(),
            "SDP exchange succeeded"
        );

        if let Ok(mut answers) = self.answers.lock() {
            answers.insert(session_id, answer);
        }

        // Enqueue Answer push so the handler invokes take_answer_sdp and then
        // sends WebRTCTransportRequestor::Answer back at the controller.
        if let Err(e) = self
            .outbound_tx
            .send(OutboundWork::Answer { session_id })
            .await
        {
            warn!(session_id, err = %e, "outbound queue closed");
            return Err(WebRtcError::Failure);
        }

        Ok(AnswerOutcome {
            // We always pre-allocate a single video stream per camera; its id
            // is whatever the handler returned at boot. Hardcoding 1 here
            // mirrors the upstream example's "first id"; if we ever expose
            // multiple streams per camera we'll thread the id through.
            video_stream_id: Some(1),
            audio_stream_id: None,
        })
    }

    async fn on_answer(&self, _session_id: u16, _sdp: &str) -> Result<(), WebRtcError> {
        // Deferred-offer flow not used (we never enqueue OutboundWork::Offer).
        Err(WebRtcError::InvalidInState)
    }

    async fn on_ice_candidates(
        &self,
        session_id: u16,
        _candidates: &TLVArray<'_, ICECandidateStruct<'_>>,
    ) -> Result<(), WebRtcError> {
        // go2rtc bundles ICE candidates inside the SDP answer body
        // (see media::go2rtc_api::extract_ice_candidates), so we don't need to
        // forward remote trickle candidates back to it.
        debug!(slot = self.slot, session_id, "remote ICE candidates ignored");
        Ok(())
    }

    async fn on_end_session(
        &self,
        session_id: u16,
        reason: WebRTCEndReasonEnum,
    ) -> Result<(), WebRtcError> {
        if let Ok(mut answers) = self.answers.lock() {
            answers.remove(&session_id);
        }
        info!(slot = self.slot, session_id, ?reason, "session ended");
        Ok(())
    }

    async fn next_outbound(&self) -> OutboundWork {
        match self.outbound_rx.recv().await {
            Ok(work) => work,
            Err(_) => core::future::pending::<OutboundWork>().await,
        }
    }

    async fn take_answer_sdp(
        &self,
        session_id: u16,
        sdp_out: &mut [u8],
    ) -> Result<usize, WebRtcError> {
        let sdp = self
            .answers
            .lock()
            .map_err(|_| WebRtcError::Failure)?
            .remove(&session_id)
            .ok_or(WebRtcError::InvalidInState)?;

        if sdp.len() > sdp_out.len() {
            warn!(
                slot = self.slot,
                session_id,
                sdp_len = sdp.len(),
                buf_len = sdp_out.len(),
                "answer SDP exceeds outbound buffer"
            );
            return Err(WebRtcError::ResourceExhausted);
        }
        sdp_out[..sdp.len()].copy_from_slice(sdp.as_bytes());
        Ok(sdp.len())
    }
}
