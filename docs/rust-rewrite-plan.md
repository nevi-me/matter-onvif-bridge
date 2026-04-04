# Rust-Native Matter-ONVIF Bridge: Feasibility & Implementation Plan

## Context

The existing TypeScript + go2rtc bridge works but has a heavy footprint (~80-110MB on Pi). This plan evaluates whether a Rust rewrite can achieve feature parity using: **rs-matter** (Matter), **oxvif** (ONVIF), **retina** (RTSP), and **rtsp-to-webrtc** (WebRTC).

---

## Crate Evaluation Summary

| Crate | Role | Maturity | Verdict |
|-------|------|----------|---------|
| **rs-matter** | Matter protocol + bridge | Production for basics, 522 stars | Use it, but camera clusters (0x0551, 0x0553) must be built from scratch |
| **oxvif** | ONVIF discovery + control | Production, 292+ tests, async | Direct replacement for `onvif` npm — superior type safety |
| **retina** | RTSP client | Production (Moonfire NVR), H.264/H.265 | Good building block, has WebRTC example, but needs orchestration |
| **rtsp-to-webrtc** | RTSP→WebRTC | Early stage, 17 stars, 19 commits | **Not recommended** — undocumented codecs, no releases |

---

## Feature Comparison Table

| Feature | TS + go2rtc (current) | Rust + go2rtc (hybrid) | Rust pure-native |
|---------|----------------------|------------------------|------------------|
| **Matter bridge** | matter.js 0.17.0-alpha | rs-matter (bridge example works) | Same |
| **Camera clusters** | Hand-built ClusterModel | Must implement from scratch | Same |
| **Dynamic endpoints** | Runtime add/remove | Pre-allocate 16-32 slots | Same |
| **ONVIF discovery** | onvif npm (callback-based) | oxvif (async, typed, 292 tests) | Same |
| **PTZ control** | Detected, not wired | oxvif has full PTZ support | Same |
| **ONVIF events** | Not implemented | oxvif supports events | Same |
| **RTSP→WebRTC** | go2rtc (battle-tested) | go2rtc (same) | retina + webrtc-rs (custom) |
| **Codec support** | H.264/H.265/MJPEG/AAC/G.711/Opus | Same (go2rtc) | H.264/H.265/AAC/G.711 (retina) |
| **Concurrent streams** | go2rtc manages | go2rtc manages | Must build stream pool |
| **Reconnection** | go2rtc auto-reconnects | go2rtc auto-reconnects | Must implement |
| **Memory (Pi 4)** | ~80-110MB | ~20-30MB | ~15-25MB |
| **Binary size** | ~200MB (node_modules) | ~5-10MB | ~5-10MB |
| **Startup time** | ~2-3s | ~0.5s | ~0.5s |
| **Dev effort** | Done | 4-6 weeks | 12-20 weeks |

### What's NOT possible (or requires significant R&D)

| Gap | Impact | Workaround |
|-----|--------|------------|
| No camera clusters in rs-matter | HIGH — must hand-code 0x0551 (22 attrs, 13 cmds) and 0x0553 (1 attr, 7 cmds) with TLV encoding | Use TS cluster definitions as spec; implement manually in Rust |
| rs-matter endpoints are compile-time | MEDIUM — can't add/remove endpoints at runtime | Pre-allocate 16-32 slots at startup (commercial bridges do this too) |
| rtsp-to-webrtc is immature | LOW — was hoping for go2rtc replacement | Use go2rtc in hybrid mode; defer native pipeline to Phase 4 |
| rs-matter API may have breaking changes | MEDIUM — project is evolving | Pin version, accept maintenance burden |
| No ONVIF snapshot→Matter CaptureSnapshot | LOW — TS bridge also stubs this | Implement later using oxvif snapshot URI |

### What's BETTER in Rust

| Improvement | Detail |
|-------------|--------|
| **oxvif > onvif npm** | Typed structs vs XML callback soup; native PTZ + events support |
| **Memory** | 3-5x reduction on Pi |
| **Single binary** | `cargo build --release --target aarch64-unknown-linux-gnu` → one file |
| **Type safety** | Cluster definitions enforced at compile time |
| **Startup** | ~0.5s vs ~2-3s |

