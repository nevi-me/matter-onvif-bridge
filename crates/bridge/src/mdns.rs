//! mDNS helper — uses the system mDNS service (avahi on Linux, Bonjour on macOS)
//! via the `zeroconf` crate. This avoids port 5353 conflicts with avahi-daemon
//! and provides proper mDNS response handling for remote controllers.

use rs_matter::error::Error;
use rs_matter::transport::network::mdns::zeroconf::ZeroconfMdns;
use rs_matter::{crypto::Crypto, Matter};

pub async fn run_mdns<C: Crypto>(
    matter: &Matter<'_>,
    _crypto: C,
) -> Result<(), Error> {
    log::info!("Starting mDNS via system service (zeroconf/avahi)");
    ZeroconfMdns::new().run(matter).await
}
