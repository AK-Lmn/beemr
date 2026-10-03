//! Port mapping through PCP (RFC 6887) and NAT-PMP (RFC 6886).
//!
//! Many routers that don't speak UPnP (including many ISP routers and Apple
//! routers) support these instead. A mapped port makes this device directly
//! reachable, the fastest path for everyone. Mappings are short-lived and
//! renewed while beemr runs, so nothing stays open after it exits.

use std::net::{IpAddr, Ipv4Addr, UdpSocket};
use std::num::NonZeroU16;
use std::time::Duration;

use crab_nat::{
    natpmp, InternetProtocol, PortMapping, PortMappingOptions, PortMappingType, TimeoutConfig,
};
use libp2p::multiaddr::Protocol;
use libp2p::Multiaddr;

use crate::node::{self, Node, PortMapState};

/// Short lifetime, renewed while running: a crash leaves nothing open for long.
const LIFETIME_SECS: u32 = 600;
const NATPMP_PORT: u16 = 5351;

/// Map `port` for QUIC (UDP) and TCP, and keep the mapping alive. Runs until
/// the process exits. `public` is false in isolated test networks, where the
/// router's private "external" address is accepted.
pub async fn run(node: Node, port: u16, public: bool) {
    let Some(port) = NonZeroU16::new(port) else {
        return;
    };
    let Some((gateway, client)) = gateway_and_client() else {
        node.set_port_map(PortMapState::Unavailable);
        return;
    };
    let options = PortMappingOptions {
        external_port: Some(port),
        lifetime_seconds: Some(LIFETIME_SECS),
        timeout_config: Some(TimeoutConfig {
            initial_timeout: Duration::from_millis(250),
            max_retries: 2,
            max_retry_timeout: Some(Duration::from_secs(1)),
        }),
    };

    let mut mappings = Vec::new();
    for protocol in [InternetProtocol::Udp, InternetProtocol::Tcp] {
        match PortMapping::new(gateway.into(), client.into(), protocol, port, options).await {
            Ok(mapping) => mappings.push(mapping),
            Err(e) => tracing::debug!(?protocol, error = %e, "port mapping failed"),
        }
    }
    let Some(first) = mappings.first() else {
        node.set_port_map(PortMapState::Unavailable);
        return;
    };

    let (method, external_ip) = match first.mapping_type() {
        PortMappingType::Pcp { external_ip, .. } => ("PCP", Some(external_ip)),
        PortMappingType::NatPmp => (
            "NAT-PMP",
            natpmp::external_address(gateway.into(), None)
                .await
                .ok()
                .map(IpAddr::V4),
        ),
    };
    match external_ip {
        Some(ip) if node::is_global(&ip) || !public => {
            for mapping in &mappings {
                node.add_external(external_addr(ip, mapping));
            }
            node.set_port_map(PortMapState::Mapped(method));
        }
        _ => {
            // The router's own address is private (carrier-grade NAT), so the
            // mapping doesn't make us reachable.
            node.set_port_map(PortMapState::NotRoutable);
            return;
        }
    }

    loop {
        tokio::time::sleep(Duration::from_secs(u64::from(LIFETIME_SECS / 2))).await;
        for mapping in &mut mappings {
            if let Err(e) = mapping.renew().await {
                tracing::debug!(error = %e, "port mapping renewal failed");
            }
        }
    }
}

/// The default gateway (IPv4) and our address on its network.
fn gateway_and_client() -> Option<(Ipv4Addr, Ipv4Addr)> {
    let gateway = *netdev::get_default_gateway().ok()?.ipv4.first()?;
    // Connecting a UDP socket sends nothing; it reveals our source address.
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    socket.connect((gateway, NATPMP_PORT)).ok()?;
    match socket.local_addr().ok()?.ip() {
        IpAddr::V4(client) => Some((gateway, client)),
        IpAddr::V6(_) => None,
    }
}

fn external_addr(ip: IpAddr, mapping: &PortMapping) -> Multiaddr {
    let base = Multiaddr::empty().with(match ip {
        IpAddr::V4(ip) => Protocol::Ip4(ip),
        IpAddr::V6(ip) => Protocol::Ip6(ip),
    });
    let port = mapping.external_port().get();
    match mapping.protocol() {
        InternetProtocol::Udp => base.with(Protocol::Udp(port)).with(Protocol::QuicV1),
        InternetProtocol::Tcp => base.with(Protocol::Tcp(port)),
    }
}
