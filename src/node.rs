//! The libp2p node: transports, NAT traversal and relays, driven by a background task.
//!
//! A [`Node`] is a cheap handle. The swarm itself runs in a tokio task and
//! publishes what it learns (listen addresses, reachability, relay
//! reservations, connection events) through watch and broadcast channels.

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, TcpListener, UdpSocket};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use futures::StreamExt;
use libp2p::core::transport::ListenerId;
use libp2p::core::ConnectedPoint;
use libp2p::multiaddr::Protocol;
use libp2p::swarm::behaviour::toggle::Toggle;
use libp2p::swarm::dial_opts::DialOpts;
use libp2p::swarm::{ConnectionId, NetworkBehaviour, SwarmEvent};
use libp2p::{
    autonat, dcutr, identify, kad, noise, ping, relay, tcp, upnp, yamux, Multiaddr, PeerId,
    StreamProtocol, Swarm, SwarmBuilder,
};
use tokio::sync::{broadcast, mpsc, watch};

use crate::{Error, Result};

/// Sent in libp2p Identify, so beemr nodes can recognise each other.
pub const AGENT: &str = concat!("beemr/", env!("CARGO_PKG_VERSION"));
const HOP_PROTOCOL: StreamProtocol = StreamProtocol::new("/libp2p/circuit/relay/0.2.0/hop");

/// Entry points to the public IPFS network, used only to find relays for
/// hole punching. After that, peers are reached directly.
const IPFS_BOOTSTRAP: &[&str] = &[
    "/dnsaddr/bootstrap.libp2p.io/p2p/QmNnooDu7bfjPFoTZYxMNLWUQJyrVwtbZg5gBMjTezGAJN",
    "/dnsaddr/bootstrap.libp2p.io/p2p/QmQCU2EcMqAqQPR2i9bChDtGNJchTbq5TbXJJ16u19uLTa",
    "/dnsaddr/bootstrap.libp2p.io/p2p/QmbLHAnMoJPWSCR5Zhtx6BHJX9KiKNN6tpvbUcqanj75Nb",
    "/dnsaddr/bootstrap.libp2p.io/p2p/QmcZf59bWwK5XFi76CZX8cbJ4BhTzzA3gU1ZjYZcYW3dwt",
    "/dnsaddr/va1.bootstrap.libp2p.io/p2p/12D3KooWKnDdG3iXw9eTFijk3EWSunZcFi54Zka4wmtqtt6rPxc8",
    "/ip4/104.131.131.82/tcp/4001/p2p/QmaCpDMGvV2BGHeYERUEnRQAwe3N8SzbUtfsmvsqQLuvuJ",
    "/ip4/104.131.131.82/udp/4001/quic-v1/p2p/QmaCpDMGvV2BGHeYERUEnRQAwe3N8SzbUtfsmvsqQLuvuJ",
];

/// A relay whose circuits allow at least this much data can carry transfers.
/// Public IPFS relays allow ~128 KiB: enough to coordinate hole punching only.
const FULL_RELAY_MIN_BYTES: u64 = 64 * 1024 * 1024;
const LIMITED_RELAYS_WANTED: usize = 2;
const FULL_RELAYS_WANTED: usize = 2;
const RELAY_SEARCH_INTERVAL: Duration = Duration::from_secs(20);

/// What the node is for, which decides which services it runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Downloads or sends a message: only dials out.
    Client,
    /// Serves a share: must be reachable, so it reserves relays.
    Share,
    /// The background service: reachable, and relays for others when it can.
    Daemon,
    /// A dedicated relay.
    Relay,
}

impl Mode {
    fn reserves_relays(self) -> bool {
        matches!(self, Mode::Share | Mode::Daemon)
    }

    fn serves_relay(self) -> bool {
        matches!(self, Mode::Daemon | Mode::Relay)
    }
}

#[derive(Clone, Debug)]
pub struct NodeOptions {
    pub mode: Mode,
    /// TCP and UDP port to listen on; 0 picks a free one.
    pub port: u16,
    /// Use public networks (IPFS DHT for relays, UPnP, AutoNAT). Disabled in
    /// isolated test environments.
    pub public_network: bool,
    /// Ask the router to forward a port (UPnP).
    pub upnp: bool,
    /// Relays to reserve on in addition to any discovered ones.
    pub relays: Vec<Multiaddr>,
    /// A fixed network identity (32-byte Ed25519 seed). Relays use one so
    /// their address stays valid across restarts; others use a fresh one.
    pub key_seed: Option<[u8; 32]>,
}

