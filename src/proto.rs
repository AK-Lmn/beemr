//! The beemr application protocol, spoken over an encrypted libp2p stream.
//!
//! See `PROTOCOL.md` for the specification this module implements.

use futures::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use libp2p::{PeerId, StreamProtocol};

use crate::crypto;
use crate::identity::{DeviceId, Identity};
use crate::ticket::ShareSecret;
use crate::{Error, Result};

/// The libp2p protocol name beemr streams are negotiated under.
pub const PROTOCOL: StreamProtocol = StreamProtocol::new("/beemr/2");
/// Maximum file bytes carried by one `Data` message.
pub const MAX_CHUNK: usize = 64 * 1024;
/// Upper bound on one encoded message; larger frames are rejected before allocating.
const MAX_MESSAGE: usize = 128 * 1024;

/// Which end of a stream we are. Bound into identity signatures so a
/// signature from one side can never be replayed as the other.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    /// Opened the stream: a receiver downloading.
    Initiator,
    /// Accepted the stream: a sharer.
    Responder,
}

impl Side {
    fn label(self) -> &'static [u8] {
        match self {
            Side::Initiator => b"beemr/hello/initiator",
            Side::Responder => b"beemr/hello/responder",
        }
    }

    fn other(self) -> Side {
        match self {
            Side::Initiator => Side::Responder,
            Side::Responder => Side::Initiator,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Dir,
}

/// Every message exchanged on a beemr stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Message {
    /// A device proving its identity for this connection, and its chosen name.
    Hello {
        device: DeviceId,
        signature: [u8; 64],
        name: String,
    },
    /// What is being shared: the top-level name and totals, for progress display.
    Header {
        name: String,
        total_bytes: u64,
        file_count: u64,
    },
    /// A file or directory. A file's contents follow as `Data` messages.
    Entry {
        kind: EntryKind,
        mode: u32,
        size: u64,
        path: String,
    },
    Data(Vec<u8>),
    /// All entries have been sent.
    End {
        total_bytes: u64,
    },
    /// The receiver saved everything successfully.
    Ack,
    /// The request was refused or aborted, with a reason for the user.
    Fail(String),
    /// Request a share's contents, proving knowledge of its ticket.
    Download {
        proof: [u8; 32],
    },
}

mod tag {
    pub const HELLO: u8 = 1;
    pub const HEADER: u8 = 2;
    pub const ENTRY: u8 = 3;
    pub const DATA: u8 = 4;
    pub const END: u8 = 5;
    pub const ACK: u8 = 6;
    pub const FAIL: u8 = 7;
    pub const DOWNLOAD: u8 = 8;
    // 9 and 10 were text messages, removed in 0.3; don't reuse them.
}

