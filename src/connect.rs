//! The connection ladder: reach a device by every available path at once and
//! keep the best one.
//!
//! 1. Direct: LAN addresses, public/IPv6 addresses, UPnP-mapped ports.
//! 2. Hole punching: connect through a relay, then DCUtR upgrades to direct.
//! 3. Full relay: if hole punching fails, a beemr relay carries the data.
//!
//! Short messages may use any relay. File transfers need a direct
//! connection or a full relay, since public relays only allow ~128 KiB.

use std::collections::HashSet;
use std::time::Duration;

use libp2p::swarm::dial_opts::{DialOpts, PeerCondition};
use libp2p::swarm::ConnectionId;
use libp2p::{Multiaddr, PeerId};
use tokio::sync::{broadcast, mpsc};
use tokio::time::Instant;

use crate::discovery::{AddressRecord, Dht};
use crate::identity::DeviceId;
use crate::node::{self, Node, NodeEvent};
use crate::{Error, Result};

/// How long to give hole punching before falling back to a full relay: enough
/// for libp2p's three ~5 s attempts.
const HOLE_PUNCH_GRACE: Duration = Duration::from_secs(16);
const RESOLVE_RETRY: Duration = Duration::from_secs(3);
const RERESOLVE_EVERY: Duration = Duration::from_secs(10);
/// How long QUIC relay circuits get before TCP ones are tried too.
const TCP_CIRCUIT_DELAY: Duration = Duration::from_secs(3);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Purpose {
    /// Any connection will do (a small text message).
    Message,
    /// Needs a direct connection or a full relay (file transfer).
    Files,
}

/// How a connection was made, for the user.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Path {
    Direct,
    HolePunched,
    Relayed,
}

impl Path {
    pub fn describe(self) -> &'static str {
        match self {
            Path::Direct => "direct connection",
            Path::HolePunched => "direct connection (through the firewall via hole punching)",
            Path::Relayed => "relayed connection (end-to-end encrypted)",
        }
    }
}

/// Where to look for a device.
pub struct Target {
    /// Addresses to dial without knowing the peer id (from a ticket).
    pub lan: Vec<Multiaddr>,
    /// A DHT record to look up for the device's current addresses.
    pub record: Option<(Dht, DeviceId, Vec<u8>)>,
}

#[derive(Clone, Copy, Debug)]
pub struct Route {
    pub peer: PeerId,
    pub path: Path,
}

/// Connect to a target, returning the peer to open streams to. On success,
/// only connections suitable for `purpose` remain open to that peer.
pub async fn connect(
    node: &Node,
    target: Target,
    purpose: Purpose,
    timeout: Duration,
) -> Result<Route> {
    let mut target = target;
    if force_relay() {
        // Diagnostics: ignore direct addresses so the relay and hole-punching
        // path is exercised even between devices on the same network.
        target.lan.clear();
    }
    let mut ladder = Ladder {
        node,
        purpose,
        events: node.events(),
        lan_dials: HashSet::new(),
        lan_peers: Vec::new(),
        expected: None,
        full_relay_dials: HashSet::new(),
        delayed: Vec::new(),
        delayed_at: None,
        full_relays: HashSet::new(),
        seen_relayed: false,
        hole_punch_deadline: None,
        hole_punch_failed: false,
        record_found: false,
        errors: Vec::new(),
    };
    for addr in target.lan {
        let id = node.dial(DialOpts::unknown_peer_id().address(addr).build());
        ladder.lan_dials.insert(id);
    }

    let (records_tx, mut records) = mpsc::channel::<AddressRecord>(1);
    let resolving = target.record.is_some();
    let resolver = target.record.map(|(dht, device, salt)| {
        tokio::spawn(async move {
            // Keep watching the record: relays come and go, and the device
            // republishes whenever its addresses change.
            let mut last: Option<AddressRecord> = None;
            loop {
                if let Some(record) = dht.resolve(&device, &salt).await {
                    let reachable = !record.addrs.is_empty() || !record.full_relays.is_empty();
                    if reachable && last.as_ref() != Some(&record) {
                        last = Some(record.clone());
                        if records_tx.send(record).await.is_err() {
                            return;
                        }
                    }
                }
                let wait = if last.is_some() {
                    RERESOLVE_EVERY
                } else {
                    RESOLVE_RETRY
                };
                tokio::time::sleep(wait).await;
            }
        })
    });
    let _abort = AbortOnDrop(resolver);

    let deadline = tokio::time::sleep(timeout);
    tokio::pin!(deadline);
    loop {
        if let Some(route) = ladder.evaluate().await? {
            return Ok(route);
        }
        if !resolving && ladder.lan_dials.is_empty() && ladder.lan_peers.is_empty() {
            return Err(ladder.diagnose());
        }
        let punch_deadline = ladder.hole_punch_deadline;
        let delayed_at = ladder.delayed_at;
        tokio::select! {
            event = ladder.events.recv() => match event {
                Ok(event) => ladder.on_event(event),
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => bail!("the network stopped unexpectedly"),
            },
            Some(record) = records.recv() => ladder.on_record(record),
            _ = sleep_until_opt(delayed_at) => ladder.dial_delayed(),
            _ = sleep_until_opt(punch_deadline) => {
                ladder.hole_punch_failed = true;
                ladder.hole_punch_deadline = None;
            }
            _ = &mut deadline => return Err(ladder.diagnose()),
        }
    }
}