/// How the router answered UPnP port-mapping requests.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UpnpState {
    #[default]
    Pending,
    Disabled,
    Mapped,
    NotFound,
    NotRoutable,
}

/// An address at which this node can be reached through a relay.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelayedAddr {
    /// Full dialable address, ending in `/p2p-circuit/p2p/<this node>`.
    pub addr: Multiaddr,
    pub relay: PeerId,
    /// Whether the relay allows enough data for file transfers.
    pub full: bool,
}

/// What the node currently knows about its own reachability.
#[derive(Clone, Debug, Default)]
pub struct Status {
    /// Concrete local listen addresses (not relayed).
    pub listen: Vec<Multiaddr>,
    /// Addresses confirmed reachable from the internet (UPnP, AutoNAT).
    pub external: Vec<Multiaddr>,
    pub relayed: Vec<RelayedAddr>,
    pub upnp: UpnpState,
    pub connected_peers: usize,
}

impl Status {
    /// Whether peers on the internet can connect to us directly: an address
    /// confirmed reachable that is publicly routable (not a LAN address that
    /// a relay on the same network happened to confirm).
    pub fn directly_reachable(&self) -> bool {
        self.external
            .iter()
            .any(|a| ip_of(a).is_some_and(|ip| is_global(&ip)))
    }
}

/// Connection events, for code waiting on a particular peer.
#[derive(Clone, Debug)]
pub enum NodeEvent {
    Connected {
        conn: ConnectionId,
        peer: PeerId,
        /// The relay the connection runs through; `None` for a direct connection.
        relay: Option<PeerId>,
    },
    Disconnected {
        conn: ConnectionId,
        peer: PeerId,
    },
    DialFailed {
        conn: ConnectionId,
        error: String,
    },
    HolePunch {
        peer: PeerId,
        success: bool,
    },
}

enum Command {
    Dial(DialOpts),
    Close(ConnectionId),
    AddRelayCandidates(Vec<Multiaddr>),
}

#[derive(Clone, Copy, Debug)]
struct ConnInfo {
    peer: PeerId,
    relay: Option<PeerId>,
}

/// State shared between the swarm task and handles.
#[derive(Default)]
struct Shared {
    connections: HashMap<ConnectionId, ConnInfo>,
    /// Whether each relay we've used allows full transfers.
    relay_full: HashMap<PeerId, bool>,
}

/// Handle to a running libp2p node.
#[derive(Clone)]
pub struct Node {
    peer_id: PeerId,
    port: u16,
    control: libp2p_stream::Control,
    commands: mpsc::UnboundedSender<Command>,
    status: watch::Receiver<Status>,
    events: broadcast::Sender<NodeEvent>,
    shared: Arc<Mutex<Shared>>,
}

#[derive(NetworkBehaviour)]
struct Behaviour {
    relay_client: relay::client::Behaviour,
    relay_server: Toggle<relay::Behaviour>,
    identify: identify::Behaviour,
    ping: ping::Behaviour,
    dcutr: dcutr::Behaviour,
    kad: Toggle<kad::Behaviour<kad::store::MemoryStore>>,
    autonat_client: Toggle<autonat::v2::client::Behaviour>,
    autonat_server: Toggle<autonat::v2::server::Behaviour>,
    upnp: Toggle<upnp::tokio::Behaviour>,
    stream: libp2p_stream::Behaviour,
}

