#![recursion_limit = "2048"]

mod config;
mod hooks;
mod mdns;
mod onvif_bridge;
mod slot_persistence;

use core::pin::pin;
use std::net::{TcpListener, UdpSocket};
use std::sync::Arc;
use std::sync::RwLock;

use embassy_futures::select::{select, select4, Either};
use rand::RngCore;

use rs_matter::crypto::{default_crypto, Crypto};
use rs_matter::dm::clusters::app::cam_av_stream::{
    self, CameraAvStreamConfig, CameraAvStreamHandler, Feature as CamAvFeature,
    RateDistortionPoint, StreamUsageEnum, VideoCodecEnum, VideoSensorParams,
};
use rs_matter::dm::clusters::app::webrtc_prov::{self, WebRtcProvHandler};
use rs_matter::dm::clusters::desc::{self, ClusterHandler as _};
use rs_matter::dm::clusters::groups::{self, ClusterHandler as _};
use rs_matter::dm::devices::test::{DAC_PRIVKEY, TEST_DEV_ATT, TEST_DEV_COMM};
use rs_matter::dm::devices::{DEV_TYPE_AGGREGATOR, DEV_TYPE_BRIDGED_NODE};
use rs_matter::dm::endpoints::EthSysHandlerBuilder;
use rs_matter::dm::networks::eth::EthNetwork;
use rs_matter::dm::networks::unix::UnixNetifs;
use rs_matter::dm::DeviceType;
use rs_matter::dm::{Async, DataModel, Dataver, Endpoint, EpClMatcher, Node};
use rs_matter::error::Error;
use rs_matter::im::{EthInteractionModelState, InteractionModel};
use rs_matter::pairing::qr::QrTextType;
use rs_matter::pairing::DiscoveryCapabilities;
use rs_matter::persist::DirKvBlobStore;
use rs_matter::respond::DefaultResponder;
use rs_matter::sc::pase::MAX_COMM_WINDOW_TIMEOUT_SECS;
use rs_matter::transport::exchange::MatterBuffers;
use rs_matter::transport::network::tcp::TcpNetwork;
use rs_matter::transport::network::{Address, ChainedNetwork};
use rs_matter::transport::MATTER_SOCKET_BIND_ADDR;
use rs_matter::utils::select::Coalesce;
use rs_matter::{clusters, devices, root_endpoint, Matter};

use matter_camera::{MotionState, OccupancyDataver, OccupancyHandler, OCCUPANCY_CLUSTER};

pub use rs_matter::dm::clusters::decl::bridged_device_basic_information::{
    self, ClusterHandler as _, KeepActiveRequest,
};
use rs_matter::dm::{InvokeContext, ReadContext};
use rs_matter::tlv::{TLVBuilderParent, Utf8StrBuilder};
use rs_matter::with;

use crate::hooks::{BridgeCamAvHooks, BridgeWebRtcHooks};
use crate::onvif_bridge::CameraSlot;

/// Camera device type — Matter 1.5 Camera (0x0142). Previously we used
/// 0x0103, which is OnOffLight in the Matter spec — that mistake caused
/// Google Home to render the bridge as light switches.
const DEV_TYPE_MATTER_CAMERA: DeviceType = DeviceType {
    dtype: 0x0142,
    drev: 1,
};

/// Maximum number of camera endpoints we pre-allocate.
///
/// Google Home walks every endpoint in the descriptor and shows even
/// `reachable=false` ones as "unavailable" devices, so the slot pool needs
/// to be sized close to the actual number of cameras. 8 leaves a small
/// headroom over the 6 cameras the user currently runs; bumping requires
/// a recompile because the static `Node` array and the handler chain are
/// both type-level constructs.
pub const MAX_CAMERAS: usize = 8;

/// How many of the pre-allocated camera slots include the OccupancySensing
/// cluster (for cameras whose ONVIF device advertises a MotionAlarm topic).
/// The remaining `MAX_CAMERAS - WITH_OCCUPANCY_CAMERAS` slots are camera-only.
pub const WITH_OCCUPANCY_CAMERAS: usize = 7;

