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
    let (ipv4, ipv6, interface) = match initialize_network() {
        Ok(network) => network,
        Err(error) => {
            log::warn!("mDNS is unavailable: {error}");
            return core::future::pending().await;
        }
    };

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
        .ok_or_else(|| ErrorCode::NoNetworkInterface.into())
}
