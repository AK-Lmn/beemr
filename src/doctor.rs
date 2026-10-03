//! `beemr doctor`: explain how reachable this device is, and why.

use std::time::Duration;

use crate::discovery::Dht;
use crate::nat::NatKind;
use crate::node::{self, Mode, Node, NodeOptions, PortMapState, UpnpState};
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
        relay_for_others: false,
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
    let line = |ok: bool, label: &str, value: String| {
        println!("{} {label:<17} {value}", if ok { "✓" } else { "✗" });
    };

    println!("Device: \"{}\" ({})\n", profile.name, profile.identity.id());
    line(
        !lan.is_empty(),
        "Local network",
        if lan.is_empty() {
            "no LAN address".to_string()
        } else {
            lan.iter()
                .map(|ip| ip.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        },
    );
    line(
        public_ip.is_some(),
        "Public address",
        public_ip.map_or("unknown (DHT unreachable?)".to_string(), |a| {
            a.ip().to_string()
        }),
    );
    line(
        ipv6,
        "IPv6",
        if ipv6 { "available" } else { "not available" }.to_string(),
    );
    let mapped = status.mapping_method();
    line(
        mapped.is_some(),
        "Router port",
        match (mapped, status.upnp, status.port_map) {
            (Some(how), _, _) => format!("opened automatically ({how})"),
            (None, UpnpState::NotRoutable, _) | (None, _, PortMapState::NotRoutable) => {
                "router is itself behind another NAT (carrier-grade NAT)".to_string()
            }
            (None, UpnpState::Disabled, _) => "port mapping disabled".to_string(),
            _ => "router didn't open a port (no UPnP, PCP or NAT-PMP)".to_string(),
        },
    );
    line(
        matches!(status.nat, NatKind::Reachable | NatKind::Cone),
        "Network type",
        status.nat.describe().to_string(),
    );
    line(
        status.directly_reachable(),
        "Direct reach",
        if status.directly_reachable() {
            "others can connect straight to this device"
        } else {
            "behind a firewall/NAT (normal for home and mobile networks)"
        }
        .to_string(),
    );
    line(
        limited > 0,
        "Hole punching",
        if limited > 0 {
            format!("ready via {limited} public relay(s)")
        } else {
            "no public relay reserved yet".to_string()
        },
    );
    line(
        full > 0,
        "beemr relays",
        if full > 0 {
            format!("{full} available as a fallback")
        } else {
            "none found (only needed when hole punching fails)".to_string()
        },
    );
    // Mirrors when `beemr share` prepares Tor (see share.rs).
    let needs_tor = !status.directly_reachable()
        && full == 0
        && matches!(status.nat, NatKind::Symmetric | NatKind::Unknown);
    println!(
        "  {:<17} {}",
        "Tor fallback",
        if needs_tor {
            "would be prepared when sharing (this network is hard to reach)"
        } else {
            "not needed on this network"
        }
    );
    println!();
    let verdict = if status.directly_reachable() {
        "Excellent: other devices can always reach this one."
    } else if status.nat == NatKind::Symmetric && full == 0 {
        "Fair: strict NAT. Port prediction works when the other side isn't strict too; otherwise a relay or Tor is used."
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