---

## Recommendation: Hybrid (Rust + go2rtc)

Keep go2rtc for media bridging. Its integration surface is tiny (1 HTTP POST for SDP, 1 WebSocket for ICE), but it handles the hardest problem (RTSP→WebRTC with codec passthrough). Building that natively with retina + webrtc-rs is 3-5 months of R&D.

---

## Implementation Plan

### Phase 0: Scaffold + Validate (deliverable: POC binaries)

1. Create `rust/` workspace alongside existing TS code
2. Add dependencies: `rs-matter`, `oxvif`, `reqwest`, `tokio`, `tokio-tungstenite`, `serde`, `tracing`, `dotenvy`
3. Validate independently:
   - Run rs-matter bridge.rs example → confirm commissioning with chip-tool
   - Run oxvif WS-Discovery → confirm cameras found
   - POST SDP to go2rtc `/api/webrtc` via reqwest → confirm answer returned

### Phase 1: Matter Bridge with Stub Endpoints

1. Define camera cluster types (enums, structs, attribute/command IDs) as Rust types with TLV derive macros
2. Implement `DataModelHandler` for both clusters (read attributes, dispatch commands)
3. Create bridge with AggregatorEndpoint + 16 pre-allocated BridgedNodeEndpoint slots
4. BridgedDeviceBasicInformation populated with placeholder data
5. Validate: `chip-tool pairing onnetwork` + `chip-tool read` returns stub camera attributes

### Phase 2: ONVIF Integration

1. ONVIF discovery module: WS-Discovery (oxvif) + static cameras from `ONVIF_STATIC_CAMERAS`
2. Per-camera client: device info, profiles, stream URIs via `oxvif::OnvifSession`
3. Camera registry: `Arc<RwLock<HashMap<String, CameraDevice>>>` + `tokio::sync::broadcast`
4. Wire registry → bridge: populate BDBI and camera cluster attributes from real ONVIF data
5. Miss threshold (3 scans) before marking camera lost

### Phase 3: Media Pipeline (go2rtc Integration)

1. go2rtc REST client: PUT/DELETE `/api/streams`, POST `/api/webrtc`
2. Stream manager: subscribe to registry events, register/deregister RTSP streams
3. WebRTC session: SDP exchange, ICE candidate extraction, WebSocket trickle
4. Wire ProvideOffer command → go2rtc → return SDP answer to Matter controller
5. go2rtc lifecycle: external mode (poll ready) + local mode (spawn subprocess)

### Phase 4 (Future): Native RTSP→WebRTC

Replace go2rtc with retina + webrtc-rs. Estimated 3-5 months. Only pursue if single-binary deployment becomes a hard requirement.

---

## Verification Plan

1. **Phase 0**: `chip-tool pairing onnetwork 2 20202021` succeeds; oxvif finds VIGI cameras
2. **Phase 1**: `chip-tool read camera-av-stream-management video-sensor-params 2 1` returns stub data
3. **Phase 2**: Camera names/models in `chip-tool read bridged-device-basic-information 2 1` match real ONVIF data
4. **Phase 3**: SDP offer via chip-tool/Google Home → WebRTC stream plays from camera through go2rtc
5. **Cross-compile**: `cross build --release --target aarch64-unknown-linux-gnu` produces working binary for Pi

---

## Risks

| Risk | Severity | Mitigation |
|------|----------|------------|
| Custom cluster TLV encoding in rs-matter | **HIGH** | Start Phase 1 early; if blocked, consider contributing to rs-matter or using raw byte encoding |
| rs-matter breaking API changes | MEDIUM | Pin to specific commit/version |
| Pre-allocated endpoints feel wasteful | LOW | 16 empty endpoints cost ~kilobytes; commercial bridges do this |
| go2rtc still needed | LOW | Accepted tradeoff; tiny integration surface |
