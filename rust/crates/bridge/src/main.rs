mod config;
mod mdns;
mod onvif_bridge;

use core::pin::pin;
use std::net::UdpSocket;
use std::sync::Arc;
use std::sync::RwLock;

use embassy_futures::select::{select, select4};
use rand::RngCore;

use rs_matter::crypto::{default_crypto, Crypto};
use rs_matter::dm::clusters::desc::{self, ClusterHandler as _};
use rs_matter::dm::clusters::groups::{self, ClusterHandler as _};
use rs_matter::dm::clusters::net_comm::NetworkType;
use rs_matter::dm::devices::test::{DAC_PRIVKEY, TEST_DEV_ATT, TEST_DEV_COMM};
use rs_matter::dm::devices::{DEV_TYPE_AGGREGATOR, DEV_TYPE_BRIDGED_NODE};
use rs_matter::dm::endpoints;
use rs_matter::dm::events::DefaultEvents;
use rs_matter::dm::networks::unix::UnixNetifs;
use rs_matter::dm::subscriptions::DefaultSubscriptions;
use rs_matter::dm::DeviceType;
use rs_matter::dm::{
    Async, AsyncHandler, AsyncMetadata, DataModel, Dataver, EmptyHandler, Endpoint, EpClMatcher,
    Node,
};
use rs_matter::error::Error;
use rs_matter::pairing::qr::QrTextType;
use rs_matter::pairing::DiscoveryCapabilities;
use rs_matter::persist::{Psm, NO_NETWORKS};
use rs_matter::respond::DefaultResponder;
use rs_matter::sc::pase::MAX_COMM_WINDOW_TIMEOUT_SECS;
use rs_matter::transport::MATTER_SOCKET_BIND_ADDR;
use rs_matter::utils::select::Coalesce;
use rs_matter::utils::storage::pooled::PooledBuffers;
use rs_matter::{clusters, devices, Matter};

use matter_camera::cluster_av_stream::{AvStreamHandler, AV_STREAM_CLUSTER};
use matter_camera::cluster_webrtc::{WebRtcHandler, WEBRTC_CLUSTER};
use matter_camera::types::CameraEndpointState;

pub use rs_matter::dm::clusters::decl::bridged_device_basic_information::{
    self, ClusterHandler as _, KeepActiveRequest,
};
use rs_matter::dm::{InvokeContext, ReadContext};
use rs_matter::tlv::{TLVBuilderParent, Utf8StrBuilder};
use rs_matter::with;

/// Camera device type (Matter 1.5 camera device)
/// Using a placeholder device type ID — the actual camera device type
/// will be finalized in the Matter 1.5 spec.
const DEV_TYPE_CAMERA: DeviceType = DeviceType {
    dtype: 0x0101, // Placeholder — will update when camera device type is ratified
    drev: 1,
};

/// Maximum number of camera endpoints we pre-allocate.
const MAX_CAMERAS: usize = 16;

/// Endpoint IDs: 0 = root, 1 = aggregator, 2..17 = camera slots
const AGGREGATOR_EP: u16 = 1;
const CAMERA_EP_START: u16 = 2;