/// Endpoint IDs: 0 = root, 1 = aggregator, 2..=8 = camera+occupancy slots,
/// 9 = camera-only slot.
const AGGREGATOR_EP: u16 = 1;
pub const CAMERA_EP_START: u16 = 2;

// ── CameraAvStreamManagement (0x0551) sizing ──
// Each camera pre-seeds one stream from its ONVIF profile, and controllers
// (HA's matter.js Matter Server) allocate additional LiveView streams
// dynamically. Audio is disabled — VIGI cameras encode G.711, which isn't
// in the Matter `AudioCodecEnum` (only Opus / AAC-LC).
const MAX_VIDEO: usize = 4;
const MAX_AUDIO: usize = 0;

// ── WebRTCTransportProvider (0x0553) sizing — mirrors upstream example ──
const N_SESSIONS: usize = 4;
const SDP_LEN: usize = 8 * 1024;
const OUT_LEN: usize = SDP_LEN + 1024;
const CAND_LEN: usize = 256;
const MAX_CAND: usize = 16;

pub type CamAvHandler =
    CameraAvStreamHandler<'static, BridgeCamAvHooks, MAX_VIDEO, MAX_AUDIO>;
pub type WebRtcHandler =
    WebRtcProvHandler<BridgeWebRtcHooks, N_SESSIONS, SDP_LEN, OUT_LEN, CAND_LEN, MAX_CAND>;

const STREAM_USAGES: &[StreamUsageEnum] = &[StreamUsageEnum::LiveView];

const RATE_DISTORTION: &[RateDistortionPoint] = &[
    RateDistortionPoint {
        codec: VideoCodecEnum::H264,
        min_resolution: (320, 240),
        min_bit_rate: 200_000,
    },
    RateDistortionPoint {
        codec: VideoCodecEnum::HEVC,
        min_resolution: (320, 240),
        min_bit_rate: 200_000,
    },
];

const CAM_AV_CONFIG: CameraAvStreamConfig<'static> = CameraAvStreamConfig {
    max_concurrent_encoders: 4,
    max_encoded_pixel_rate: 3840 * 2160 * 30,
    sensor: VideoSensorParams {
        sensor_width: 3840,
        sensor_height: 2160,
        // HA's Matter Server dashboard sends VideoStreamAllocate with
        // maxFrameRate=120; the handler rejects any allocation whose
        // max_frame_rate exceeds sensor.max_fps with CONSTRAINT_ERROR.
        // The sensor here is a capability ceiling for negotiation, not a
        // promise — go2rtc delivers whatever the camera actually encodes.
        max_fps: 120,
        max_hdrfps: None,
    },
    min_viewport: (320, 240),
    max_content_buffer_size: 1_048_576,
    max_network_bandwidth: 10_000,
    supported_stream_usages: STREAM_USAGES,
    default_stream_usage_priorities: STREAM_USAGES,
    rate_distortion_points: RATE_DISTORTION,
    mic_capabilities: None,
};

fn main() -> Result<(), Error> {
    // Run the actual main on a dedicated thread with a large stack. With the
    // `large-buffers` rs-matter feature enabled, `Matter` (which now embeds
    // the KV scratch buffer), `MatterBuffers`, and the deeply-nested
    // data-model handler chain push the stack well past the 8 MB OS default —
    // without this we abort with `stack overflow` before any logging reaches
    // the journal. The upstream `webrtc_camera` example dodges this by
    // putting these structs in `StaticCell` (BSS); the big-stack-thread
    // approach lets us keep plain stack values with no further refactor.
    std::thread::Builder::new()
        .name("matter-main".into())
        .stack_size(64 * 1024 * 1024)
        .spawn(matter_main)
        .expect("failed to spawn matter-main thread")
        .join()
        .expect("matter-main thread panicked")
}