impl Node {
    /// Start a node with a fresh, per-process network identity.
    pub async fn start(options: NodeOptions) -> Result<Node> {
        let keypair = match options.key_seed {
            Some(mut seed) => libp2p::identity::Keypair::ed25519_from_bytes(&mut seed)
                .map_err(|e| Error::new(e.to_string()))?,
            None => libp2p::identity::Keypair::generate_ed25519(),
        };
        let peer_id = keypair.public().to_peer_id();
        let public = options.public_network;
        let use_upnp = options.upnp;
        let mode = options.mode;

        let mut swarm = SwarmBuilder::with_existing_identity(keypair)
            .with_tokio()
            .with_tcp(
                tcp::Config::default().nodelay(true),
                noise::Config::new,
                yamux::Config::default,
            )?
            .with_quic()
            .with_dns()?
            .with_relay_client(noise::Config::new, yamux::Config::default)?
            .with_behaviour(|key, relay_client| {
                let local = key.public().to_peer_id();
                Behaviour {
                    relay_client,
                    relay_server: mode
                        .serves_relay()
                        .then(|| relay::Behaviour::new(local, relay_server_config()))
                        .into(),
                    identify: identify::Behaviour::new(
                        identify::Config::new("ipfs/0.1.0".into(), key.public())
                            .with_agent_version(AGENT.into()),
                    ),
                    ping: ping::Behaviour::default(),
                    dcutr: dcutr::Behaviour::new(local),
                    kad: (public && mode != Mode::Client)
                        .then(|| {
                            let mut config = kad::Config::new(kad::PROTOCOL_NAME);
                            config.set_query_timeout(Duration::from_secs(30));
                            kad::Behaviour::with_config(
                                local,
                                kad::store::MemoryStore::new(local),
                                config,
                            )
                        })
                        .into(),
                    autonat_client: (public && mode != Mode::Client)
                        .then(autonat::v2::client::Behaviour::default)
                        .into(),
                    autonat_server: (public && mode.serves_relay())
                        .then(autonat::v2::server::Behaviour::default)
                        .into(),
                    upnp: use_upnp.then(upnp::tokio::Behaviour::default).into(),
                    stream: libp2p_stream::Behaviour::new(),
                }
            })
            .map_err(|e| Error::new(e.to_string()))?
            .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(60)))
            .build();

        let port = listen(&mut swarm, options.port)?;
        if let Some(kad) = swarm.behaviour_mut().kad.as_mut() {
            for addr in IPFS_BOOTSTRAP {
                let addr: Multiaddr = addr.parse().expect("bootstrap addresses are valid");
                if let Some((peer, transport)) = split_peer(&addr) {
                    kad.add_address(&peer, transport);
                }
            }
            let _ = kad.bootstrap();
        }
        for relay in &options.relays {
            reserve(&mut swarm, relay);
        }

        let control = swarm.behaviour().stream.new_control();
        let (commands, command_rx) = mpsc::unbounded_channel();
        let (status_tx, status) = watch::channel(Status {
            upnp: if use_upnp {
                UpnpState::Pending
            } else {
                UpnpState::Disabled
            },
            ..Status::default()
        });
        let (events, _) = broadcast::channel(256);
        let shared = Arc::new(Mutex::new(Shared::default()));

        let driver = Driver {
            swarm,
            mode,
            public,
            commands: command_rx,
            status: status_tx,
            events: events.clone(),
            shared: Arc::clone(&shared),
            relay_listeners: HashMap::new(),
            relay_requested: HashSet::new(),
            relayed: HashMap::new(),
            listen: Vec::new(),
            external: Vec::new(),
            upnp: if use_upnp {
                UpnpState::Pending
            } else {
                UpnpState::Disabled
            },
        };
        tokio::spawn(driver.run());

        let node = Node {
            peer_id,
            port,
            control,
            commands,
            status,
            events,
            shared,
        };
        // Wait until the listeners report their concrete addresses.
        let mut status = node.status();
        let _ = tokio::time::timeout(
            Duration::from_secs(3),
            status.wait_for(|s| !s.listen.is_empty()),
        )
        .await;
        Ok(node)
    }

    pub fn peer_id(&self) -> PeerId {
        self.peer_id
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn control(&self) -> libp2p_stream::Control {
        self.control.clone()
    }

    pub fn status(&self) -> watch::Receiver<Status> {
        self.status.clone()
    }

    pub fn events(&self) -> broadcast::Receiver<NodeEvent> {
        self.events.subscribe()
    }

    /// Start a dial; the outcome arrives as a [`NodeEvent`]. Returns its id.
    pub fn dial(&self, opts: DialOpts) -> ConnectionId {
        let id = opts.connection_id();
        let _ = self.commands.send(Command::Dial(opts));
        id
    }

    pub fn close(&self, conn: ConnectionId) {
        let _ = self.commands.send(Command::Close(conn));
    }

    /// Offer relay addresses (e.g. beemr relays found on the DHT) to reserve on.
    pub fn add_relay_candidates(&self, addrs: Vec<Multiaddr>) {
        if !addrs.is_empty() {
            let _ = self.commands.send(Command::AddRelayCandidates(addrs));
        }
    }

    /// Current connections to `peer`, with the relay each one uses.
    pub fn connections_to(&self, peer: &PeerId) -> Vec<(ConnectionId, Option<PeerId>)> {
        self.shared()
            .connections
            .iter()
            .filter(|(_, info)| info.peer == *peer)
            .map(|(id, info)| (*id, info.relay))
            .collect()
    }

    /// Whether a relay is known to carry full transfers.
    pub fn relay_is_full(&self, relay: &PeerId) -> bool {
        self.shared()
            .relay_full
            .get(relay)
            .copied()
            .unwrap_or(false)
    }

    fn shared(&self) -> MutexGuard<'_, Shared> {
        self.shared.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Local network IPs we listen on, for tickets. Loopback is included only
    /// when `include_loopback` is set (isolated test networks).
    pub fn lan_ips(&self, include_loopback: bool) -> Vec<IpAddr> {
        let mut ips: Vec<IpAddr> = self
            .status
            .borrow()
            .listen
            .iter()
            .filter_map(ip_of)
            .filter(|ip| is_lan(ip) || (include_loopback && ip.is_loopback()))
            .collect();
        ips.sort();
        ips.dedup();
        ips
    }
}

