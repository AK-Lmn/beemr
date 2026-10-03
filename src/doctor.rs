//! `beemr doctor`: explain how reachable this device is, and why.

use std::time::Duration;

use crate::discovery::Dht;
use crate::node::{self, Mode, Node, NodeOptions, UpnpState};
use crate::profile::Profile;
use crate::Result;

const OBSERVE_FOR: Duration = Duration::from_secs(30);

pub async fn run(profile: Profile) -> Result<()> {
    let network = profile.network.clone();
    eprintln!(
        "Checking this device's connectivity (about {} seconds)…\n",
        OBSERVE_FOR.as_secs()
    );
    let node = Node::start(NodeOptions {
        mode: Mode::Share,
        port: 0,
        public_network: network.public,
        upnp: network.public,
        relays: profile.config.relays()?,
        key_seed: None,
    })
    .await?;
    let dht = Dht::start(&network, false)?;
    if let Some(dht) = &dht {
        tokio::spawn(crate::share::find_full_relays(node.clone(), dht.clone()));
    }
    tokio::time::sleep(OBSERVE_FOR).await;

    let status = node.status().borrow().clone();
    let public_ip = match &dht {
        Some(dht) => dht.public_address().await,
        None => None,
    };
    let mut ips: Vec<_> = status.listen.iter().filter_map(node::ip_of).collect();
    ips.sort();
    ips.dedup();
    let lan: Vec<_> = ips.iter().filter(|ip| node::is_lan(ip)).collect();
    let ipv6 = ips.iter().any(|ip| ip.is_ipv6() && node::is_global(ip));
    let limited = status.relayed.iter().filter(|r| !r.full).count();
    let full = status.relayed.iter().filter(|r| r.full).count();
    let mark = |ok: bool| if ok { "✓" } else { "✗" };

    println!(
        "Device:            \"{}\" ({})",
        profile.name,
        profile.identity.id()
    );
    println!(
        "{} Local network     {}",
        mark(!lan.is_empty()),
        if lan.is_empty() {
            "no LAN address".to_string()
        } else {
            lan.iter()
                .map(|ip| ip.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        }
    );
    println!(
        "{} Public address    {}",
        mark(public_ip.is_some()),
        public_ip.map_or("unknown (DHT unreachable?)".to_string(), |a| a
            .ip()
            .to_string())
    );
    println!(
        "{} IPv6              {}",
        mark(ipv6),
        if ipv6 { "available" } else { "not available" }
    );
    println!(
        "{} Router (UPnP)     {}",
        mark(status.upnp == UpnpState::Mapped),
        match status.upnp {
            UpnpState::Mapped => "opened a port automatically",
            UpnpState::NotFound => "no UPnP router found",
            UpnpState::NotRoutable => "router is itself behind another NAT",
            UpnpState::Disabled => "disabled",
            UpnpState::Pending => "no answer yet",
        }
    );
    println!(
        "{} Direct reachability {}",
        mark(status.directly_reachable()),
        if status.directly_reachable() {
            "others can connect straight to this device"
        } else {
            "behind a firewall/NAT (normal for home and mobile networks)"
        }
    );
    println!(
        "{} Hole punching     {}",
        mark(limited > 0),
        if limited > 0 {
            format!("ready via {limited} public relay(s)")
        } else {
            "no public relay reserved yet".to_string()
        }
    );
    println!(
        "{} beemr relays   {}",
        mark(full > 0),
        if full > 0 {
            format!("{full} available as a fallback")
        } else {
            "none found (only needed when hole punching fails)".to_string()
        }
    );
    println!();
    let verdict = if status.directly_reachable() {
        "Excellent: other devices can always reach this one."
    } else if limited > 0 && full > 0 {
        "Good: connections work through hole punching, with a relay as backup."
    } else if limited > 0 {
        "Good: connections work through hole punching on most networks."
    } else {
        "Limited: only devices on your local network can reach this one right now."
    };
    println!("{verdict}");
    Ok(())
}