fn matter_main() -> Result<(), Error> {
    dotenvy::dotenv().ok();

    env_logger::init_from_env(
        env_logger::Env::default().filter_or(env_logger::DEFAULT_FILTER_ENV, "info"),
    );

    let cfg = config::Config::from_env();
    log::info!(
        "Matter-ONVIF Bridge (Rust) — port={}, passcode={}, discriminator={}",
        cfg.matter.port,
        cfg.matter.passcode,
        cfg.matter.discriminator
    );

    // Root device basic info. This describes the bridge itself, not the
    // bridged cameras — each camera's serial/hw/sw is exposed via its own
    // BridgedDeviceBasicInformation cluster (see BridgedHandler below).
    //
    // The serial number is derived from the bridge host's hostname so it's
    // stable per-deployment. Software version is taken from the crate's
    // Cargo.toml at compile time.
    let host_serial: &'static str = Box::leak(
        hostname::get()
            .ok()
            .and_then(|s| s.into_string().ok())
            .map(|h| format!("matter-onvif-bridge@{h}"))
            .unwrap_or_else(|| "matter-onvif-bridge".to_string())
            .into_boxed_str(),
    );
    let dev_det = rs_matter::dm::clusters::basic_info::BasicInfoConfig {
        vid: 0xFFF1,
        pid: 0x8001,
        product_name: "Matter-ONVIF Camera Bridge",
        vendor_name: "matter-onvif-bridge",
        device_name: "Camera Bridge",
        serial_no: host_serial,
        hw_ver: 1,
        hw_ver_str: "1.0",
        sw_ver: 1,
        sw_ver_str: env!("CARGO_PKG_VERSION"),
        // TCP is REQUIRED for Home Assistant camera support: all WebRTC
        // transport commands carry the Large-Message (L) quality and
        // matter.js only sends/accepts them over a TCP large-payload CASE
        // session. `T=1` in the mDNS TXT record advertises the TCP listener
        // below. (In May 2026 TCP+large-buffers broke UDP commissioning via
        // over-MTU ReportData frames; rs-matter 0.2.0's MRP/mDNS reworks are
        // expected to have improved this — regression-test commissioning
        // with Google Home/Aqara before trusting it.)
        tcp_supported: true,
        ..rs_matter::dm::clusters::basic_info::BasicInfoConfig::new()
    };
    let matter = Matter::new(&dev_det, TEST_DEV_COMM, &TEST_DEV_ATT, cfg.matter.port);

    // Persistence: the KV scratch buffer now lives inside `Matter`; we only
    // provide the blob store and re-hydrate both the Matter core and the
    // interaction-model state (event-number epoch, network store).
    let persist_path = std::path::PathBuf::from(&cfg.matter.storage_path);
    let store = DirKvBlobStore::new(persist_path);
    let kv = matter.kv(store);

    let buffers: MatterBuffers = MatterBuffers::new();
    let mut state: EthInteractionModelState =
        EthInteractionModelState::new(EthNetwork::new_default());

    futures_lite::future::block_on(matter.load_persist(&kv))?;
    futures_lite::future::block_on(state.load_persist(&kv))?;

    let crypto = default_crypto(rand::thread_rng(), DAC_PRIVKEY);
    let mut rand = crypto.rand()?;

    // Per-slot bridge-side state — only what BDBI / Occupancy need to read.
    let camera_slots: Vec<Arc<RwLock<CameraSlot>>> = (0..MAX_CAMERAS)
        .map(|_| Arc::new(RwLock::new(CameraSlot::default())))
        .collect();

    // Motion flags + dataver counters for occupancy-supporting slots only.
    let motion_states: Vec<MotionState> = (0..WITH_OCCUPANCY_CAMERAS)
        .map(|_| MotionState::new())
        .collect();
    let occupancy_datavers: Vec<OccupancyDataver> = (0..WITH_OCCUPANCY_CAMERAS)
        .map(|_| OccupancyDataver::new(rand.next_u32()))
        .collect();
    let occupancy_handlers: Vec<OccupancyHandler> = (0..WITH_OCCUPANCY_CAMERAS)
        .map(|i| OccupancyHandler::new(occupancy_datavers[i].clone(), motion_states[i].clone()))
        .collect();

    // Per-slot upstream cluster handlers. We need 8 distinct handler instances
    // (each carries its own session table, Dataver, and slot-specific hooks),
    // so allocate them on the heap and leak to `'static` — much simpler than
    // declaring 8 named `StaticCell`s.
    let registry = onvif_client::registry::CameraRegistry::new(64);
    let go2rtc_api = media::go2rtc_api::Go2RtcApi::new(&cfg.go2rtc.host, cfg.go2rtc.api_port);
    let stream_names_shared: Arc<RwLock<std::collections::HashMap<usize, String>>> =
        Arc::new(RwLock::new(std::collections::HashMap::new()));

    let av_handlers_leaked: &'static [&'static CamAvHandler] = {
        let mut v: Vec<&'static CamAvHandler> = Vec::with_capacity(MAX_CAMERAS);
        for slot in 0..MAX_CAMERAS {
            let endpoint_id = CAMERA_EP_START + slot as u16;
            let h = CameraAvStreamHandler::new(
                Dataver::new_rand(&mut rand),
                endpoint_id,
                CAM_AV_CONFIG,
                CamAvFeature::VIDEO.bits(),
                BridgeCamAvHooks,
            );
            v.push(Box::leak(Box::new(h)));
        }
        Vec::leak(v)
    };

    let webrtc_handlers_leaked: &'static [&'static WebRtcHandler] = {
        let mut v: Vec<&'static WebRtcHandler> = Vec::with_capacity(MAX_CAMERAS);
        for slot in 0..MAX_CAMERAS {
            let endpoint_id = CAMERA_EP_START + slot as u16;
            let hooks = BridgeWebRtcHooks::new(slot, go2rtc_api.clone(), stream_names_shared.clone());
            let h = WebRtcProvHandler::new(Dataver::new_rand(&mut rand), endpoint_id, hooks);
            v.push(Box::leak(Box::new(h)));
        }
        Vec::leak(v)
    };

    // Cross-thread channel for "seed pre-allocated video stream into slot N".
    // The ONVIF bridge thread sends; the main thread (which owns the
    // `!Sync` cluster handlers) consumes and calls `add_preallocated_video`.
    let (seed_tx, seed_rx) =
        async_channel::unbounded::<(usize, rs_matter::dm::clusters::app::cam_av_stream::VideoStream)>();

    let _media_bridge = onvif_bridge::start_onvif_bridge(
        &cfg,
        &camera_slots,
        &motion_states,
        &occupancy_datavers,
        seed_tx,
        registry,
        go2rtc_api.clone(),
        stream_names_shared.clone(),
    );

    let im = InteractionModel::new(
        &matter,
        &crypto,
        &buffers,
        data_model(
            rand,
            av_handlers_leaked,
            webrtc_handlers_leaked,
            &occupancy_handlers,
            &camera_slots,
        ),
        &kv,
        &state,
    );

    let responder = DefaultResponder::new(&im);
    let mut respond = pin!(responder.run::<4, 4>());
    let mut im_job = pin!(im.run());

    // Dual-stack UDP socket via socket2 (Rust std sets IPV6_V6ONLY=1 by default).
    let udp_socket = {
        let s = socket2::Socket::new(
            socket2::Domain::IPV6,
            socket2::Type::DGRAM,
            Some(socket2::Protocol::UDP),
        )?;
        s.set_only_v6(false)?;
        s.set_reuse_address(true)?;
        s.bind(&MATTER_SOCKET_BIND_ADDR.into())?;
        s.set_nonblocking(true)?;
        async_io::Async::<UdpSocket>::new_nonblocking(s.into())?
    };

    // TCP listener for Large-Message payloads (WebRTC SDP / ICE invokes).
    // BasicInfoConfig advertises T=1 so controllers open large-payload CASE
    // sessions here.
    let tcp_socket = {
        let s = socket2::Socket::new(
            socket2::Domain::IPV6,
            socket2::Type::STREAM,
            Some(socket2::Protocol::TCP),
        )?;
        s.set_only_v6(false)?;
        s.set_reuse_address(true)?;
        s.bind(&MATTER_SOCKET_BIND_ADDR.into())?;
        s.listen(8)?;
        s.set_nonblocking(true)?;
        async_io::Async::<TcpListener>::new(s.into())?
    };
    let tcp = TcpNetwork::<8>::new(tcp_socket);
    log::info!("TCP transport enabled on {}", MATTER_SOCKET_BIND_ADDR);

    let mut net_send = ChainedNetwork::new(|addr: &Address| addr.is_tcp(), &tcp, &udp_socket);
    let mut net_recv = ChainedNetwork::new(|addr: &Address| addr.is_tcp(), &tcp, &udp_socket);
    let mut net_multicast = &udp_socket;

    let mut mdns = pin!(mdns::run_mdns(&matter, &crypto));
    let mut transport = pin!(matter.run(
        &crypto,
        &mut net_send,
        &mut net_recv,
        &mut net_multicast,
    ));

    // Seeding driver: receives "(slot, VideoStream)" messages from the
    // ONVIF bridge thread and calls `add_preallocated_video` on the
    // matching handler. Runs on the same thread as the rs-matter executor
    // so we can hand it `&'static` handler refs that are `!Sync`.
    let mut seed_driver = pin!(async {
        while let Ok((slot, stream)) = seed_rx.recv().await {
            if let Some(handler) = av_handlers_leaked.get(slot) {
                match handler.add_preallocated_video(stream) {
                    Ok(id) => log::info!("Seeded video stream id={id} into slot {slot}"),
                    Err(e) => log::warn!("add_preallocated_video failed for slot {slot}: {e:?}"),
                }
            } else {
                log::warn!("seed_driver: slot {slot} out of range");
            }
        }
        // Channel closed shouldn't happen during normal operation; park
        // forever rather than letting the select arm complete.
        futures_lite::future::pending::<Result<(), Error>>().await
    });

    if !matter.is_commissioned() {
        log::info!("Device not commissioned. Displaying QR code...");
        matter.print_standard_qr_text(DiscoveryCapabilities::IP)?;
        matter.print_standard_qr_code(QrTextType::Unicode, DiscoveryCapabilities::IP)?;
        matter.open_basic_comm_window(MAX_COMM_WINDOW_TIMEOUT_SECS, &crypto, &())?;
    } else {
        log::info!("Device already commissioned.");
    }

    // `Coalesce` isn't implemented for `Select5`, so wrap the four core
    // futures in a select4 first, then race the seed driver against the
    // result via a 2-way select.
    let main_fut = pin!(select4(&mut transport, &mut mdns, &mut respond, &mut im_job).coalesce());
    match futures_lite::future::block_on(select(main_fut, &mut seed_driver)) {
        Either::First(r) => r,
        Either::Second(r) => r,
    }
}