fn relay_server_config() -> relay::Config {
    relay::Config {
        max_reservations: 256,
        max_reservations_per_peer: 4,
        reservation_duration: Duration::from_secs(60 * 60),
        max_circuits: 64,
        max_circuits_per_peer: 8,
        max_circuit_duration: Duration::from_secs(4 * 60 * 60),
        max_circuit_bytes: 32 * 1024 * 1024 * 1024,
        ..relay::Config::default()
    }
}

/// Listen on TCP and QUIC, IPv4 and IPv6, all on the same port number.
fn listen(swarm: &mut Swarm<Behaviour>, requested: u16) -> Result<u16> {
    let port = if requested != 0 {
        requested
    } else {
        free_port().ok_or_else(|| Error::new("couldn't find a free network port"))?
    };
    let addrs = [
        format!("/ip4/0.0.0.0/udp/{port}/quic-v1"),
        format!("/ip4/0.0.0.0/tcp/{port}"),
        format!("/ip6/::/udp/{port}/quic-v1"),
        format!("/ip6/::/tcp/{port}"),
    ];
    let mut listening = 0;
    for addr in addrs {
        if swarm
            .listen_on(addr.parse().expect("listen addresses are valid"))
            .is_ok()
        {
            listening += 1;
        }
    }
    if listening == 0 {
        bail!("couldn't listen on port {port}");
    }
    Ok(port)
}

/// A port that is currently free for both TCP and UDP.
fn free_port() -> Option<u16> {
    (0..20).find_map(|_| {
        let tcp = TcpListener::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
        let port = tcp.local_addr().ok()?.port();
        UdpSocket::bind((Ipv4Addr::UNSPECIFIED, port)).ok()?;
        Some(port)
    })
}

/// Ask a relay for a reservation, so peers can reach us through it.
fn reserve(swarm: &mut Swarm<Behaviour>, relay: &Multiaddr) -> Option<ListenerId> {
    let circuit = relay.clone().with(Protocol::P2pCircuit);
    swarm.listen_on(circuit).ok()
}

/// Split `/…/p2p/<id>` into the peer id and the transport address.
pub fn split_peer(addr: &Multiaddr) -> Option<(PeerId, Multiaddr)> {
    let mut transport = addr.clone();
    match transport.pop()? {
        Protocol::P2p(peer) => Some((peer, transport)),
        _ => None,
    }
}

/// The relay peer in a circuit address `/…/p2p/<relay>/p2p-circuit/…`.
pub fn relay_of(addr: &Multiaddr) -> Option<PeerId> {
    let mut last_peer = None;
    for protocol in addr.iter() {
        match protocol {
            Protocol::P2p(peer) => last_peer = Some(peer),
            Protocol::P2pCircuit => return last_peer,
            _ => {}
        }
    }
    None
}

/// Whether an address uses a transport beemr speaks: `/ip*/…/tcp/…` or
/// `/ip*/…/udp/…/quic-v1`, optionally followed by `/p2p/…`. Excludes
/// WebSocket, WebTransport, WebRTC and relayed addresses.
pub fn is_plain_transport(addr: &Multiaddr) -> bool {
    let mut protocols = addr.iter();
    let ip = matches!(protocols.next(), Some(Protocol::Ip4(_) | Protocol::Ip6(_)));
    let transport = match protocols.next() {
        Some(Protocol::Tcp(_)) => true,
        Some(Protocol::Udp(_)) => matches!(protocols.next(), Some(Protocol::QuicV1)),
        _ => false,
    };
    let rest_ok = match protocols.next() {
        None => true,
        Some(Protocol::P2p(_)) => protocols.next().is_none(),
        Some(_) => false,
    };
    ip && transport && rest_ok
}

