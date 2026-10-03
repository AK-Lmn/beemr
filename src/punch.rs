//! Port prediction: a direct connection through a strict (symmetric) NAT.
//!
//! Standard hole punching fails when a NAT picks a new external port for
//! every destination, because the other side can't know which port to aim at.
//! When only one side is strict, the "birthday" technique still works:
//!
//! - The **strict side** opens many UDP sockets and sends probes from each to
//!   the other side's stable external QUIC address, creating many mappings
//!   on its NAT with unknown, random external ports.
//! - The **other side** sprays punch packets from its QUIC socket at random
//!   ports on the strict side's IP. With enough of both, one lands on an open
//!   mapping, like the birthday paradox.
//! - The strict socket that receives a punch packet knows its path works. It
//!   closes, and a second libp2p QUIC endpoint with the **same identity**
//!   binds that port and dials the other side directly, so the rest of beemr
//!   simply sees a direct connection to the same peer.
//!
//! Coordination runs over the existing (relayed) connection, using the
//! `/beemr/punch/1` stream protocol.

use std::collections::HashSet;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use libp2p::multiaddr::Protocol;
use libp2p::swarm::dial_opts::{DialOpts, PeerCondition};
use libp2p::{Multiaddr, PeerId, Stream, StreamProtocol};
use tokio::net::UdpSocket;
use tokio::sync::Notify;

use crate::nat::NatKind;
use crate::node::{Mode, Node, NodeEvent, NodeOptions};
use crate::path::PathKind;
use crate::{crypto, Error, Result};

pub const PROTOCOL: StreamProtocol = StreamProtocol::new("/beemr/punch/1");

/// Sockets the strict side opens. Kept under common open-file limits.
const SOCKETS: usize = 200;
/// Random ports the other side aims at. With 200 sockets this gives ~85%.
const SPRAY: usize = 600;
const PROBE_EVERY: Duration = Duration::from_millis(150);
const PUNCH_FOR: Duration = Duration::from_secs(8);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(6);

/// What each side tells the other before punching.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Info {
    kind: NatKind,
    /// Our external QUIC address as a peer saw it.
    external: Option<SocketAddr>,
}

impl Info {
    fn local(node: &Node) -> Info {
        Info {
            kind: node.status().borrow().nat,
            external: node.observed_quic(),
        }
    }

    fn encode(&self) -> Vec<u8> {
        let kind = match self.kind {
            NatKind::Unknown => 0,
            NatKind::Reachable => 1,
            NatKind::Cone => 2,
            NatKind::Symmetric => 3,
        };
        let mut out = vec![kind];
        if let Some(SocketAddr::V4(addr)) = self.external {
            out.extend_from_slice(&addr.ip().octets());
            out.extend_from_slice(&addr.port().to_be_bytes());
        }
        out
    }

    fn decode(bytes: &[u8]) -> Result<Info> {
        let kind = match bytes.first() {
            Some(0) => NatKind::Unknown,
            Some(1) => NatKind::Reachable,
            Some(2) => NatKind::Cone,
            Some(3) => NatKind::Symmetric,
            _ => bail!("malformed punch message"),
        };
        let external = bytes.get(1..7).map(|rest| {
            SocketAddr::new(
                IpAddr::V4(Ipv4Addr::new(rest[0], rest[1], rest[2], rest[3])),
                u16::from_be_bytes([rest[4], rest[5]]),
            )
        });
        Ok(Info { kind, external })
    }
}

/// The role each side plays, decided from both sides' NAT kinds.
#[derive(Debug, PartialEq, Eq)]
enum Role {
    /// The strict side: opens many sockets, then connects from the winner.
    Strict { target: SocketAddr },
    /// The easy side: sprays punch packets at the strict side's IP.
    Sprayer { target_ip: IpAddr },
}

fn roles(mine: Info, theirs: Info) -> Option<Role> {
    match (mine.kind, theirs.kind) {
        (NatKind::Symmetric, NatKind::Cone) => Some(Role::Strict {
            target: theirs.external?,
        }),
        (NatKind::Cone, NatKind::Symmetric) => Some(Role::Sprayer {
            target_ip: theirs.external?.ip(),
        }),
        _ => None,
    }
}