impl Message {
    fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        match self {
            Message::Hello {
                device,
                signature,
                name,
            } => {
                out.push(tag::HELLO);
                out.extend_from_slice(&device.0);
                out.extend_from_slice(signature);
                out.extend_from_slice(name.as_bytes());
            }
            Message::Header {
                name,
                total_bytes,
                file_count,
            } => {
                out.push(tag::HEADER);
                out.extend_from_slice(&total_bytes.to_be_bytes());
                out.extend_from_slice(&file_count.to_be_bytes());
                out.extend_from_slice(name.as_bytes());
            }
            Message::Entry {
                kind,
                mode,
                size,
                path,
            } => {
                out.push(tag::ENTRY);
                out.push(match kind {
                    EntryKind::File => 0,
                    EntryKind::Dir => 1,
                });
                out.extend_from_slice(&mode.to_be_bytes());
                out.extend_from_slice(&size.to_be_bytes());
                out.extend_from_slice(path.as_bytes());
            }
            Message::Data(bytes) => {
                out.reserve(1 + bytes.len());
                out.push(tag::DATA);
                out.extend_from_slice(bytes);
            }
            Message::End { total_bytes } => {
                out.push(tag::END);
                out.extend_from_slice(&total_bytes.to_be_bytes());
            }
            Message::Ack => out.push(tag::ACK),
            Message::Fail(reason) => {
                out.push(tag::FAIL);
                out.extend_from_slice(reason.as_bytes());
            }
            Message::Download { proof } => {
                out.push(tag::DOWNLOAD);
                out.extend_from_slice(proof);
            }
        }
        out
    }

    fn decode(mut buf: Vec<u8>) -> Result<Message> {
        let malformed = || Error::new("received a malformed message");
        let (&kind, body) = buf.split_first().ok_or_else(malformed)?;
        let mut r = Cursor(body);
        let msg = match kind {
            tag::HELLO => Message::Hello {
                device: DeviceId(r.array().ok_or_else(malformed)?),
                signature: r.array().ok_or_else(malformed)?,
                name: r.rest_str().ok_or_else(malformed)?,
            },
            tag::HEADER => Message::Header {
                total_bytes: r.u64().ok_or_else(malformed)?,
                file_count: r.u64().ok_or_else(malformed)?,
                name: r.rest_str().ok_or_else(malformed)?,
            },
            tag::ENTRY => Message::Entry {
                kind: match r.array::<1>().ok_or_else(malformed)? {
                    [0] => EntryKind::File,
                    [1] => EntryKind::Dir,
                    _ => return Err(malformed()),
                },
                mode: r.u32().ok_or_else(malformed)?,
                size: r.u64().ok_or_else(malformed)?,
                path: r.rest_str().ok_or_else(malformed)?,
            },
            tag::DATA => {
                buf.remove(0);
                return Ok(Message::Data(buf));
            }
            tag::END => Message::End {
                total_bytes: r.u64().ok_or_else(malformed)?,
            },
            tag::ACK => Message::Ack,
            tag::FAIL => Message::Fail(r.rest_lossy()),
            tag::DOWNLOAD => Message::Download {
                proof: r.array().ok_or_else(malformed)?,
            },
            _ => return Err(malformed()),
        };
        if !r.0.is_empty() {
            return Err(malformed());
        }
        Ok(msg)
    }
}

/// A minimal reader over a byte slice.
struct Cursor<'a>(&'a [u8]);

impl Cursor<'_> {
    fn array<const N: usize>(&mut self) -> Option<[u8; N]> {
        let (head, tail) = self.0.split_at_checked(N)?;
        self.0 = tail;
        head.try_into().ok()
    }

    fn u32(&mut self) -> Option<u32> {
        self.array().map(u32::from_be_bytes)
    }

    fn u64(&mut self) -> Option<u64> {
        self.array().map(u64::from_be_bytes)
    }

    fn rest_str(&mut self) -> Option<String> {
        let s = std::str::from_utf8(self.0).ok()?.to_string();
        self.0 = &[];
        Some(s)
    }

    fn rest_lossy(&mut self) -> String {
        let s = String::from_utf8_lossy(self.0).into_owned();
        self.0 = &[];
        s
    }
}

/// Length-prefixed messages over any byte stream.
pub struct Framed<S> {
    io: S,
}

impl<S: AsyncRead + AsyncWrite + Unpin> Framed<S> {
    pub fn new(io: S) -> Self {
        Framed { io }
    }

    pub async fn send(&mut self, msg: &Message) -> Result<()> {
        let body = msg.encode();
        debug_assert!(body.len() <= MAX_MESSAGE);
        let mut frame = Vec::with_capacity(4 + body.len());
        frame.extend_from_slice(&(body.len() as u32).to_be_bytes());
        frame.extend_from_slice(&body);
        self.io.write_all(&frame).await?;
        self.io.flush().await?;
        Ok(())
    }

    pub async fn recv(&mut self) -> Result<Message> {
        let mut len = [0u8; 4];
        self.io
            .read_exact(&mut len)
            .await
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::UnexpectedEof => {
                    Error::new("the other device closed the connection")
                }
                _ => e.into(),
            })?;
        let len = u32::from_be_bytes(len) as usize;
        if !(1..=MAX_MESSAGE).contains(&len) {
            bail!("received an invalid frame");
        }
        let mut body = vec![0u8; len];
        self.io.read_exact(&mut body).await?;
        Message::decode(body)
    }

    /// Flush and close our sending half.
    pub async fn close(&mut self) {
        let _ = self.io.close().await;
    }

    /// Close our half and wait (up to `timeout`) for the other side to close
    /// theirs. This guarantees our last message was received before the
    /// process exits, which a plain close does not.
    pub async fn finish(&mut self, timeout: std::time::Duration) {
        let _ = self.io.close().await;
        let drain = async {
            let mut sink = [0u8; 1024];
            while matches!(self.io.read(&mut sink).await, Ok(n) if n > 0) {}
        };
        let _ = tokio::time::timeout(timeout, drain).await;
    }
}