/// Whether the relay part of a circuit address (before `/p2p-circuit`) uses a
/// transport beemr can dial.
pub fn relay_transport_ok(addr: &Multiaddr) -> bool {
    let relay_part: Multiaddr = addr
        .iter()
        .take_while(|p| *p != Protocol::P2pCircuit)
        .collect();
    is_plain_transport(&relay_part)
}

pub fn is_relayed(addr: &Multiaddr) -> bool {
    addr.iter().any(|p| p == Protocol::P2pCircuit)
}

pub fn ip_of(addr: &Multiaddr) -> Option<IpAddr> {
    addr.iter().find_map(|p| match p {
        Protocol::Ip4(ip) => Some(IpAddr::V4(ip)),
        Protocol::Ip6(ip) => Some(IpAddr::V6(ip)),
        _ => None,
    })
}

/// Private LAN addresses (RFC 1918 IPv4, IPv6 unique-local).
pub fn is_lan(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => ip.is_private(),
        IpAddr::V6(ip) => (ip.segments()[0] & 0xfe00) == 0xfc00,
    }
}

/// Addresses routable on the public internet.
pub fn is_global(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, ..] = ip.octets();
            let carrier_grade_nat = a == 100 && (b & 0xc0) == 64;
            !(ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_unspecified()
                || ip.is_broadcast()
                || ip.is_documentation()
                || carrier_grade_nat)
        }
        IpAddr::V6(ip) => (ip.segments()[0] & 0xe000) == 0x2000 && !is_documentation_v6(ip),
    }
}

fn is_documentation_v6(ip: &Ipv6Addr) -> bool {
    ip.segments()[0] == 0x2001 && ip.segments()[1] == 0x0db8
}

/// Both transports for one IP and port.
pub fn addrs_for(ip: IpAddr, port: u16) -> [Multiaddr; 2] {
    let base = Multiaddr::empty().with(match ip {
        IpAddr::V4(ip) => Protocol::Ip4(ip),
        IpAddr::V6(ip) => Protocol::Ip6(ip),
    });
    [
        base.clone()
            .with(Protocol::Udp(port))
            .with(Protocol::QuicV1),
        base.with(Protocol::Tcp(port)),
    ]
}

/// The swarm task.
struct Driver {
    swarm: Swarm<Behaviour>,
    mode: Mode,
    public: bool,
    commands: mpsc::UnboundedReceiver<Command>,
    status: watch::Sender<Status>,
    events: broadcast::Sender<NodeEvent>,
    shared: Arc<Mutex<Shared>>,
    /// Relay reservations in progress or active, by listener.
    relay_listeners: HashMap<ListenerId, PeerId>,
    relay_requested: HashSet<PeerId>,
    /// Our circuit addresses, per relay reservation (a relay may have several).
    relayed: HashMap<ListenerId, Vec<RelayedAddr>>,
    listen: Vec<Multiaddr>,
    external: Vec<Multiaddr>,
    upnp: UpnpState,
}

impl Driver {
    async fn run(mut self) {
        let mut search = tokio::time::interval(RELAY_SEARCH_INTERVAL);
        loop {
            tokio::select! {
                event = self.swarm.select_next_some() => self.on_swarm_event(event),
                command = self.commands.recv() => match command {
                    Some(command) => self.on_command(command),
                    // Every handle was dropped: shut down.
                    None => return,
                },
                _ = search.tick() => self.search_relays(),
            }
        }
    }

    fn shared(&self) -> MutexGuard<'_, Shared> {
        self.shared.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn on_command(&mut self, command: Command) {
        match command {
            Command::Dial(opts) => {
                let id = opts.connection_id();
                if let Err(e) = self.swarm.dial(opts) {
                    let _ = self.events.send(NodeEvent::DialFailed {
                        conn: id,
                        error: e.to_string(),
                    });
                }
            }
            Command::Close(conn) => {
                self.swarm.close_connection(conn);
            }
            Command::AddRelayCandidates(addrs) => {
                for addr in addrs {
                    // Connect first; Identify tells us whether it's a relay.
                    let _ = self.swarm.dial(addr);
                }
            }
        }
    }

    fn reservations(&self, full: bool) -> usize {
        let shared = self.shared();
        self.relay_listeners
            .values()
            .filter(|relay| shared.relay_full.get(relay).copied().unwrap_or(false) == full)
            .count()
    }

