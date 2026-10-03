//! Finding devices: signed address records on the BitTorrent Mainline DHT.
//!
//! Records are BEP 44 mutable items signed by the device's own key, so only
//! that device can publish them, and anyone can verify where they came from.
//! The DHT is a public network of millions of BitTorrent nodes; nothing here
//! depends on a server run by us.

use std::net::SocketAddrV4;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures::StreamExt;
use libp2p::{Multiaddr, PeerId};
use mainline::async_dht::AsyncDht;
use mainline::{Dht as MainlineDht, Id, MutableItem};

use crate::config::NetworkSettings;
use crate::identity::{DeviceId, Identity};
use crate::{crypto, Error, Result};

/// Topic under which beemr relays announce themselves.
const RELAYS_TOPIC: &[u8] = b"beemr/relays/1";
const RECORD_VERSION: u8 = 1;
/// BEP 44 limits values to 1000 bytes.
const MAX_VALUE: usize = 1000;

/// Where a device can currently be reached.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AddressRecord {
    pub peer: PeerId,
    /// The device's self-chosen name.
    pub name: String,
    /// Dialable addresses: direct ones and circuits through limited relays.
    pub addrs: Vec<Multiaddr>,
    /// Circuits through relays that allow full file transfers.
    pub full_relays: Vec<Multiaddr>,
}

impl AddressRecord {
    /// `version | peer (u8 len) | name (u8 len) | count (u8) | (flag | u16 len | multiaddr)*`,
    /// where flag 1 marks a full relay. Addresses that don't fit in the
    /// 1000-byte limit are dropped, lowest priority (latest) first.
    pub fn encode(&self) -> Vec<u8> {
        let peer = self.peer.to_bytes();
        let name = truncate_utf8(&self.name, 64);
        let mut out = vec![RECORD_VERSION, peer.len() as u8];
        out.extend_from_slice(&peer);
        out.push(name.len() as u8);
        out.extend_from_slice(name.as_bytes());
        let count_at = out.len();
        out.push(0);
        let mut count = 0u8;
        let flagged = self
            .full_relays
            .iter()
            .map(|a| (1u8, a))
            .chain(self.addrs.iter().map(|a| (0u8, a)));
        for (flag, addr) in flagged {
            let bytes = addr.to_vec();
            if out.len() + 3 + bytes.len() > MAX_VALUE || count == u8::MAX {
                continue;
            }
            out.push(flag);
            out.extend_from_slice(&(bytes.len() as u16).to_be_bytes());
            out.extend_from_slice(&bytes);
            count += 1;
        }
        out[count_at] = count;
        out
    }

    pub fn decode(bytes: &[u8]) -> Option<AddressRecord> {
        let mut r = bytes;
        let mut take = |n: usize| -> Option<&[u8]> {
            let (head, tail) = r.split_at_checked(n)?;
            r = tail;
            Some(head)
        };
        if take(1)? != [RECORD_VERSION] {
            return None;
        }
        let peer_len = take(1)?[0] as usize;
        let peer = PeerId::from_bytes(take(peer_len)?).ok()?;
        let name_len = take(1)?[0] as usize;
        let name = String::from_utf8(take(name_len)?.to_vec()).ok()?;
        let count = take(1)?[0];
        let (mut addrs, mut full_relays) = (Vec::new(), Vec::new());
        for _ in 0..count {
            let flag = take(1)?[0];
            let len = take(2)?;
            let len = u16::from_be_bytes([len[0], len[1]]) as usize;
            if let Ok(addr) = Multiaddr::try_from(take(len)?.to_vec()) {
                if flag == 1 {
                    full_relays.push(addr);
                } else {
                    addrs.push(addr);
                }
            }
        }
        Some(AddressRecord {
            peer,
            name,
            addrs,
            full_relays,
        })
    }
}

