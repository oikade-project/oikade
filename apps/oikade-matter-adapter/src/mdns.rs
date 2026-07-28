// Copyright 2025-2026 Project CHIP Authors
// SPDX-License-Identifier: Apache-2.0

//! Portable mDNS transport adapted from the rs-matter v0.2.0 bridge example.

#[cfg(not(target_os = "macos"))]
use std::net::UdpSocket;

use rs_matter::Matter;
use rs_matter::crypto::Crypto;
use rs_matter::error::Error;
#[cfg(not(target_os = "macos"))]
use rs_matter::error::ErrorCode;
#[cfg(not(target_os = "macos"))]
use rs_matter::transport::network::mdns::builtin::{BuiltinMdns, Host};
#[cfg(not(target_os = "macos"))]
use rs_matter::transport::network::mdns::{
    MDNS_IPV4_BROADCAST_ADDR, MDNS_IPV6_BROADCAST_ADDR, MDNS_SOCKET_DEFAULT_BIND_ADDR,
};
#[cfg(not(target_os = "macos"))]
use rs_matter::transport::network::{Ipv4Addr, Ipv6Addr};
#[cfg(not(target_os = "macos"))]
use socket2::{Domain, Protocol, Socket, Type};

pub async fn run<C: Crypto + Copy>(matter: &Matter<'_>, _crypto: C) -> Result<(), Error> {
    loop {
        #[cfg(target_os = "macos")]
        let result = rs_matter::transport::network::mdns::astro::AstroMdns::new()
            .run(matter)
            .await;

        #[cfg(not(target_os = "macos"))]
        let result = run_builtin(matter, _crypto).await;

        if let Err(error) = result {
            log::warn!("mDNS transport unavailable; retrying: {error}");
            embassy_time::Timer::after(embassy_time::Duration::from_secs(10)).await;
        }
    }
}

#[cfg(not(target_os = "macos"))]
async fn run_builtin<C: Crypto>(matter: &Matter<'_>, crypto: C) -> Result<(), Error> {
    let (ipv4, ipv6, interface) = initialize_network()?;

    let socket = Socket::new(Domain::IPV6, Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_reuse_address(true)?;
    #[cfg(unix)]
    socket.set_reuse_port(true)?;
    socket.set_only_v6(false)?;
    socket.bind(&MDNS_SOCKET_DEFAULT_BIND_ADDR.into())?;
    let socket = async_io::Async::<UdpSocket>::new_nonblocking(socket.into())?;
    socket
        .get_ref()
        .join_multicast_v6(&MDNS_IPV6_BROADCAST_ADDR, interface)?;
    socket
        .get_ref()
        .join_multicast_v4(&MDNS_IPV4_BROADCAST_ADDR, &ipv4)?;

    BuiltinMdns::new()
        .run(
            &socket,
            &socket,
            &Host {
                hostname: "oikade-matter",
                ip: ipv4,
                ipv6,
            },
            Some(ipv4),
            Some(interface),
            matter,
            crypto,
        )
        .await
}

#[cfg(not(target_os = "macos"))]
fn initialize_network() -> Result<(Ipv4Addr, Ipv6Addr, u32), Error> {
    let interfaces = if_addrs::get_if_addrs().map_err(|_| ErrorCode::StdIoError)?;
    select_network(&interfaces).ok_or_else(|| ErrorCode::NoNetworkInterface.into())
}

#[cfg(not(target_os = "macos"))]
fn select_network(interfaces: &[if_addrs::Interface]) -> Option<(Ipv4Addr, Ipv6Addr, u32)> {
    let find = |filter: fn(std::net::Ipv6Addr) -> bool| {
        interfaces
            .iter()
            .filter(|interface| !interface.is_loopback())
            .filter_map(|interface| match interface.addr {
                if_addrs::IfAddr::V6(ref address) if filter(address.ip) => Some((
                    interface.name.clone(),
                    address.ip,
                    interface.index.unwrap_or(0),
                )),
                _ => None,
            })
            .find_map(|(name, ipv6, index)| {
                interfaces
                    .iter()
                    .filter(|other| other.name == name)
                    .find_map(|other| match other.addr {
                        if_addrs::IfAddr::V4(ref address) => Some((address.ip, ipv6, index)),
                        _ => None,
                    })
            })
    };

    find(|address| address.is_unicast_link_local())
        .or_else(|| find(|_| true))
        .or_else(|| {
            interfaces
                .iter()
                .filter(|interface| !interface.is_loopback())
                .find_map(|interface| match interface.addr {
                    if_addrs::IfAddr::V4(ref address) => Some((
                        address.ip,
                        std::net::Ipv6Addr::UNSPECIFIED,
                        interface.index.unwrap_or(0),
                    )),
                    _ => None,
                })
        })
        .map(|(ipv4, ipv6, index)| (ipv4.octets().into(), ipv6.octets().into(), index))
}

#[cfg(all(test, not(target_os = "macos")))]
mod tests {
    use std::net::{Ipv4Addr as StdIpv4Addr, Ipv6Addr as StdIpv6Addr};

    use if_addrs::{IfAddr, IfOperStatus, Ifv4Addr, Ifv6Addr, Interface};

    use super::select_network;

    fn interface(name: &str, addr: IfAddr, index: u32) -> Interface {
        Interface {
            name: name.to_owned(),
            addr,
            index: Some(index),
            oper_status: IfOperStatus::Up,
            is_p2p: false,
        }
    }

    #[test]
    fn network_selection_requires_a_non_loopback_ipv4_interface() {
        assert_eq!(select_network(&[]), None);

        let interfaces = [interface(
            "loopback",
            IfAddr::V4(Ifv4Addr {
                ip: StdIpv4Addr::LOCALHOST,
                netmask: StdIpv4Addr::new(255, 0, 0, 0),
                prefixlen: 8,
                broadcast: None,
            }),
            1,
        )];
        assert_eq!(select_network(&interfaces), None);
    }

    #[test]
    fn network_selection_pairs_ipv4_with_link_local_ipv6() {
        let ipv6 = StdIpv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 0x10);
        let interfaces = [
            interface(
                "eth0",
                IfAddr::V4(Ifv4Addr {
                    ip: StdIpv4Addr::new(192, 0, 2, 10),
                    netmask: StdIpv4Addr::new(255, 255, 255, 0),
                    prefixlen: 24,
                    broadcast: Some(StdIpv4Addr::new(192, 0, 2, 255)),
                }),
                7,
            ),
            interface(
                "eth0",
                IfAddr::V6(Ifv6Addr {
                    ip: ipv6,
                    netmask: StdIpv6Addr::new(0xffff, 0xffff, 0xffff, 0xffff, 0, 0, 0, 0),
                    prefixlen: 64,
                    broadcast: None,
                }),
                7,
            ),
        ];

        assert_eq!(
            select_network(&interfaces),
            Some(([192, 0, 2, 10].into(), ipv6.octets().into(), 7))
        );
    }
}
