//! go2rtc REST API client — stream registration and WebRTC SDP exchange.
//!
//! API endpoints (go2rtc v1.9+):
//! - PUT  /api/streams?name={name}     — Register RTSP stream source
//! - DELETE /api/streams?name={name}   — Unregister stream
//! - POST /api/webrtc?src={name}       — SDP offer/answer exchange
//! - GET  /api/streams                 — List registered streams

use tracing::{debug, warn};

/// go2rtc REST API client.
#[derive(Clone)]
pub struct Go2RtcApi {
    client: reqwest::Client,
    base_url: String,
}

impl Go2RtcApi {
    /// Create a new API client pointing at the given host and port.
    pub fn new(host: &str, port: u16) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url: format!("http://{}:{}", host, port),
        }
    }

    /// Register an RTSP stream source in go2rtc.
    pub async fn add_stream(&self, name: &str, rtsp_url: &str) -> Result<(), String> {
        let url = format!("{}/api/streams?name={}", self.base_url, urlencoding::encode(name));

        debug!(name, rtsp_url, "Registering stream in go2rtc");

        let resp = self
            .client
            .put(&url)
            .body(rtsp_url.to_string())
            .send()
            .await
            .map_err(|e| format!("PUT /api/streams failed: {e}"))?;

        if !resp.status().is_success() {
            return Err(format!(
                "PUT /api/streams returned {}",
                resp.status()
            ));
        }

        Ok(())
    }

    /// Unregister a stream from go2rtc. Tolerates 404 (stream not found).
    pub async fn remove_stream(&self, name: &str) -> Result<(), String> {
        let url = format!("{}/api/streams?name={}", self.base_url, urlencoding::encode(name));

        debug!(name, "Removing stream from go2rtc");

        let resp = self
            .client
            .delete(&url)
            .send()
            .await
            .map_err(|e| format!("DELETE /api/streams failed: {e}"))?;

        if !resp.status().is_success() && resp.status().as_u16() != 404 {
            return Err(format!(
                "DELETE /api/streams returned {}",
                resp.status()
            ));
        }

        Ok(())
    }

    /// Exchange an SDP offer for an SDP answer via go2rtc's WebRTC endpoint.
    pub async fn exchange_sdp(&self, stream_name: &str, sdp_offer: &str) -> Result<String, String> {
        let url = format!(
            "{}/api/webrtc?src={}",
            self.base_url,
            urlencoding::encode(stream_name)
        );

        debug!(stream_name, sdp_offer_len = sdp_offer.len(), "Exchanging SDP with go2rtc");

        let resp = self
            .client
            .post(&url)
            .header("Content-Type", "application/sdp")
            .body(sdp_offer.to_string())
            .send()
            .await
            .map_err(|e| format!("POST /api/webrtc failed: {e}"))?;

        if !resp.status().is_success() {
            return Err(format!(
                "POST /api/webrtc returned {}",
                resp.status()
            ));
        }

        let sdp_answer = resp
            .text()
            .await
            .map_err(|e| format!("Failed to read SDP answer: {e}"))?;

        debug!(
            stream_name,
            sdp_answer_len = sdp_answer.len(),
            "SDP exchange complete"
        );

        Ok(sdp_answer)
    }

    /// Check if go2rtc is ready by polling the API endpoint.
    pub async fn is_ready(&self) -> bool {
        let url = format!("{}/api", self.base_url);
        matches!(self.client.get(&url).send().await, Ok(r) if r.status().is_success())
    }
}

/// Extract ICE candidates from an SDP string.
///
/// go2rtc typically embeds ICE candidates directly in the SDP answer,
/// so trickle ICE via WebSocket is usually not needed.
pub fn extract_ice_candidates(sdp: &str) -> Vec<String> {
    sdp.lines()
        .filter(|line| line.starts_with("a=candidate:"))
        .map(|line| line.trim().to_string())
        .collect()
}