// ── Node definition ──

macro_rules! camera_endpoints {
    (
        with_occupancy: [$($occ_id:expr),* $(,)?],
        plain: [$($plain_id:expr),* $(,)?] $(,)?
    ) => {
        &[
            root_endpoint!(eth),
            Endpoint::new(
                AGGREGATOR_EP,
                devices!(DEV_TYPE_AGGREGATOR),
                clusters!(desc::DescHandler::CLUSTER),
            ),
            $(
                Endpoint::new(
                    $occ_id,
                    devices!(DEV_TYPE_MATTER_CAMERA, DEV_TYPE_BRIDGED_NODE),
                    clusters!(
                        desc::DescHandler::CLUSTER,
                        groups::GroupsHandler::CLUSTER,
                        BridgedHandler::CLUSTER,
                        CamAvHandler::CLUSTER,
                        WebRtcHandler::CLUSTER,
                        OCCUPANCY_CLUSTER
                    ),
                ),
            )*
            $(
                Endpoint::new(
                    $plain_id,
                    devices!(DEV_TYPE_MATTER_CAMERA, DEV_TYPE_BRIDGED_NODE),
                    clusters!(
                        desc::DescHandler::CLUSTER,
                        groups::GroupsHandler::CLUSTER,
                        BridgedHandler::CLUSTER,
                        CamAvHandler::CLUSTER,
                        WebRtcHandler::CLUSTER
                    ),
                ),
            )*
        ]
    }
}

