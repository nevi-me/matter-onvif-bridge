//! WebRTC session management — SDP offer/answer negotiation via go2rtc.

use std::sync::atomic::{AtomicU16, Ordering};

use tracing::debug;

use crate::go2rtc_api::{self, Go2RtcApi};

/// Global session ID counter.
static NEXT_SESSION_ID: AtomicU16 = AtomicU16::new(1);

/// A WebRTC session that bridges a Matter controller to a go2rtc stream.
pub struct WebRtcSession {
    pub id: u16,
    pub stream_name: String,
    api: Go2RtcApi,
}

/// Result of SDP negotiation.
pub struct NegotiateResult {
    /// The SDP answer from go2rtc.
    pub sdp_answer: String,
    /// ICE candidates extracted from the SDP answer.
    pub ice_candidates: Vec<String>,
}

impl WebRtcSession {
    /// Create a new WebRTC session for the given stream.
    pub fn new(stream_name: String, api: Go2RtcApi) -> Self {
        let id = NEXT_SESSION_ID.fetch_add(1, Ordering::Relaxed);
        debug!(session_id = id, stream_name, "Created WebRTC session");
        Self {
            id,
            stream_name,
            api,
        }
    }

    /// Negotiate SDP: send offer to go2rtc, receive answer with embedded ICE candidates.
    pub async fn negotiate(&self, sdp_offer: &str) -> Result<NegotiateResult, String> {
        debug!(
            session_id = self.id,
            stream_name = self.stream_name,
            sdp_offer_len = sdp_offer.len(),
            "Negotiating SDP with go2rtc"
        );

        let sdp_answer = self
            .api
            .exchange_sdp(&self.stream_name, sdp_offer)
            .await?;

        let ice_candidates = go2rtc_api::extract_ice_candidates(&sdp_answer);

        debug!(
            session_id = self.id,
            sdp_answer_len = sdp_answer.len(),
            ice_candidates = ice_candidates.len(),
            "SDP negotiation complete"
        );

        Ok(NegotiateResult {
            sdp_answer,
            ice_candidates,
        })
    }
}
