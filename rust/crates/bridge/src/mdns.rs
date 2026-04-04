//! mDNS helper — uses rs-matter's built-in mDNS responder.
//! Adapted from the rs-matter examples.

use std::net::UdpSocket;

use rs_matter::dm::ChangeNotify;
use rs_matter::error::Error;
use rs_matter::transport::network::mdns::builtin::{BuiltinMdnsResponder, Host};
use rs_matter::transport::network::mdns::{
    MDNS_IPV4_BROADCAST_ADDR, MDNS_IPV6_BROADCAST_ADDR, MDNS_SOCKET_DEFAULT_BIND_ADDR,
};
use rs_matter::transport::network::{Ipv4Addr, Ipv6Addr};
use rs_matter::{crypto::Crypto, Matter};
use socket2::{Domain, Protocol, Socket, Type};

pub async fn run_mdns<C: Crypto>(
    matter: &Matter<'_>,
    crypto: C,
    notify: &dyn ChangeNotify,
) -> Result<(), Error> {
    let (ipv4_addr, ipv6_addr, interface) = initialize_network()?;

    let mut socket = Socket::new(Domain::IPV6, Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_reuse_address(true)?;
    socket.set_only_v6(false)?;
    socket.bind(&MDNS_SOCKET_DEFAULT_BIND_ADDR.into())?;
    let socket = async_io::Async::<UdpSocket>::new_nonblocking(socket.into())?;

    socket
        .get_ref()
        .join_multicast_v6(&MDNS_IPV6_BROADCAST_ADDR, interface)?;
    socket
        .get_ref()
        .join_multicast_v4(&MDNS_IPV4_BROADCAST_ADDR, &ipv4_addr)?;

    BuiltinMdnsResponder::new(matter, crypto, notify)
        .run(
            &socket,
            &socket,
            &Host {
                id: 0,
                hostname: "matter-onvif-bridge",
                ip: ipv4_addr,
                ipv6: ipv6_addr,
            },
            Some(ipv4_addr),
            Some(interface),
        )
        .await
}

fn initialize_network() -> Result<(Ipv4Addr, Ipv6Addr, u32), Error> {
    use nix::{net::if_::InterfaceFlags, sys::socket::SockaddrIn6};
    use rs_matter::error::ErrorCode;

    let interfaces = || {
        nix::ifaddrs::getifaddrs().unwrap().filter(|ia| {
            ia.flags
                .contains(InterfaceFlags::IFF_UP | InterfaceFlags::IFF_BROADCAST)
                && !ia
                    .flags
                    .intersects(InterfaceFlags::IFF_LOOPBACK | InterfaceFlags::IFF_POINTOPOINT)
        })
    };

    let (iname, ip, ipv6) = interfaces()
        .filter_map(|ia| {
            ia.address
                .and_then(|addr| addr.as_sockaddr_in6().map(SockaddrIn6::ip))
                .map(|ipv6| (ia.interface_name, ipv6))
        })
        .filter_map(|(iname, ipv6)| {
            interfaces()
                .filter(|ia2| ia2.interface_name == iname)
                .find_map(|ia2| {
                    ia2.address
                        .and_then(|addr| addr.as_sockaddr_in().map(|addr| addr.ip().into()))
                        .map(|ip: std::net::Ipv4Addr| (iname.clone(), ip, ipv6))
                })
        })
        .next()
        .ok_or_else(|| {
            log::error!("Cannot find network interface suitable for mDNS broadcasting");
            ErrorCode::StdIoError
        })?;

    log::info!("Will use network interface {iname} with {ip}/{ipv6} for mDNS");

    Ok((ip.octets().into(), ipv6.octets().into(), 0_u32))
}