/// Whether a circuit address reaches its relay over QUIC.
fn circuit_uses_quic(addr: &Multiaddr) -> bool {
    addr.iter()
        .take_while(|p| *p != libp2p::multiaddr::Protocol::P2pCircuit)
        .any(|p| p == libp2p::multiaddr::Protocol::QuicV1)
}

/// `BEEMR_FORCE_RELAY=1`: a diagnostic switch that skips direct addresses.
fn force_relay() -> bool {
    std::env::var("BEEMR_FORCE_RELAY").is_ok_and(|v| v == "1")
}

struct AbortOnDrop(Option<tokio::task::JoinHandle<()>>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        if let Some(handle) = &self.0 {
            handle.abort();
        }
    }
}

async fn sleep_until_opt(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

struct Ladder<'a> {
    node: &'a Node,
    purpose: Purpose,
    events: broadcast::Receiver<NodeEvent>,
    lan_dials: HashSet<ConnectionId>,
    /// Peers reached through ticket LAN addresses (identity checked later).
    lan_peers: Vec<PeerId>,
    /// The peer named in the device's DHT record.
    expected: Option<PeerId>,
    full_relay_dials: HashSet<ConnectionId>,
    /// TCP relay circuits to try if QUIC ones haven't connected: (peer, addresses, full relay).
    delayed: Vec<(PeerId, Vec<Multiaddr>, bool)>,
    delayed_at: Option<Instant>,
    /// Relays the sender says carry full transfers.
    full_relays: HashSet<PeerId>,
    seen_relayed: bool,
    hole_punch_deadline: Option<Instant>,
    hole_punch_failed: bool,
    record_found: bool,
    errors: Vec<String>,
}

