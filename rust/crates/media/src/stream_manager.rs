//! Stream manager — syncs camera registry with go2rtc stream registration.
//!
//! Listens for camera Added/Removed events and registers/deregisters
//! RTSP streams in go2rtc accordingly.

use onvif_client::registry::{CameraRegistry, RegistryEvent};
use tracing::{error, info};

use crate::go2rtc_api::Go2RtcApi;

/// Run the stream manager loop, listening for registry events.
pub async fn run_stream_manager(
    registry: &CameraRegistry,
    api: Go2RtcApi,
    onvif_username: &str,
    onvif_password: &str,
) {
    let mut rx = registry.subscribe();

    info!("Stream manager started — listening for camera events");

    loop {
        match rx.recv().await {
            Ok(RegistryEvent::Added(camera)) | Ok(RegistryEvent::Updated(camera)) => {
                let stream_name = sanitize_stream_name(&camera.id);
                let rtsp_url = inject_credentials(&camera.stream_uri, onvif_username, onvif_password);

                if rtsp_url.is_empty() {
                    error!(camera_id = camera.id, "No RTSP URL for camera, skipping stream registration");
                    continue;
                }

                match api.add_stream(&stream_name, &rtsp_url).await {
                    Ok(()) => {
                        info!(
                            camera_id = camera.id,
                            stream_name,
                            "Registered RTSP stream in go2rtc"
                        );
                    }
                    Err(e) => {
                        error!(
                            camera_id = camera.id,
                            stream_name,
                            err = %e,
                            "Failed to register stream in go2rtc"
                        );
                    }
                }
            }
            Ok(RegistryEvent::Removed(id)) => {
                let stream_name = sanitize_stream_name(&id);

                match api.remove_stream(&stream_name).await {
                    Ok(()) => {
                        info!(camera_id = id, stream_name, "Removed stream from go2rtc");
                    }
                    Err(e) => {
                        error!(
                            camera_id = id,
                            stream_name,
                            err = %e,
                            "Failed to remove stream from go2rtc"
                        );
                    }
                }
            }
            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                error!(missed = n, "Stream manager missed events — registry channel lagged");
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                info!("Registry channel closed, stream manager exiting");
                break;
            }
        }
    }
}

/// Sanitize camera ID into a go2rtc-compatible stream name.
/// Replaces non-alphanumeric characters (except - and _) with underscores.
fn sanitize_stream_name(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Inject ONVIF credentials into an RTSP URL if not already present.
fn inject_credentials(uri: &str, username: &str, password: &str) -> String {
    if uri.is_empty() {
        return String::new();
    }

    match url::Url::parse(uri) {
        Ok(mut parsed) => {
            if parsed.username().is_empty() {
                let _ = parsed.set_username(username);
                let _ = parsed.set_password(Some(password));
            }
            parsed.to_string()
        }
        Err(_) => {
            // Can't parse — return as-is
            uri.to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_stream_name() {
        assert_eq!(sanitize_stream_name("camera-1"), "camera-1");
        assert_eq!(sanitize_stream_name("192.168.1.1:554"), "192_168_1_1_554");
        assert_eq!(sanitize_stream_name("ABC123"), "ABC123");
    }

    #[test]
    fn test_inject_credentials() {
        assert_eq!(
            inject_credentials("rtsp://192.168.1.1:554/stream", "admin", "pass"),
            "rtsp://admin:pass@192.168.1.1:554/stream"
        );
        assert_eq!(
            inject_credentials("rtsp://user:existing@192.168.1.1:554/stream", "admin", "pass"),
            "rtsp://user:existing@192.168.1.1:554/stream"
        );
        assert_eq!(inject_credentials("", "admin", "pass"), "");
    }
}