const NODE: Node<'static> = Node {
    endpoints: camera_endpoints![
        with_occupancy: [2, 3, 4, 5, 6, 7, 8],
        plain: [9],
    ],
};

// ── BridgedDeviceBasicInformation handler ──

#[derive(Clone)]
pub struct BridgedHandler {
    dataver: Dataver,
    state: Arc<RwLock<CameraSlot>>,
}

impl BridgedHandler {
    pub fn new(dataver: Dataver, state: Arc<RwLock<CameraSlot>>) -> Self {
        Self { dataver, state }
    }

    pub fn adapt(self) -> bridged_device_basic_information::HandlerAdaptor<Self> {
        bridged_device_basic_information::HandlerAdaptor(self)
    }

    fn read_state_string(&self, f: impl Fn(&CameraSlot) -> &str) -> String {
        self.state
            .read()
            .map(|s| f(&s).to_string())
            .unwrap_or_default()
    }
}

impl bridged_device_basic_information::ClusterHandler for BridgedHandler {
    // Advertise only the attributes our handler actually implements. The
    // generated trait's default impl for un-overridden attributes returns
    // `InvalidAction`, so declaring `with!(all)` causes wildcard reads (e.g.
    // Google Home / Aqara post-CASE) to hit InvalidAction on every optional
    // attr we don't fill in (VendorID, ProductID, HardwareVersion (numeric),
    // SoftwareVersion (numeric), ManufacturingDate, PartNumber, ProductURL,
    // ProductLabel, ProductAppearance, and the Matter-1.5-new
    // ConfigurationVersion). Controllers see InvalidAction on a bridged
    // device and abandon commissioning by issuing RemoveFabric. Required
    // attrs (Reachable, UniqueID) come in via `required;`.
    const CLUSTER: rs_matter::dm::Cluster<'static> =
        bridged_device_basic_information::FULL_CLUSTER
            .with_features(0)
            .with_attrs(with!(
                required;
                bridged_device_basic_information::AttributeId::VendorName
                    | bridged_device_basic_information::AttributeId::ProductName
                    | bridged_device_basic_information::AttributeId::NodeLabel
                    | bridged_device_basic_information::AttributeId::SerialNumber
                    | bridged_device_basic_information::AttributeId::HardwareVersionString
                    | bridged_device_basic_information::AttributeId::SoftwareVersionString
            ))
            .with_cmds(with!());