/// The verified identity of the device at the other end of a stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeerInfo {
    pub device: DeviceId,
    /// The name the device chose for itself. Not verified.
    pub name: String,
}

/// What a connection's identity signatures and download proofs are bound to,
/// so they can't be replayed on another connection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Binding {
    /// A libp2p connection: both peer IDs, authenticated by its Noise or TLS
    /// encryption.
    Libp2p {
        initiator: PeerId,
        responder: PeerId,
    },
    /// A Tor connection: the onion service's identity (authenticated by Tor)
    /// and a fresh nonce the initiator sends first.
    Tor { onion: [u8; 32], nonce: [u8; 32] },
}

impl Binding {
    /// The connection-specific bytes, ordered so that the signer comes first.
    fn parts(&self, signer: Side) -> Vec<u8> {
        match self {
            Binding::Libp2p {
                initiator,
                responder,
            } => {
                let (first, second) = match signer {
                    Side::Initiator => (initiator, responder),
                    Side::Responder => (responder, initiator),
                };
                [first.to_bytes(), second.to_bytes()].concat()
            }
            Binding::Tor { onion, nonce } => [b"tor".as_slice(), onion, nonce].concat(),
        }
    }

    fn proof_parts(&self) -> Vec<u8> {
        self.parts(Side::Initiator)
    }
}

fn hello_payload(signer: Side, binding: &Binding) -> Vec<u8> {
    [signer.label(), &binding.parts(signer)].concat()
}

/// Exchange `Hello`s. Each device signs the connection's [`Binding`], which
/// ties its long-term identity to this encrypted session, so a relay or
/// man-in-the-middle can't splice or replay it.
pub async fn handshake<S: AsyncRead + AsyncWrite + Unpin>(
    framed: &mut Framed<S>,
    me: &Identity,
    my_name: &str,
    binding: &Binding,
    side: Side,
) -> Result<PeerInfo> {
    let hello = Message::Hello {
        device: me.id(),
        signature: me.sign(&hello_payload(side, binding)),
        name: my_name.to_string(),
    };
    if side == Side::Initiator {
        framed.send(&hello).await?;
    }
    let peer = match framed.recv().await? {
        Message::Hello {
            device,
            signature,
            name,
        } => {
            if !device.verify(&hello_payload(side.other(), binding), &signature) {
                bail!("the other device's identity could not be verified");
            }
            PeerInfo {
                device,
                name: crate::config::clean_name(&name).unwrap_or_default(),
            }
        }
        Message::Fail(reason) => return Err(Error::new(reason)),
        _ => bail!("protocol error: expected a hello"),
    };
    if side == Side::Responder {
        framed.send(&hello).await?;
    }
    Ok(peer)
}

/// Proof that the downloader holds the ticket, bound to this connection.
pub fn download_proof(secret: &ShareSecret, binding: &Binding) -> [u8; 32] {
    crypto::hmac(secret, &[b"beemr/download", &binding.proof_parts()])
}