fn main() -> Result<(), Error> {
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

    // Device details — using test attestation for now
    let dev_det = rs_matter::dm::devices::test::TEST_DEV_DET;
    let matter = Matter::new_default(&dev_det, TEST_DEV_COMM, &TEST_DEV_ATT, cfg.matter.port);
    matter.initialize_transport_buffers()?;

    let buffers = PooledBuffers::<10, _>::new(0);
    let subscriptions = DefaultSubscriptions::new();
    let crypto = default_crypto(rand::thread_rng(), DAC_PRIVKEY);
    let mut rand = crypto.rand()?;
    let events = DefaultEvents::new_default();

    // Create shared state for each camera endpoint
    let camera_states: Vec<Arc<RwLock<CameraEndpointState>>> = (0..MAX_CAMERAS)
        .map(|_| Arc::new(RwLock::new(CameraEndpointState::default())))
        .collect();

    // Create handlers for camera endpoints
    let av_handlers: Vec<AvStreamHandler> = camera_states
        .iter()
        .map(|s| AvStreamHandler::new(Dataver::new_rand(&mut rand), Arc::clone(s)))
        .collect();
    let mut webrtc_handlers: Vec<WebRtcHandler> = camera_states
        .iter()
        .map(|s| WebRtcHandler::new(Dataver::new_rand(&mut rand), Arc::clone(s)))
        .collect();

    // Start ONVIF + go2rtc bridge on a separate tokio thread
    let registry = onvif_client::registry::CameraRegistry::new(64);
    let media_bridge = onvif_bridge::start_onvif_bridge(&cfg, &camera_states, registry);

    // Wire SDP exchange callbacks into WebRTC handlers BEFORE building the data model.
    // Each handler gets a closure that looks up its stream name and calls go2rtc.
    for (i, handler) in webrtc_handlers.iter_mut().enumerate() {
        let api = media_bridge.api.clone();
        let stream_names = media_bridge.stream_names.clone();

        handler.set_sdp_exchange(Box::new(move |sdp_offer: &str| {
            let stream_name = {
                let names = stream_names.read().map_err(|e| format!("Lock error: {e}"))?;
                names.get(&i).cloned().ok_or_else(|| {
                    format!("No stream registered for endpoint slot {i}")
                })?
            };

            // Run the async SDP exchange on a one-shot tokio runtime
            // (we're on the Matter executor thread, not a tokio context)
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| format!("Failed to create tokio runtime: {e}"))?;

            let api = api.clone();
            rt.block_on(async {
                let session = media::webrtc_session::WebRtcSession::new(stream_name, api);
                let result = session.negotiate(sdp_offer).await?;
                Ok((result.sdp_answer, result.ice_candidates))
            })
        }));
    }

    // Build the data model handler (borrows handlers immutably from here on)
    let dm = DataModel::new(
        &matter,
        &crypto,
        &buffers,
        &subscriptions,
        Some(&events),
        dm_handler(
            rand,
            &av_handlers,
            &webrtc_handlers,
        ),
    );

    let responder = DefaultResponder::new(&dm);
    let mut respond = pin!(responder.run::<4, 4>());
    let mut dm_job = pin!(dm.run());

    let socket = async_io::Async::<UdpSocket>::bind(MATTER_SOCKET_BIND_ADDR)?;

    let mut mdns = pin!(mdns::run_mdns(&matter, &crypto, dm.change_notify()));
    let mut transport = pin!(matter.run(&crypto, &socket, &socket, &socket));

    let mut psm: Psm<4096> = Psm::new();
    let path = std::path::PathBuf::from(&cfg.matter.storage_path);

    if !matter.is_commissioned() {
        log::info!("Device not commissioned. Displaying QR code...");
        matter.print_standard_qr_text(DiscoveryCapabilities::IP)?;
        matter.print_standard_qr_code(QrTextType::Unicode, DiscoveryCapabilities::IP)?;
        matter.open_basic_comm_window(MAX_COMM_WINDOW_TIMEOUT_SECS, &crypto, dm.change_notify())?;
    } else {
        log::info!("Device already commissioned.");
    }

    let mut persist = pin!(psm.run(&path, &matter, NO_NETWORKS, Some(&events)));

    let all = select4(
        &mut transport,
        &mut mdns,
        &mut persist,
        select(&mut respond, &mut dm_job).coalesce(),
    );

    futures_lite::future::block_on(all.coalesce())
}

// ── Node definition ──

/// Build the static Node metadata with pre-allocated camera endpoints.
///
/// We use a macro to generate the endpoint array at compile time since
/// `Node` requires `&'static [Endpoint]`.
macro_rules! camera_endpoints {
    ($($id:expr),* $(,)?) => {
        &[
            // Endpoint 0: Root
            endpoints::root_endpoint_with_groups(NetworkType::Ethernet),
            // Endpoint 1: Aggregator
            Endpoint {
                id: AGGREGATOR_EP,
                device_types: devices!(DEV_TYPE_AGGREGATOR),
                clusters: clusters!(desc::DescHandler::CLUSTER),
            },
            // Camera endpoints (pre-allocated)
            $(
                Endpoint {
                    id: $id,
                    device_types: devices!(DEV_TYPE_CAMERA, DEV_TYPE_BRIDGED_NODE),
                    clusters: clusters!(
                        desc::DescHandler::CLUSTER,
                        groups::GroupsHandler::CLUSTER,
                        BridgedHandler::CLUSTER,
                        AV_STREAM_CLUSTER,
                        WEBRTC_CLUSTER
                    ),
                },
            )*
        ]
    }
}

const NODE: Node<'static> = Node {
    id: 0,
    endpoints: camera_endpoints![
        2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17
    ],
};

// ── BridgedDeviceBasicInformation handler ──

#[derive(Clone, Debug)]
pub struct BridgedHandler {
    dataver: Dataver,
}

impl BridgedHandler {
    pub const fn new(dataver: Dataver) -> Self {
        Self { dataver }
    }

    pub const fn adapt(self) -> bridged_device_basic_information::HandlerAdaptor<Self> {
        bridged_device_basic_information::HandlerAdaptor(self)
    }
}

impl bridged_device_basic_information::ClusterHandler for BridgedHandler {
    const CLUSTER: rs_matter::dm::Cluster<'static> =
        bridged_device_basic_information::FULL_CLUSTER
            .with_features(0)
            .with_attrs(with!(required))
            .with_cmds(with!());