    fn dataver(&self) -> u32 {
        self.dataver.get()
    }

    fn dataver_changed(&self) {
        self.dataver.changed();
    }

    fn reachable(&self, _ctx: impl ReadContext) -> Result<bool, Error> {
        Ok(self.state.read().map(|s| s.occupied).unwrap_or(false))
    }

    fn unique_id<P: TLVBuilderParent>(
        &self,
        _ctx: impl ReadContext,
        builder: Utf8StrBuilder<P>,
    ) -> Result<P, Error> {
        let val = self.read_state_string(|s| &s.unique_id);
        builder.set(&val)
    }

    fn vendor_name<P: TLVBuilderParent>(
        &self,
        _ctx: impl ReadContext,
        builder: Utf8StrBuilder<P>,
    ) -> Result<P, Error> {
        let val = self.read_state_string(|s| &s.vendor_name);
        builder.set(&val)
    }

    fn product_name<P: TLVBuilderParent>(
        &self,
        _ctx: impl ReadContext,
        builder: Utf8StrBuilder<P>,
    ) -> Result<P, Error> {
        let val = self.read_state_string(|s| &s.product_name);
        builder.set(&val)
    }

    fn node_label<P: TLVBuilderParent>(
        &self,
        _ctx: impl ReadContext,
        builder: Utf8StrBuilder<P>,
    ) -> Result<P, Error> {
        let val = self.read_state_string(|s| &s.node_label);
        builder.set(&val)
    }

    fn serial_number<P: TLVBuilderParent>(
        &self,
        _ctx: impl ReadContext,
        builder: Utf8StrBuilder<P>,
    ) -> Result<P, Error> {
        let val = self.read_state_string(|s| &s.serial_number);
        builder.set(&val)
    }

    fn hardware_version_string<P: TLVBuilderParent>(
        &self,
        _ctx: impl ReadContext,
        builder: Utf8StrBuilder<P>,
    ) -> Result<P, Error> {
        let val = self.read_state_string(|s| &s.hardware_version_string);
        builder.set(&val)
    }

    fn software_version_string<P: TLVBuilderParent>(
        &self,
        _ctx: impl ReadContext,
        builder: Utf8StrBuilder<P>,
    ) -> Result<P, Error> {
        let val = self.read_state_string(|s| &s.software_version_string);
        builder.set(&val)
    }

