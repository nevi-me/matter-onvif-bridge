# Matter-ONVIF Camera Bridge

A Rust bridge that discovers ONVIF IP cameras and exposes them as Matter camera devices. Uses [rs-matter](https://github.com/project-chip/rs-matter) for the Matter protocol, [oxvif](https://github.com/smiti1642/oxvif) for ONVIF discovery/control, and [go2rtc](https://github.com/AlexxIT/go2rtc) for RTSP-to-WebRTC media bridging.

## Features

- **Matter 1.5 camera clusters** — CameraAvStreamManagement (0x0551) and WebRTCTransportProvider (0x0553)
- **ONVIF discovery** — WS-Discovery + static camera list, with miss-threshold grace period
- **WebRTC streaming** — SDP exchange via go2rtc for H.264/H.265 passthrough
- **Bridge architecture** — up to 16 cameras exposed as bridged Matter endpoints
- **Small footprint** — ~10MB binary, ~20MB runtime on Raspberry Pi

## Quick Start

```bash
# 1. Copy .env.example and configure
cp .env.example .env
# Edit .env with your ONVIF camera credentials

# 2. Start go2rtc (for media bridging)
docker compose up -d

# 3. Run the bridge
cargo run -p matter-onvif-bridge

# 4. Commission with chip-tool or scan the QR code with Google Home
chip-tool pairing onnetwork 2 20202021
```

## Cross-Compile for Raspberry Pi

```bash
# Install cross (Docker-based cross-compilation)
cargo install cross --git https://github.com/cross-rs/cross

# Build for aarch64
cross build --release --target aarch64-unknown-linux-gnu -p matter-onvif-bridge

# Deploy
scp target/aarch64-unknown-linux-gnu/release/matter-onvif-bridge pi@<host>:~/
```

## Test ONVIF Connectivity

```bash
# Reads cameras from .env, tests WS-Discovery and direct connections
cargo run -p onvif-client --example test_connect
```

## Configuration

All configuration is via environment variables (`.env` file):

| Variable | Default | Description |
|----------|---------|-------------|
| `ONVIF_USERNAME` | `admin` | ONVIF camera credentials |
| `ONVIF_PASSWORD` | `admin` | ONVIF camera credentials |
| `ONVIF_DISCOVERY_MODE` | `auto` | `auto` (WS-Discovery + static) or `static` |
| `ONVIF_DISCOVERY_INTERVAL` | `60000` | Rescan interval in ms |
| `ONVIF_STATIC_CAMERAS` | | Comma-separated `host:port` list |
| `GO2RTC_MODE` | `external` | `external` (Docker) or `local` (subprocess) |
| `GO2RTC_PATH` | `./bin/go2rtc` | Binary path for local mode |
| `GO2RTC_HOST` | `localhost` | go2rtc API host |
| `GO2RTC_API_PORT` | `1984` | go2rtc REST API port |
| `GO2RTC_WEBRTC_PORT` | `8555` | go2rtc WebRTC port |
| `MATTER_PORT` | `5540` | Matter protocol port |
| `MATTER_PASSCODE` | `20202021` | Commissioning passcode |
| `MATTER_DISCRIMINATOR` | `3840` | Commissioning discriminator |
| `MATTER_STORAGE_PATH` | `.matter-storage` | Persistent state directory |

## Architecture

```
ONVIF Cameras (RTSP + SOAP)
    ↓
[oxvif WS-Discovery + Client] ← discovers cameras, fetches device info
    ↓
[CameraRegistry] ← broadcast events (Added/Removed)
    ↓
├→ [StreamManager] → registers RTSP streams in go2rtc
├→ [MatterBridge] → populates BridgedNodeEndpoints
│   ├→ CameraAvStreamManagement (0x0551)
│   └→ WebRtcTransportProvider (0x0553)
│       └→ ProvideOffer → go2rtc SDP exchange → WebRTC stream
│
Matter Controllers (Google Home, chip-tool)
    ↓
[rs-matter] ← mDNS discovery, PASE commissioning, CASE sessions
```

## Workspace Structure

```
crates/
  bridge/          # Main binary — Matter bridge + ONVIF + go2rtc wiring
  matter-camera/   # Custom Matter cluster implementations (0x0551, 0x0553)
  onvif-client/    # ONVIF discovery, client, and camera registry
  media/           # go2rtc API client, stream manager, WebRTC sessions
```

## License

MIT
