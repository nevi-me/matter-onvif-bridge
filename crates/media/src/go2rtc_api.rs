//! go2rtc REST API client — stream registration and WebRTC SDP exchange.
//!
//! API endpoints (go2rtc v1.9+):
//! - PUT  /api/streams?name={name}     — Register RTSP stream source
//! - DELETE /api/streams?name={name}   — Unregister stream
//! - POST /api/webrtc?src={name}       — SDP offer/answer exchange
//! - GET  /api/streams                 — List registered streams

use std::collections::HashMap;

use serde::Deserialize;
use tracing::debug;

#[derive(Debug, Deserialize)]
struct StreamProducer {
    url: String,
}

#[derive(Debug, Deserialize)]
struct StreamInfo {
    #[serde(default)]
    producers: Vec<StreamProducer>,
}

/// Registration state of a named stream in go2rtc.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamState {
    /// No stream with this name is registered.
    Absent,
    /// Registered and the first producer URL matches ours.
    Matches,
    /// Registered under this name but with a different producer URL —
    /// either a stale registration or go2rtc normalized the URL.
    Differs { stored: String },
    /// go2rtc unreachable or returned garbage.
    Unknown,
}

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
    /// go2rtc 1.9+ expects the source URL as a `src` query parameter.
    pub async fn add_stream(&self, name: &str, rtsp_url: &str) -> Result<(), String> {
        let url = format!(
            "{}/api/streams?name={}&src={}",
            self.base_url,
            urlencoding::encode(name),
            urlencoding::encode(rtsp_url),
        );

        debug!(name, rtsp_url, "Registering stream in go2rtc");

        let resp = self
            .client
            .put(&url)
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

    /// Query the registration state of the stream named `name`.
    ///
    /// go2rtc returns 400 on `PUT /api/streams` when the name already
    /// exists, and the producer URL it reports back is not always
    /// byte-identical to what we registered (it may re-encode credentials
    /// or normalize the URL), so equality against `expected_url` is only a
    /// best-effort signal — callers must treat `Differs` as "re-register",
    /// not as an error.
    pub async fn stream_state(&self, name: &str, expected_url: &str) -> StreamState {
        let url = format!("{}/api/streams", self.base_url);
        let Ok(resp) = self.client.get(&url).send().await else {
            return StreamState::Unknown;
        };
        if !resp.status().is_success() {
            return StreamState::Unknown;
        }
        let Ok(streams) = resp.json::<HashMap<String, StreamInfo>>().await else {
            return StreamState::Unknown;
        };
        match streams.get(name) {
            None => StreamState::Absent,
            Some(s) => {
                let stored = s.producers.first().map(|p| p.url.as_str()).unwrap_or("");
                if stored == expected_url {
                    StreamState::Matches
                } else {
                    StreamState::Differs {
                        stored: stored.to_string(),
                    }
                }
            }
        }
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