pub fn download_proof_matches(secret: &ShareSecret, binding: &Binding, proof: &[u8; 32]) -> bool {
    crypto::hmac_matches(secret, &[b"beemr/download", &binding.proof_parts()], proof)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_util::compat::{Compat, TokioAsyncReadCompatExt};

    fn pipe() -> (
        Framed<Compat<tokio::io::DuplexStream>>,
        Framed<Compat<tokio::io::DuplexStream>>,
    ) {
        let (a, b) = tokio::io::duplex(1 << 20);
        (Framed::new(a.compat()), Framed::new(b.compat()))
    }

    #[test]
    fn messages_round_trip() {
        let messages = [
            Message::Hello {
                device: DeviceId([3; 32]),
                signature: [9; 64],
                name: "Sara's laptop".into(),
            },
            Message::Header {
                name: "photos".into(),
                total_bytes: 123,
                file_count: 4,
            },
            Message::Entry {
                kind: EntryKind::File,
                mode: 0o755,
                size: 99,
                path: "photos/a b/ü.jpg".into(),
            },
            Message::Data(vec![1, 2, 3]),
            Message::Data(vec![]),
            Message::End { total_bytes: 123 },
            Message::Ack,
            Message::Fail("nope".into()),
            Message::Download { proof: [5; 32] },
        ];
        for msg in messages {
            assert_eq!(Message::decode(msg.encode()).unwrap(), msg);
        }
        assert!(Message::decode(vec![]).is_err());
        assert!(Message::decode(vec![tag::END, 1, 2]).is_err());
        assert!(Message::decode(vec![99]).is_err());
    }

    #[tokio::test]
    async fn framing_round_trips_large_data() {
        let (mut a, mut b) = pipe();
        let big = Message::Data(vec![7; MAX_CHUNK]);
        a.send(&big).await.unwrap();
        assert_eq!(b.recv().await.unwrap(), big);
    }

    #[tokio::test]
    async fn finish_waits_for_the_other_side() {
        let (mut a, mut b) = pipe();
        let peer = tokio::spawn(async move {
            assert_eq!(b.recv().await.unwrap(), Message::Ack);
            b.close().await;
        });
        a.send(&Message::Ack).await.unwrap();
        a.finish(std::time::Duration::from_secs(5)).await;
        peer.await.unwrap();
    }

    fn libp2p(initiator: PeerId, responder: PeerId) -> Binding {
        Binding::Libp2p {
            initiator,
            responder,
        }
    }

    async fn run_handshake(
        alice_sees: Binding,
        bob_sees: Binding,
    ) -> (Result<PeerInfo>, Result<PeerInfo>) {
        let (mut a, mut b) = pipe();
        let (alice, bob) = (Identity::generate(), Identity::generate());
        let responder = tokio::spawn(async move {
            handshake(&mut b, &bob, "Bob", &bob_sees, Side::Responder).await
        });
        let alice = handshake(&mut a, &alice, "Alice", &alice_sees, Side::Initiator).await;
        (alice, responder.await.unwrap())
    }

    #[tokio::test]
    async fn handshake_verifies_both_sides() {
        let (pa, pb) = (PeerId::random(), PeerId::random());
        let (alice, bob) = run_handshake(libp2p(pa, pb), libp2p(pa, pb)).await;
        assert_eq!(alice.unwrap().name, "Bob");
        assert_eq!(bob.unwrap().name, "Alice");

        let tor = Binding::Tor {
            onion: [7; 32],
            nonce: [8; 32],
        };
        let (alice, bob) = run_handshake(tor.clone(), tor).await;
        assert!(alice.is_ok() && bob.is_ok());
    }

    #[tokio::test]
    async fn handshake_rejects_signature_for_another_connection() {
        let (pa, pb, elsewhere) = (PeerId::random(), PeerId::random(), PeerId::random());
        // Alice signs for a different remote peer than Bob actually is.
        let (_, bob) = run_handshake(libp2p(pa, elsewhere), libp2p(pa, pb)).await;
        assert!(bob.is_err());

        // A Tor hello can't be replayed with another nonce.
        let (_, bob) = run_handshake(
            Binding::Tor {
                onion: [7; 32],
                nonce: [1; 32],
            },
            Binding::Tor {
                onion: [7; 32],
                nonce: [2; 32],
            },
        )
        .await;
        assert!(bob.is_err());
    }

    #[test]
    fn download_proof_is_bound_to_the_connection() {
        let secret = [1; 16];
        let (a, b) = (PeerId::random(), PeerId::random());
        let proof = download_proof(&secret, &libp2p(a, b));
        assert!(download_proof_matches(&secret, &libp2p(a, b), &proof));
        assert!(!download_proof_matches(&secret, &libp2p(b, a), &proof));
        assert!(!download_proof_matches(&[2; 16], &libp2p(a, b), &proof));
        let tor = |nonce| Binding::Tor {
            onion: [7; 32],
            nonce,
        };
        let proof = download_proof(&secret, &tor([1; 32]));
        assert!(!download_proof_matches(&secret, &tor([2; 32]), &proof));
    }

    #[test]
    fn libp2p_binding_matches_the_original_wire_format() {
        // Version 2.0 signed `label | own peer | other peer`; keep it that way.
        let (a, b) = (PeerId::random(), PeerId::random());
        let payload = hello_payload(Side::Responder, &libp2p(a, b));
        assert_eq!(
            payload,
            [
                b"beemr/hello/responder".as_slice(),
                &b.to_bytes(),
                &a.to_bytes()
            ]
            .concat()
        );
    }
}
