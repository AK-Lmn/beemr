//! A dedicated relay: forwards end-to-end encrypted traffic for beemr
//! devices that can't reach each other directly.

use std::time::Duration;

use libp2p::multiaddr::Protocol;
use libp2p::Multiaddr;

use crate::discovery::Dht;
use crate::node::{self, Mode, Node, NodeOptions};
use crate::profile::Profile;
use crate::Result;

pub const DEFAULT_PORT: u16 = 4545;
const ANNOUNCE_EVERY: Duration = Duration::from_secs(20 * 60);
const RETRY_EVERY: Duration = Duration::from_secs(10);

pub async fn run(profile: Profile, port: u16, public_listing: bool) -> Result<()> {
    let network = profile.network.clone();
    let node = Node::start(NodeOptions {
        mode: Mode::Relay,
        port,
        public_network: network.public,
        upnp: network.public,
        relays: Vec::new(),
        relay_for_others: false,
        key_seed: Some(profile.identity.derive_seed("relay-key")),
    })
    .await?;
    let dht = Dht::start(&network, true)?;
    eprintln!("beemr relay running on port {}.\n", node.port());

    // Give UPnP and reachability checks a moment before printing addresses.
    let mut status = node.status();
    let _ = tokio::time::timeout(
        Duration::from_secs(20),
        status.wait_for(|s| s.directly_reachable()),
    )
    .await;
    let (internet, local) = relay_addresses(&node);
    if internet.is_empty() {
        eprintln!(
            "This machine isn't reachable from the internet yet, so it can only relay\n\
             for devices on its own network. To relay for everyone, enable UPnP on the\n\
             router, forward port {} (TCP and UDP) to this machine, or run it on a server.\n",
            node.port()
        );
    } else {
        eprintln!("Devices anywhere can use this relay with:\n");
        for addr in &internet {
            eprintln!("    beemr relay use {addr}");
        }
        eprintln!();
    }
    if !local.is_empty() {
        eprintln!("Devices on this network can use it with:\n");
        for addr in &local {
            eprintln!("    beemr relay use {addr}");
        }
        eprintln!();
    }
    match (&dht, public_listing) {
        (Some(_), true) => {
            eprintln!("Listing this relay publicly so every beemr device can use it.")
        }
        _ => eprintln!("Not listed publicly (only devices you configure will use it)."),
    }
    eprintln!("Press Ctrl+C to stop.\n");

    let announce = async {
        loop {
            let announced = match (&dht, public_listing) {
                (Some(dht), true) => dht.announce_relay(node.port()).await.is_ok(),
                _ => true,
            };
            // Retry quickly until the DHT accepts the announcement.
            let wait = if announced {
                ANNOUNCE_EVERY
            } else {
                RETRY_EVERY
            };
            tokio::time::sleep(wait).await;
        }
    };
    tokio::select! {
        _ = announce => {}
        _ = tokio::signal::ctrl_c() => eprintln!("Relay stopped."),
    }
    Ok(())
}

/// Addresses other devices can reach this relay at, with its peer id:
/// (internet-reachable, local-network only).
fn relay_addresses(node: &Node) -> (Vec<Multiaddr>, Vec<Multiaddr>) {
    let status = node.status().borrow().clone();
    let with_id = |a: &Multiaddr| a.clone().with(Protocol::P2p(node.peer_id()));
    let mut internet: Vec<Multiaddr> = status
        .external
        .iter()
        .chain(status.listen.iter())
        .filter(|a| node::ip_of(a).is_some_and(|ip| node::is_global(&ip)))
        .map(with_id)
        .collect();
    let mut local: Vec<Multiaddr> = status
        .listen
        .iter()
        .filter(|a| node::ip_of(a).is_some_and(|ip| node::is_lan(&ip) && ip.is_ipv4()))
        .map(with_id)
        .collect();
    for list in [&mut internet, &mut local] {
        list.sort();
        list.dedup();
    }
    (internet, local)
}