    fn dataver(&self) -> u32 {
        self.dataver.get()
    }

    fn dataver_changed(&self) {
        self.dataver.changed();
    }

    fn reachable(&self, _ctx: impl ReadContext) -> Result<bool, Error> {
        // Stub: always reachable. Phase 2 will wire this to ONVIF discovery state.
        Ok(true)
    }

    fn unique_id<P: TLVBuilderParent>(
        &self,
        _ctx: impl ReadContext,
        _builder: Utf8StrBuilder<P>,
    ) -> Result<P, Error> {
        // Optional attribute, not yet implemented
        Err(rs_matter::error::ErrorCode::AttributeNotFound.into())
    }

    fn handle_keep_active(
        &self,
        _ctx: impl InvokeContext,
        _request: KeepActiveRequest<'_>,
    ) -> Result<(), Error> {
        // Not applicable for camera bridge
        Err(rs_matter::error::ErrorCode::CommandNotFound.into())
    }
}

// ── Data Model handler composition ──

/// Compose the full data model handler chain.
///
/// This chains the root endpoint system handlers with the aggregator descriptor
/// and all 16 pre-allocated camera endpoint handlers.
fn dm_handler<'a>(
    mut rand: impl RngCore + Copy,
    av_handlers: &'a [AvStreamHandler],
    webrtc_handlers: &'a [WebRtcHandler],
) -> impl AsyncMetadata + AsyncHandler + 'a {
    // Build the camera endpoint handler chain
    let mut chain = EmptyHandler
        // Aggregator endpoint descriptor
        .chain(
            EpClMatcher::new(Some(AGGREGATOR_EP), Some(desc::DescHandler::CLUSTER.id)),
            Async(desc::DescHandler::new_aggregator(Dataver::new_rand(&mut rand)).adapt()),
        );

    // For each camera endpoint, chain in all cluster handlers
    // We do this inline with a macro since the chain type must be known at compile time.
    // However, since we're returning `impl AsyncHandler`, we can build it dynamically
    // using a loop — but rs-matter's ChainedHandler is generic, so each .chain() call
    // changes the type. We need to use a macro or manual unrolling.
    //
    // For Phase 1, we'll use a helper that chains all 16 endpoints.
    macro_rules! chain_camera_ep {
        ($chain:expr, $rand:expr, $av:expr, $webrtc:expr, $ep:expr, $idx:expr) => {
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
                    Async(BridgedHandler::new(Dataver::new_rand(&mut $rand)).adapt()),
                )
                .chain(
                    EpClMatcher::new(Some($ep), Some(AV_STREAM_CLUSTER.id)),
                    Async(&$av[$idx]),
                )
                .chain(
                    EpClMatcher::new(Some($ep), Some(WEBRTC_CLUSTER.id)),
                    Async(&$webrtc[$idx]),
                )
        };
    }

    let chain = chain_camera_ep!(chain, rand, av_handlers, webrtc_handlers, 2, 0);
    let chain = chain_camera_ep!(chain, rand, av_handlers, webrtc_handlers, 3, 1);
    let chain = chain_camera_ep!(chain, rand, av_handlers, webrtc_handlers, 4, 2);
    let chain = chain_camera_ep!(chain, rand, av_handlers, webrtc_handlers, 5, 3);
    let chain = chain_camera_ep!(chain, rand, av_handlers, webrtc_handlers, 6, 4);
    let chain = chain_camera_ep!(chain, rand, av_handlers, webrtc_handlers, 7, 5);
    let chain = chain_camera_ep!(chain, rand, av_handlers, webrtc_handlers, 8, 6);
    let chain = chain_camera_ep!(chain, rand, av_handlers, webrtc_handlers, 9, 7);
    let chain = chain_camera_ep!(chain, rand, av_handlers, webrtc_handlers, 10, 8);
    let chain = chain_camera_ep!(chain, rand, av_handlers, webrtc_handlers, 11, 9);
    let chain = chain_camera_ep!(chain, rand, av_handlers, webrtc_handlers, 12, 10);
    let chain = chain_camera_ep!(chain, rand, av_handlers, webrtc_handlers, 13, 11);
    let chain = chain_camera_ep!(chain, rand, av_handlers, webrtc_handlers, 14, 12);
    let chain = chain_camera_ep!(chain, rand, av_handlers, webrtc_handlers, 15, 13);
    let chain = chain_camera_ep!(chain, rand, av_handlers, webrtc_handlers, 16, 14);
    let chain = chain_camera_ep!(chain, rand, av_handlers, webrtc_handlers, 17, 15);

    (
        NODE,
        endpoints::with_eth(
            &(),
            &UnixNetifs,
            rand,
            endpoints::with_sys(
                &false,
                rand,
                endpoints::with_groups(rand, chain),
            ),
        ),
    )
}