    /// Look for more relays if we don't have enough reservations.
    fn search_relays(&mut self) {
        if !self.mode.reserves_relays() {
            return;
        }
        if self.reservations(false) < LIMITED_RELAYS_WANTED {
            if let Some(kad) = self.swarm.behaviour_mut().kad.as_mut() {
                kad.get_closest_peers(PeerId::random());
            }
        }
    }

    fn consider_relay(&mut self, peer: PeerId, info: &identify::Info) {
        if !self.mode.reserves_relays()
            || peer == *self.swarm.local_peer_id()
            || self.relay_requested.contains(&peer)
            || !info.protocols.contains(&HOP_PROTOCOL)
        {
            return;
        }
        let is_beemr = info.agent_version.starts_with("beemr/");
        let wanted = if is_beemr {
            self.reservations(true) < FULL_RELAYS_WANTED
        } else {
            self.reservations(false) < LIMITED_RELAYS_WANTED
        };
        if !wanted {
            return;
        }
        // Reserve over a public address, preferring QUIC.
        let mut candidates: Vec<&Multiaddr> = info
            .listen_addrs
            .iter()
            .filter(|a| {
                is_plain_transport(a) && ip_of(a).is_some_and(|ip| is_global(&ip) || !self.public)
            })
            .collect();
        // Prefer real network addresses over loopback, then QUIC over TCP.
        candidates.sort_by_key(|a| {
            (
                ip_of(a).is_some_and(|ip| ip.is_loopback()),
                !a.iter().any(|p| p == Protocol::QuicV1),
            )
        });
        let Some(addr) = candidates.first() else {
            return;
        };
        let relay_addr = (*addr).clone().with(Protocol::P2p(peer));
        tracing::debug!(%relay_addr, is_beemr, "requesting relay reservation");
        if let Some(listener) = reserve(&mut self.swarm, &relay_addr) {
            self.relay_requested.insert(peer);
            self.relay_listeners.insert(listener, peer);
            if is_beemr {
                // Confirmed when the reservation reports its limits.
                self.shared().relay_full.entry(peer).or_insert(true);
            }
        }
    }

    fn publish_status(&mut self) {
        let connected_peers = self.swarm.connected_peers().count();
        let shared = self.shared();
        let relayed = self
            .relayed
            .values()
            .flatten()
            .map(|r| RelayedAddr {
                full: shared.relay_full.get(&r.relay).copied().unwrap_or(false),
                ..r.clone()
            })
            .collect();
        drop(shared);
        self.status.send_replace(Status {
            listen: self.listen.clone(),
            external: self.external.clone(),
            relayed,
            upnp: self.upnp,
            connected_peers,
        });
    }