/// Run port prediction from the side that opened the punch stream. Returns
/// a second endpoint holding the direct connection if this side was the
/// strict one; `None` if this side sprayed (the direct connection then
/// arrives on `node`).
pub async fn initiate(node: &Node, peer: PeerId) -> Result<Option<Node>> {
    let mut control = node.control();
    let mut stream = control
        .open_stream(peer, PROTOCOL)
        .await
        .map_err(|e| Error::new(e.to_string()))?;
    let mine = Info::local(node);
    let sent = Instant::now();
    write_frame(&mut stream, &mine.encode()).await?;
    let theirs = Info::decode(&read_frame(&mut stream).await?)?;
    let round_trip = sent.elapsed();
    tracing::debug!(?mine, ?theirs, "port prediction");
    let role = roles(mine, theirs).ok_or_else(|| Error::new("port prediction not applicable"))?;
    write_frame(&mut stream, b"go").await?;
    // The other side starts when "go" arrives, half a round trip from now.
    tokio::time::sleep(round_trip / 2).await;
    let result = punch(node, peer, role).await;
    let _ = stream.close().await;
    result
}

/// Answer a punch stream opened by `peer`. See [`initiate`].
pub async fn respond(node: &Node, peer: PeerId, mut stream: Stream) -> Result<Option<Node>> {
    let theirs = Info::decode(&read_frame(&mut stream).await?)?;
    let mine = Info::local(node);
    write_frame(&mut stream, &mine.encode()).await?;
    tracing::debug!(?mine, ?theirs, "port prediction");
    let role = roles(mine, theirs).ok_or_else(|| Error::new("port prediction not applicable"))?;
    if read_frame(&mut stream).await? != b"go" {
        bail!("punch coordination failed");
    }
    let result = punch(node, peer, role).await;
    let _ = stream.close().await;
    result
}

async fn punch(node: &Node, peer: PeerId, role: Role) -> Result<Option<Node>> {
    match role {
        Role::Sprayer { target_ip } => {
            // Mark first: the connection arrives (and is used) mid-spray.
            node.mark_punched(peer, PathKind::PortPredicted);
            spray(node, peer, target_ip).await;
            Ok(None)
        }
        Role::Strict { target } => {
            let port = probe_until_hit(target).await?;
            let endpoint = connect_from(node, peer, port, target).await?;
            endpoint.mark_punched(peer, PathKind::PortPredicted);
            Ok(Some(endpoint))
        }
    }
}

/// Aim QUIC punch packets at random ports on the strict side's IP. Each dial
/// with an overridden role makes libp2p-quic send punch packets from our QUIC
/// socket; the strict side's connection then arrives as an inbound connection.
async fn spray(node: &Node, peer: PeerId, target_ip: IpAddr) {
    let mut ports = HashSet::new();
    while ports.len() < SPRAY {
        let random = u16::from_be_bytes(crypto::random());
        ports.insert(1024 + random % (u16::MAX - 1024));
    }
    for port in ports {
        let addr = quic_addr(target_ip, port);
        node.dial(
            DialOpts::peer_id(peer)
                .addresses(vec![addr])
                .condition(PeerCondition::Always)
                .override_role()
                .build(),
        );
    }
    tokio::time::sleep(PUNCH_FOR).await;
}

/// Open many sockets probing `target`; return the local port of the first
/// socket that hears back from it.
async fn probe_until_hit(target: SocketAddr) -> Result<u16> {
    let mut sockets = Vec::with_capacity(SOCKETS);
    for _ in 0..SOCKETS {
        match UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).await {
            Ok(socket) => sockets.push(Arc::new(socket)),
            Err(_) => break, // out of file descriptors: use what we have
        }
    }
    if sockets.is_empty() {
        bail!("couldn't open sockets for port prediction");
    }
    let done = Arc::new(Notify::new());
    let (hit_tx, mut hit_rx) = tokio::sync::mpsc::channel::<u16>(1);
    let mut tasks = Vec::new();
    for socket in sockets {
        let (done, hit_tx) = (Arc::clone(&done), hit_tx.clone());
        tasks.push(tokio::spawn(async move {
            let port = socket.local_addr().map(|a| a.port()).unwrap_or(0);
            let probe: [u8; 8] = crypto::random();
            let mut ticker = tokio::time::interval(PROBE_EVERY);
            let mut buf = [0u8; 2048];
            loop {
                tokio::select! {
                    _ = ticker.tick() => {
                        let _ = socket.send_to(&probe, target).await;
                    }
                    received = socket.recv_from(&mut buf) => {
                        if let Ok((_, from)) = received {
                            if from == target {
                                let _ = hit_tx.try_send(port);
                                return;
                            }
                        }
                    }
                    _ = done.notified() => return,
                }
            }
        }));
    }
    drop(hit_tx);
    let hit = tokio::time::timeout(PUNCH_FOR, hit_rx.recv()).await;
    done.notify_waiters();
    for task in tasks {
        task.abort();
        let _ = task.await;
    }
    match hit {
        Ok(Some(port)) => Ok(port),
        _ => bail!("port prediction didn't find a path"),
    }
}