impl Ladder<'_> {
    fn candidates(&self) -> Vec<PeerId> {
        match self.expected {
            Some(peer) => vec![peer],
            None => self.lan_peers.clone(),
        }
    }

    fn on_event(&mut self, event: NodeEvent) {
        match event {
            NodeEvent::Connected { conn, peer, relay } => {
                if self.lan_dials.remove(&conn) && !self.lan_peers.contains(&peer) {
                    self.lan_peers.push(peer);
                }
                self.full_relay_dials.remove(&conn);
                if relay.is_some() && self.candidates().contains(&peer) {
                    self.seen_relayed = true;
                    if self.purpose == Purpose::Files && self.hole_punch_deadline.is_none() {
                        self.hole_punch_deadline = Some(Instant::now() + HOLE_PUNCH_GRACE);
                    }
                }
            }
            NodeEvent::DialFailed { conn, error } => {
                self.lan_dials.remove(&conn);
                self.full_relay_dials.remove(&conn);
                self.errors.push(error);
            }
            NodeEvent::HolePunch {
                peer,
                success: false,
            } if self.candidates().contains(&peer) => {
                self.hole_punch_failed = true;
            }
            _ => {}
        }
    }

    fn on_record(&mut self, mut record: AddressRecord) {
        if force_relay() {
            record.addrs.retain(node::is_relayed);
        }
        self.record_found = true;
        self.expected = Some(record.peer);
        let peer = record.peer;

        // Relay circuits over QUIC go first: our connection to the relay then
        // runs over QUIC, so we learn a QUIC address to hole-punch with (QUIC
        // punches through NATs far more reliably than TCP). TCP circuits are
        // a fallback for networks that block UDP, dialled a moment later.
        let (now, later): (Vec<Multiaddr>, Vec<Multiaddr>) = record
            .addrs
            .into_iter()
            .partition(|a| !node::is_relayed(a) || circuit_uses_quic(a));
        self.dial_peer(peer, now, false);
        self.delayed.push((peer, later, false));

        for addr in &record.full_relays {
            if let Some(relay) = node::relay_of(addr) {
                self.full_relays.insert(relay);
            }
        }
        let (now, later): (Vec<Multiaddr>, Vec<Multiaddr>) =
            record.full_relays.into_iter().partition(circuit_uses_quic);
        // Full relays are dialled in parallel so the fallback is ready if needed.
        for addr in now {
            self.dial_peer(peer, vec![addr], true);
        }
        self.delayed.push((peer, later, true));
        if self.delayed.iter().any(|(_, addrs, _)| !addrs.is_empty()) {
            self.delayed_at = Some(Instant::now() + TCP_CIRCUIT_DELAY);
        }
    }

    fn dial_peer(&mut self, peer: PeerId, addrs: Vec<Multiaddr>, full_relay: bool) {
        if addrs.is_empty() {
            return;
        }
        let id = self.node.dial(
            DialOpts::peer_id(peer)
                .addresses(addrs)
                .condition(PeerCondition::Always)
                .build(),
        );
        if full_relay {
            self.full_relay_dials.insert(id);
        }
    }

    /// Dial the TCP circuits held back by [`Self::on_record`], unless a
    /// connection to that peer already exists.
    fn dial_delayed(&mut self) {
        self.delayed_at = None;
        for (peer, addrs, full) in std::mem::take(&mut self.delayed) {
            if self.node.connections_to(&peer).is_empty() {
                if full {
                    for addr in addrs {
                        self.dial_peer(peer, vec![addr], true);
                    }
                } else {
                    self.dial_peer(peer, addrs, false);
                }
            }
        }
    }

    fn is_full_relay(&self, relay: &PeerId) -> bool {
        self.full_relays.contains(relay) || self.node.relay_is_full(relay)
    }

    /// Decide whether we have a good enough connection yet.
    async fn evaluate(&mut self) -> Result<Option<Route>> {
        for peer in self.candidates() {
            let conns = self.node.connections_to(&peer);
            if conns.is_empty() {
                continue;
            }
            let direct: Vec<ConnectionId> = conns
                .iter()
                .filter(|(_, relay)| relay.is_none())
                .map(|(id, _)| *id)
                .collect();
            if !direct.is_empty() {
                self.keep_only(&peer, &direct).await;
                let path = if self.seen_relayed {
                    Path::HolePunched
                } else {
                    Path::Direct
                };
                return Ok(Some(Route { peer, path }));
            }
            if self.purpose == Purpose::Message {
                return Ok(Some(Route {
                    peer,
                    path: Path::Relayed,
                }));
            }
            if self.hole_punch_failed {
                let full: Vec<ConnectionId> = conns
                    .iter()
                    .filter(|(_, relay)| relay.is_some_and(|r| self.is_full_relay(&r)))
                    .map(|(id, _)| *id)
                    .collect();
                if !full.is_empty() {
                    self.keep_only(&peer, &full).await;
                    return Ok(Some(Route {
                        peer,
                        path: Path::Relayed,
                    }));
                }
                if self.full_relay_dials.is_empty() {
                    return Err(no_route_error());
                }
            }
        }
        Ok(None)
    }

    /// Close every other connection to `peer` and wait until they are gone,
    /// so new streams can only use the ones we chose.
    async fn keep_only(&mut self, peer: &PeerId, keep: &[ConnectionId]) {
        let closing: Vec<ConnectionId> = self
            .node
            .connections_to(peer)
            .into_iter()
            .map(|(id, _)| id)
            .filter(|id| !keep.contains(id))
            .collect();
        for id in &closing {
            self.node.close(*id);
        }
        let wait = async {
            while self
                .node
                .connections_to(peer)
                .iter()
                .any(|(id, _)| closing.contains(id))
            {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        };
        let _ = tokio::time::timeout(Duration::from_secs(2), wait).await;
    }

    fn diagnose(&self) -> Error {
        if self.seen_relayed && self.purpose == Purpose::Files {
            return no_route_error();
        }
        if self.expected.is_none() && self.lan_peers.is_empty() {
            return Error::new(
                "couldn't find the other device on the network.\n\
                 Make sure beemr is still running there and that both devices are online.",
            );
        }
        let mut message =
            String::from("found the other device but couldn't connect to any of its addresses.");
        if let Some(error) = self.errors.last() {
            message.push_str(&format!("\nLast error: {error}"));
        }
        Error::new(message)
    }
}

fn no_route_error() -> Error {
    Error::new(
        "both devices are behind firewalls that block direct connections, and no\n\
         beemr relay is available to carry the transfer.\n\n\
         Fix: run `beemr relay` on any computer that's reachable from the internet\n\
         (for example a home computer whose router supports UPnP, or a cheap cloud\n\
         server). Every beemr device finds it automatically.",
    )
}