    fn on_swarm_event(&mut self, event: SwarmEvent<BehaviourEvent>) {
        match event {
            SwarmEvent::NewListenAddr {
                listener_id,
                address,
            } => {
                if is_relayed(&address) {
                    // Relays report all their addresses; keep the ones peers can dial.
                    if let (Some(relay), true) = (relay_of(&address), relay_transport_ok(&address))
                    {
                        let mut addr = address;
                        if !matches!(addr.iter().last(), Some(Protocol::P2p(_))) {
                            addr.push(Protocol::P2p(*self.swarm.local_peer_id()));
                        }
                        let entries = self.relayed.entry(listener_id).or_default();
                        if !entries.iter().any(|e| e.addr == addr) {
                            entries.push(RelayedAddr {
                                addr,
                                relay,
                                full: false,
                            });
                        }
                    }
                } else if !self.listen.contains(&address) {
                    // A dedicated relay on a public IP is reachable at its listen
                    // addresses; confirm them so the relay service is advertised
                    // right away (libp2p only offers relaying once an external
                    // address is confirmed). In isolated test networks every
                    // non-loopback address counts.
                    if self.mode == Mode::Relay
                        && ip_of(&address)
                            .is_some_and(|ip| is_global(&ip) || (!self.public && !ip.is_loopback()))
                    {
                        self.swarm.add_external_address(address.clone());
                        if !self.external.contains(&address) {
                            self.external.push(address.clone());
                        }
                    }
                    self.listen.push(address);
                }
                self.publish_status();
            }
            SwarmEvent::ExpiredListenAddr {
                listener_id,
                address,
            } => {
                self.listen.retain(|a| a != &address);
                if let Some(entries) = self.relayed.get_mut(&listener_id) {
                    entries.retain(|e| !e.addr.ends_with(&address) && e.addr != address);
                }
                self.publish_status();
            }
            SwarmEvent::ListenerClosed {
                listener_id,
                reason,
                ..
            } => {
                tracing::debug!(?listener_id, ?reason, "listener closed");
                if let Some(relay) = self.relay_listeners.remove(&listener_id) {
                    self.relay_requested.remove(&relay);
                }
                self.relayed.remove(&listener_id);
                self.publish_status();
            }
            SwarmEvent::ExternalAddrConfirmed { address } => {
                if !is_relayed(&address) && !self.external.contains(&address) {
                    self.external.push(address);
                    self.publish_status();
                }
            }
            SwarmEvent::ExternalAddrExpired { address } => {
                self.external.retain(|a| a != &address);
                self.publish_status();
            }
            SwarmEvent::ConnectionEstablished {
                peer_id,
                connection_id,
                endpoint,
                ..
            } => {
                let relay = if endpoint.is_relayed() {
                    match &endpoint {
                        ConnectedPoint::Dialer { address, .. } => relay_of(address),
                        ConnectedPoint::Listener { local_addr, .. } => relay_of(local_addr),
                    }
                } else {
                    None
                };
                self.shared().connections.insert(
                    connection_id,
                    ConnInfo {
                        peer: peer_id,
                        relay,
                    },
                );
                let _ = self.events.send(NodeEvent::Connected {
                    conn: connection_id,
                    peer: peer_id,
                    relay,
                });
                self.publish_status();
            }
            SwarmEvent::ConnectionClosed {
                peer_id,
                connection_id,
                ..
            } => {
                self.shared().connections.remove(&connection_id);
                let _ = self.events.send(NodeEvent::Disconnected {
                    conn: connection_id,
                    peer: peer_id,
                });
                self.publish_status();
            }
            SwarmEvent::OutgoingConnectionError {
                connection_id,
                error,
                ..
            } => {
                let _ = self.events.send(NodeEvent::DialFailed {
                    conn: connection_id,
                    error: error.to_string(),
                });
            }
            SwarmEvent::Behaviour(event) => self.on_behaviour_event(event),
            _ => {}
        }
    }

