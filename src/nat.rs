//! Classifying this device's network, to choose the best way to connect.
//!
//! The QUIC socket is the same for every connection, so the external port
//! other peers see for it reveals the NAT's behaviour: the same port for
//! everyone means hole punching works ("cone" NAT); a different port per
//! peer means a strict ("symmetric") NAT that needs port prediction or a relay.

use std::collections::HashMap;
use std::net::SocketAddr;

use libp2p::PeerId;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NatKind {
    /// Not enough information yet.
    #[default]
    Unknown,
    /// Peers on the internet can connect directly (UPnP, PCP, public IP…).
    Reachable,
    /// Behind a NAT that keeps the same external port: hole punching works.
    Cone,
    /// Behind a NAT that picks a new external port per destination.
    Symmetric,
}

impl NatKind {
    pub fn describe(self) -> &'static str {
        match self {
            NatKind::Unknown => "not determined yet",
            NatKind::Reachable => "directly reachable from the internet",
            NatKind::Cone => "behind a NAT that allows hole punching",
            NatKind::Symmetric => "behind a strict (symmetric) NAT",
        }
    }
}

/// Classify from whether a public address is confirmed reachable, and the
/// external QUIC address each peer reported seeing.
pub fn classify(directly_reachable: bool, observed: &HashMap<PeerId, SocketAddr>) -> NatKind {
    if directly_reachable {
        return NatKind::Reachable;
    }
    let mut ports: Vec<u16> = observed.values().map(SocketAddr::port).collect();
    if ports.len() < 2 {
        return NatKind::Unknown;
    }
    ports.sort_unstable();
    ports.dedup();
    if ports.len() == 1 {
        NatKind::Cone
    } else {
        NatKind::Symmetric
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seen(entries: &[&str]) -> HashMap<PeerId, SocketAddr> {
        entries
            .iter()
            .map(|a| (PeerId::random(), a.parse().unwrap()))
            .collect()
    }

    #[test]
    fn classifies_networks() {
        assert_eq!(classify(true, &seen(&[])), NatKind::Reachable);
        assert_eq!(classify(false, &seen(&["1.2.3.4:5000"])), NatKind::Unknown);
        assert_eq!(
            classify(
                false,
                &seen(&["1.2.3.4:5000", "1.2.3.4:5000", "1.2.3.4:5000"])
            ),
            NatKind::Cone
        );
        assert_eq!(
            classify(false, &seen(&["1.2.3.4:5000", "1.2.3.4:61234"])),
            NatKind::Symmetric
        );
    }
}