/// Start a second QUIC endpoint with this node's identity on `port` (whose
/// NAT mapping toward `target` is open) and connect to the peer from it.
async fn connect_from(node: &Node, peer: PeerId, port: u16, target: SocketAddr) -> Result<Node> {
    let endpoint = Node::start(NodeOptions {
        mode: Mode::Client,
        port,
        public_network: false,
        upnp: false,
        relays: Vec::new(),
        relay_for_others: false,
        key_seed: Some(node.key_seed()),
    })
    .await?;
    let mut events = endpoint.events();
    endpoint.dial(
        DialOpts::peer_id(peer)
            .addresses(vec![quic_addr(target.ip(), target.port())])
            .condition(PeerCondition::Always)
            .build(),
    );
    let connected = async {
        loop {
            match events.recv().await {
                Ok(NodeEvent::Connected {
                    peer: p,
                    relay: None,
                    ..
                }) if p == peer => return true,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return false,
                _ => {}
            }
        }
    };
    match tokio::time::timeout(CONNECT_TIMEOUT, connected).await {
        Ok(true) => Ok(endpoint),
        _ => bail!("port prediction found a path but couldn't connect over it"),
    }
}

fn quic_addr(ip: IpAddr, port: u16) -> Multiaddr {
    Multiaddr::empty()
        .with(match ip {
            IpAddr::V4(ip) => Protocol::Ip4(ip),
            IpAddr::V6(ip) => Protocol::Ip6(ip),
        })
        .with(Protocol::Udp(port))
        .with(Protocol::QuicV1)
}

async fn write_frame<S: AsyncWrite + Unpin>(io: &mut S, body: &[u8]) -> Result<()> {
    io.write_all(&[body.len() as u8]).await?;
    io.write_all(body).await?;
    io.flush().await?;
    Ok(())
}

async fn read_frame<S: AsyncRead + Unpin>(io: &mut S) -> Result<Vec<u8>> {
    let mut len = [0u8; 1];
    tokio::time::timeout(Duration::from_secs(10), io.read_exact(&mut len))
        .await
        .map_err(|_| Error::new("punch coordination timed out"))??;
    let mut body = vec![0u8; len[0] as usize];
    io.read_exact(&mut body).await?;
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn info_round_trips() {
        let info = Info {
            kind: NatKind::Cone,
            external: Some("203.0.113.9:4545".parse().unwrap()),
        };
        assert_eq!(Info::decode(&info.encode()).unwrap(), info);
        let unknown = Info {
            kind: NatKind::Unknown,
            external: None,
        };
        assert_eq!(Info::decode(&unknown.encode()).unwrap(), unknown);
        assert!(Info::decode(&[9]).is_err());
    }

    #[test]
    fn roles_need_one_strict_and_one_cone() {
        let cone = Info {
            kind: NatKind::Cone,
            external: Some("198.51.100.1:5000".parse().unwrap()),
        };
        let strict = Info {
            kind: NatKind::Symmetric,
            external: Some("203.0.113.9:61234".parse().unwrap()),
        };
        assert_eq!(
            roles(strict, cone),
            Some(Role::Strict {
                target: "198.51.100.1:5000".parse().unwrap()
            })
        );
        assert_eq!(
            roles(cone, strict),
            Some(Role::Sprayer {
                target_ip: "203.0.113.9".parse().unwrap()
            })
        );
        assert_eq!(roles(strict, strict), None);
        assert_eq!(roles(cone, cone), None);
    }

    #[tokio::test]
    async fn strict_side_finds_the_socket_that_hears_back() {
        // Stand-in for the other side: answer the first probe it gets.
        let easy = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let target = easy.local_addr().unwrap();
        let answer = tokio::spawn(async move {
            let mut buf = [0u8; 64];
            let (_, from) = easy.recv_from(&mut buf).await.unwrap();
            easy.send_to(b"punch", from).await.unwrap();
            from.port()
        });
        let found = probe_until_hit(target).await.unwrap();
        assert_eq!(found, answer.await.unwrap());
    }
}