    fn on_behaviour_event(&mut self, event: BehaviourEvent) {
        match event {
            BehaviourEvent::Identify(identify::Event::Received { peer_id, info, .. }) => {
                tracing::debug!(
                    %peer_id,
                    agent = %info.agent_version,
                    relay = info.protocols.contains(&HOP_PROTOCOL),
                    addrs = info.listen_addrs.len(),
                    "identified peer"
                );
                if let Some(kad) = self.swarm.behaviour_mut().kad.as_mut() {
                    if info.protocols.contains(&kad::PROTOCOL_NAME) {
                        for addr in info
                            .listen_addrs
                            .iter()
                            .filter(|a| !is_relayed(a) && ip_of(a).is_some_and(|ip| is_global(&ip)))
                        {
                            kad.add_address(&peer_id, addr.clone());
                        }
                    }
                }
                self.consider_relay(peer_id, &info);
            }
            BehaviourEvent::RelayClient(relay::client::Event::ReservationReqAccepted {
                relay_peer_id,
                limit,
                ..
            })
            | BehaviourEvent::RelayClient(relay::client::Event::OutboundCircuitEstablished {
                relay_peer_id,
                limit,
            }) => {
                tracing::debug!(%relay_peer_id, ?limit, "relay circuit or reservation accepted");
                let full = limit.is_none_or(|l| {
                    l.data_in_bytes().is_none_or(|b| b >= FULL_RELAY_MIN_BYTES)
                        && l.duration()
                            .is_none_or(|d| d >= Duration::from_secs(10 * 60))
                });
                self.shared().relay_full.insert(relay_peer_id, full);
                self.publish_status();
            }
            BehaviourEvent::Dcutr(dcutr::Event {
                remote_peer_id,
                result,
            }) => {
                tracing::debug!(%remote_peer_id, ?result, "hole punching finished");
                let _ = self.events.send(NodeEvent::HolePunch {
                    peer: remote_peer_id,
                    success: result.is_ok(),
                });
            }
            BehaviourEvent::Kad(event) => {
                tracing::debug!(?event, "kad");
            }
            BehaviourEvent::Upnp(event) => {
                self.upnp = match event {
                    upnp::Event::NewExternalAddr { .. } => UpnpState::Mapped,
                    upnp::Event::GatewayNotFound => UpnpState::NotFound,
                    upnp::Event::NonRoutableGateway => UpnpState::NotRoutable,
                    upnp::Event::ExpiredExternalAddr { .. } => self.upnp,
                };
                self.publish_status();
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_relay_addresses() {
        let relay = PeerId::random();
        let target = PeerId::random();
        let addr: Multiaddr =
            format!("/ip4/1.2.3.4/udp/4001/quic-v1/p2p/{relay}/p2p-circuit/p2p/{target}")
                .parse()
                .unwrap();
        assert!(is_relayed(&addr));
        assert_eq!(relay_of(&addr), Some(relay));
        assert_eq!(split_peer(&addr).unwrap().0, target);
        let direct: Multiaddr = "/ip4/1.2.3.4/tcp/80".parse().unwrap();
        assert!(!is_relayed(&direct));
        assert_eq!(relay_of(&direct), None);
    }

    #[test]
    fn recognises_supported_transports() {
        let ok = |s: &str| is_plain_transport(&s.parse().unwrap());
        assert!(ok("/ip4/1.2.3.4/tcp/4001"));
        assert!(ok("/ip6/2a01::1/udp/4001/quic-v1"));
        assert!(ok(&format!(
            "/ip4/1.2.3.4/udp/4001/quic-v1/p2p/{}",
            PeerId::random()
        )));
        assert!(!ok("/ip4/1.2.3.4/udp/4001/quic-v1/webtransport"));
        assert!(!ok("/ip4/1.2.3.4/tcp/443/tls/ws"));
        assert!(!ok("/dns4/example.com/tcp/4001"));
        assert!(!ok("/ip4/1.2.3.4/udp/4001"));

        let relay = PeerId::random();
        let circuit = |base: &str| -> Multiaddr {
            format!("{base}/p2p/{relay}/p2p-circuit").parse().unwrap()
        };
        assert!(relay_transport_ok(&circuit(
            "/ip4/1.2.3.4/udp/4001/quic-v1"
        )));
        assert!(!relay_transport_ok(&circuit(
            "/ip4/1.2.3.4/udp/4001/quic-v1/webtransport"
        )));
    }

    #[test]
    fn classifies_ips() {
        let ip = |s: &str| s.parse::<IpAddr>().unwrap();
        assert!(is_global(&ip("8.8.8.8")));
        assert!(!is_global(&ip("192.168.1.1")));
        assert!(!is_global(&ip("100.64.0.1")));
        assert!(is_global(&ip("2a01:4f8::1")));
        assert!(!is_global(&ip("2001:db8::1")));
        assert!(!is_global(&ip("fe80::1")));
        assert!(is_lan(&ip("10.0.0.5")));
        assert!(is_lan(&ip("fd12::1")));
        assert!(!is_lan(&ip("8.8.8.8")));
    }

    #[tokio::test]
    async fn two_nodes_connect_and_stream() {
        use futures::{AsyncReadExt, AsyncWriteExt};
        let options = |mode| NodeOptions {
            mode,
            port: 0,
            public_network: false,
            upnp: false,
            relays: vec![],
            key_seed: None,
        };
        let server = Node::start(options(Mode::Share)).await.unwrap();
        let client = Node::start(options(Mode::Client)).await.unwrap();
        let mut incoming = server.control().accept(crate::proto::PROTOCOL).unwrap();

        let [quic, _] = addrs_for(IpAddr::V4(Ipv4Addr::LOCALHOST), server.port());
        let mut events = client.events();
        client.dial(
            DialOpts::peer_id(server.peer_id())
                .addresses(vec![quic])
                .build(),
        );
        loop {
            if let NodeEvent::Connected { peer, relay, .. } = events.recv().await.unwrap() {
                assert_eq!(peer, server.peer_id());
                assert_eq!(relay, None);
                break;
            }
        }
        let mut control = client.control();
        let mut stream = control
            .open_stream(server.peer_id(), crate::proto::PROTOCOL)
            .await
            .unwrap();
        stream.write_all(b"ping").await.unwrap();
        stream.close().await.unwrap();
        let (peer, mut inbound) = incoming.next().await.unwrap();
        assert_eq!(peer, client.peer_id());
        let mut buf = Vec::new();
        inbound.read_to_end(&mut buf).await.unwrap();
        assert_eq!(buf, b"ping");
    }
}