    fn handle_keep_active(
        &self,
        _ctx: impl InvokeContext,
        _request: KeepActiveRequest<'_>,
    ) -> Result<(), Error> {
        Err(rs_matter::error::ErrorCode::CommandNotFound.into())
    }
}

// ── Data Model handler composition ──

fn data_model<'a>(
    mut rand: impl RngCore + Copy,
    av_handlers: &'static [&'static CamAvHandler],
    webrtc_handlers: &'static [&'static WebRtcHandler],
    occupancy_handlers: &'a [OccupancyHandler],
    camera_slots: &'a [Arc<RwLock<CameraSlot>>],
) -> impl DataModel + 'a {
    let chain = EthSysHandlerBuilder::new()
        .netif_diag(&UnixNetifs)
        .build(rand)
        .chain(
            EpClMatcher::new(Some(AGGREGATOR_EP), Some(desc::DescHandler::CLUSTER.id)),
            Async(desc::DescHandler::new_aggregator(Dataver::new_rand(&mut rand)).adapt()),
        );

    macro_rules! chain_camera_base {
        ($chain:expr, $rand:expr, $av:expr, $webrtc:expr, $slots:expr, $ep:expr, $idx:expr) => {
            $chain
                .chain(
                    EpClMatcher::new(Some($ep), Some(desc::DescHandler::CLUSTER.id)),
                    Async(desc::DescHandler::new(Dataver::new_rand(&mut $rand)).adapt()),
                )
                .chain(
                    EpClMatcher::new(Some($ep), Some(groups::GroupsHandler::CLUSTER.id)),
                    Async(groups::GroupsHandler::new(Dataver::new_rand(&mut $rand)).adapt()),
                )
                .chain(
                    EpClMatcher::new(Some($ep), Some(BridgedHandler::CLUSTER.id)),
                    Async(
                        BridgedHandler::new(
                            Dataver::new_rand(&mut $rand),
                            Arc::clone(&$slots[$idx]),
                        )
                        .adapt(),
                    ),
                )
                .chain(
                    EpClMatcher::new(Some($ep), Some(CamAvHandler::CLUSTER.id)),
                    cam_av_stream::HandlerAsyncAdaptor($av[$idx]),
                )
                .chain(
                    EpClMatcher::new(Some($ep), Some(WebRtcHandler::CLUSTER.id)),
                    webrtc_prov::HandlerAsyncAdaptor($webrtc[$idx]),
                )
        };
    }

    macro_rules! chain_camera_ep_with_occupancy {
        ($chain:expr, $rand:expr, $av:expr, $webrtc:expr, $occ:expr, $slots:expr, $ep:expr, $idx:expr) => {
            chain_camera_base!($chain, $rand, $av, $webrtc, $slots, $ep, $idx).chain(
                EpClMatcher::new(Some($ep), Some(OCCUPANCY_CLUSTER.id)),
                Async(&$occ[$idx]),
            )
        };
    }

    let chain = chain_camera_ep_with_occupancy!(chain, rand, av_handlers, webrtc_handlers, occupancy_handlers, camera_slots, 2, 0);
    let chain = chain_camera_ep_with_occupancy!(chain, rand, av_handlers, webrtc_handlers, occupancy_handlers, camera_slots, 3, 1);
    let chain = chain_camera_ep_with_occupancy!(chain, rand, av_handlers, webrtc_handlers, occupancy_handlers, camera_slots, 4, 2);
    let chain = chain_camera_ep_with_occupancy!(chain, rand, av_handlers, webrtc_handlers, occupancy_handlers, camera_slots, 5, 3);
    let chain = chain_camera_ep_with_occupancy!(chain, rand, av_handlers, webrtc_handlers, occupancy_handlers, camera_slots, 6, 4);
    let chain = chain_camera_ep_with_occupancy!(chain, rand, av_handlers, webrtc_handlers, occupancy_handlers, camera_slots, 7, 5);
    let chain = chain_camera_ep_with_occupancy!(chain, rand, av_handlers, webrtc_handlers, occupancy_handlers, camera_slots, 8, 6);
    let chain = chain_camera_base!(chain, rand, av_handlers, webrtc_handlers, camera_slots, 9, 7);

    (NODE, chain)
}
