//! Per-camera ONVIF client — connects, fetches device info, profiles, and stream URIs.

use oxvif::OnvifClient as OxvifClient;
use tracing::{debug, warn};

use crate::types::{
    AudioEncoderConfig, CameraDevice, DeviceInfo, MediaProfile, VideoEncoderConfig,
};

/// Connect to a single ONVIF camera and retrieve its full device information.
///
/// This performs the following ONVIF calls:
/// 1. GetCapabilities (to discover service URLs)
/// 2. GetDeviceInformation (manufacturer, model, serial, etc.)
/// 3. GetProfiles (media profiles)
/// 4. GetVideoEncoderConfigurations (codec, resolution, fps, bitrate)
/// 5. GetAudioEncoderConfigurations (codec, bitrate, sample rate)
/// 6. GetStreamUri (RTSP URL for the first profile)
pub async fn connect_camera(
    host: &str,
    port: u16,
    username: &str,
    password: &str,
) -> Result<CameraDevice, String> {
    let device_url = format!("http://{}:{}/onvif/device_service", host, port);
    debug!(device_url, "Connecting to ONVIF camera");

    let client = OxvifClient::new(&device_url).with_credentials(username, password);

    // Sync clock offset for WS-Security timestamp
    let dt = client
        .get_system_date_and_time()
        .await
        .map_err(|e| format!("GetSystemDateAndTime failed: {e}"))?;
    let client = client.with_utc_offset(dt.utc_offset_secs());

    // Get service URLs
    let caps = client
        .get_capabilities()
        .await
        .map_err(|e| format!("GetCapabilities failed: {e}"))?;

    let media_url = caps
        .media
        .url
        .as_deref()
        .ok_or("No media service URL in capabilities")?;

    // Device information
    let info = client
        .get_device_info()
        .await
        .map_err(|e| format!("GetDeviceInformation failed: {e}"))?;

    let device_info = DeviceInfo {
        manufacturer: info.manufacturer,
        model: info.model,
        firmware_version: info.firmware_version,
        serial_number: info.serial_number.clone(),
        hardware_id: info.hardware_id,
    };

    // Derive camera ID: prefer serial number, fall back to host:port
    let id = if info.serial_number.is_empty()
        || info.serial_number == "N/A"
        || info.serial_number == "0"
    {
        format!("{host}:{port}")
    } else {
        info.serial_number
    };

    // Media profiles
    let raw_profiles = client
        .get_profiles(media_url)
        .await
        .map_err(|e| format!("GetProfiles failed: {e}"))?;

    // Video encoder configurations
    let video_configs = client
        .get_video_encoder_configurations(media_url)
        .await
        .unwrap_or_else(|e| {
            warn!("GetVideoEncoderConfigurations failed: {e}");
            Vec::new()
        });

    // Audio encoder configurations
    let audio_configs = client
        .get_audio_encoder_configurations(media_url)
        .await
        .unwrap_or_else(|e| {
            debug!("GetAudioEncoderConfigurations failed (camera may not have audio): {e}");
            Vec::new()
        });

    // Build profiles
    let profiles: Vec<MediaProfile> = raw_profiles
        .iter()
        .map(|p| {
            // Try to find matching video encoder config
            let video_encoder = video_configs.first().map(|vc| VideoEncoderConfig {
                codec: format!("{:?}", vc.encoding),
                width: vc.resolution.width as u16,
                height: vc.resolution.height as u16,
                frame_rate: vc
                    .rate_control
                    .as_ref()
                    .map(|rc| rc.frame_rate_limit as u16)
                    .unwrap_or(30),
                bitrate: vc
                    .rate_control
                    .as_ref()
                    .map(|rc| rc.bitrate_limit)
                    .unwrap_or(4000),
                quality: vc.quality,
            });

            let audio_encoder = audio_configs.first().map(|ac| AudioEncoderConfig {
                codec: format!("{:?}", ac.encoding),
                bitrate: ac.bitrate,
                sample_rate: ac.sample_rate,
            });

            MediaProfile {
                token: p.token.clone(),
                name: p.name.clone(),
                video_encoder,
                audio_encoder,
                ptz_config_token: None, // TODO: extract from profile PTZ config
            }
        })
        .collect();

    // Stream URI for the first profile
    let stream_uri = if let Some(first_profile) = raw_profiles.first() {
        match client
            .get_stream_uri(media_url, &first_profile.token)
            .await
        {
            Ok(uri) => uri.uri,
            Err(e) => {
                warn!("GetStreamUri failed: {e}");
                String::new()
            }
        }
    } else {
        String::new()
    };

    debug!(
        id,
        manufacturer = device_info.manufacturer,
        model = device_info.model,
        profiles = profiles.len(),
        stream_uri,
        "Camera connected"
    );

    Ok(CameraDevice {
        id,
        host: host.to_string(),
        port,
        device_info,
        profiles,
        stream_uri,
    })
}