fn truncate_utf8(s: &str, max: usize) -> &str {
    let mut end = s.len().min(max);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// A handle to this process's Mainline DHT node.
#[derive(Clone, Debug)]
pub struct Dht {
    inner: AsyncDht,
}

impl Dht {
    /// Join the DHT, or return `None` if discovery is disabled (isolated tests
    /// without a test DHT). `server` makes this node store others' records too.
    pub fn start(network: &NetworkSettings, server: bool) -> Result<Option<Dht>> {
        let mut builder = MainlineDht::builder();
        match &network.dht_bootstrap {
            Some(bootstrap) => {
                builder.bootstrap(bootstrap);
            }
            None if network.public => {}
            // A server in an isolated network can be the first node of a private DHT.
            None if server => {
                builder.no_bootstrap();
            }
            None => return Ok(None),
        }
        if let Some(ip) = network.dht_bind {
            builder.bind_address(ip);
        }
        if let Some(port) = network.dht_port {
            builder.port(port);
        }
        if server {
            builder.server_mode();
        }
        let dht = builder
            .build()
            .map_err(|e| Error::from(e).context("can't start the DHT"))?;
        Ok(Some(Dht {
            inner: dht.as_async(),
        }))
    }

    /// Publish (or refresh) a record signed by `identity`.
    pub async fn publish(
        &self,
        identity: &Identity,
        salt: &[u8],
        record: &AddressRecord,
    ) -> Result<()> {
        let seq = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_micros() as i64);
        let item = MutableItem::new(
            identity.dht_signing_key(),
            &record.encode(),
            seq,
            Some(salt),
        );
        self.inner
            .put_mutable(item, None)
            .await
            .map_err(|e| Error::from(e).context("couldn't publish to the DHT"))?;
        Ok(())
    }

    /// The most recent record `device` published under `salt`, if any.
    pub async fn resolve(&self, device: &DeviceId, salt: &[u8]) -> Option<AddressRecord> {
        let item = self
            .inner
            .get_mutable_most_recent(&device.0, Some(salt))
            .await?;
        AddressRecord::decode(item.value())
    }

    /// Announce that a beemr relay listens on `port` at this machine's public IP.
    pub async fn announce_relay(&self, port: u16) -> Result<()> {
        self.inner
            .announce_peer(relays_topic(), Some(port))
            .await
            .map_err(|e| Error::from(e).context("couldn't announce the relay"))?;
        Ok(())
    }

    /// Public beemr relays announced on the DHT.
    pub async fn find_relays(&self, max: usize) -> Vec<SocketAddrV4> {
        let mut found = Vec::new();
        let mut stream = self.inner.get_peers(relays_topic());
        let deadline = tokio::time::sleep(Duration::from_secs(15));
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                batch = stream.next() => match batch {
                    Some(batch) => {
                        for addr in batch {
                            if !found.contains(&addr) {
                                found.push(addr);
                            }
                        }
                        if found.len() >= max {
                            break;
                        }
                    }
                    None => break,
                },
                _ = &mut deadline => break,
            }
        }
        found.truncate(max);
        found
    }

    /// This machine's public IPv4 address and port as seen by DHT nodes.
    pub async fn public_address(&self) -> Option<SocketAddrV4> {
        self.inner.info().await.public_address()
    }

    pub async fn bootstrapped(&self) -> bool {
        self.inner.bootstrapped().await
    }
}

fn relays_topic() -> Id {
    Id::from_bytes(&crypto::sha256(RELAYS_TOPIC)[..20]).expect("20 bytes is a valid id")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_round_trips() {
        let record = AddressRecord {
            peer: PeerId::random(),
            name: "Osman's Mac mini".into(),
            addrs: vec![
                "/ip4/192.168.1.5/udp/4001/quic-v1".parse().unwrap(),
                "/ip6/2a01:4f8::1/tcp/4001".parse().unwrap(),
            ],
            full_relays: vec![format!(
                "/ip4/203.0.113.7/udp/4001/quic-v1/p2p/{}/p2p-circuit",
                PeerId::random()
            )
            .parse()
            .unwrap()],
        };
        assert_eq!(AddressRecord::decode(&record.encode()), Some(record));
        assert_eq!(AddressRecord::decode(&[9, 9, 9]), None);
        assert_eq!(AddressRecord::decode(&[]), None);
    }

    #[test]
    fn record_stays_within_dht_limit() {
        let relay = PeerId::random();
        let me = PeerId::random();
        let addr: Multiaddr =
            format!("/ip4/203.0.113.7/udp/4001/quic-v1/p2p/{relay}/p2p-circuit/p2p/{me}")
                .parse()
                .unwrap();
        let record = AddressRecord {
            peer: me,
            name: "x".repeat(200),
            addrs: vec![addr; 40],
            full_relays: vec![],
        };
        let encoded = record.encode();
        assert!(encoded.len() <= MAX_VALUE);
        let decoded = AddressRecord::decode(&encoded).unwrap();
        assert!(!decoded.addrs.is_empty() && decoded.addrs.len() < 40);
        assert_eq!(decoded.name.len(), 64);
    }

    #[tokio::test]
    async fn publish_and_resolve_on_a_test_network() {
        let testnet = mainline::Testnet::builder(8).build().unwrap();
        let network = NetworkSettings {
            public: false,
            dht_bootstrap: Some(testnet.bootstrap.clone()),
            dht_bind: Some(std::net::Ipv4Addr::LOCALHOST),
            dht_port: None,
        };
        let publisher = Dht::start(&network, false).unwrap().unwrap();
        let resolver = Dht::start(&network, false).unwrap().unwrap();
        let me = Identity::generate();
        let record = AddressRecord {
            peer: PeerId::random(),
            name: "test".into(),
            addrs: vec!["/ip4/10.0.0.1/tcp/1".parse().unwrap()],
            full_relays: vec![],
        };
        publisher.publish(&me, b"test", &record).await.unwrap();
        assert_eq!(resolver.resolve(&me.id(), b"test").await, Some(record));
        assert_eq!(resolver.resolve(&me.id(), b"other salt").await, None);
    }
}
