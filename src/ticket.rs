//! Tickets: everything a receiver needs to find and authenticate a share,
//! encoded as one copy-pasteable string.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;

use crate::crypto;
use crate::identity::DeviceId;
use crate::{Error, Result};

pub const SECRET_LEN: usize = 16;
const VERSION: u8 = 2;
const MAX_LAN_ADDRS: usize = 8;

/// The per-share secret. Only holders of the ticket can download.
pub type ShareSecret = [u8; SECRET_LEN];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ticket {
    /// The sharing device. Its signed address record is looked up on the DHT.
    pub device: DeviceId,
    pub secret: ShareSecret,
    /// Port the sharer listens on (TCP and QUIC), for the LAN addresses below.
    pub port: u16,
    /// Local network addresses, so same-network transfers work without internet.
    pub lan: Vec<IpAddr>,
}

impl Ticket {
    /// Binary layout: `version | device (32) | secret (16) | port (2) | (family | ip)*`,
    /// then base64url without padding.
    pub fn encode(&self) -> String {
        let mut bytes = vec![VERSION];
        bytes.extend_from_slice(&self.device.0);
        bytes.extend_from_slice(&self.secret);
        bytes.extend_from_slice(&self.port.to_be_bytes());
        for ip in self.lan.iter().take(MAX_LAN_ADDRS) {
            match ip {
                IpAddr::V4(ip) => {
                    bytes.push(4);
                    bytes.extend_from_slice(&ip.octets());
                }
                IpAddr::V6(ip) => {
                    bytes.push(6);
                    bytes.extend_from_slice(&ip.octets());
                }
            }
        }
        URL_SAFE_NO_PAD.encode(bytes)
    }

    pub fn decode(text: &str) -> Result<Ticket> {
        let invalid =
            || Error::new("that doesn't look like a beemr ticket (was it copied completely?)");
        let bytes = URL_SAFE_NO_PAD.decode(text.trim()).map_err(|_| invalid())?;
        let (&version, rest) = bytes.split_first().ok_or_else(invalid)?;
        if version != VERSION {
            bail!("this ticket was made by a different version of beemr; update both devices");
        }
        let (device, rest) = rest.split_at_checked(32).ok_or_else(invalid)?;
        let (secret, rest) = rest.split_at_checked(SECRET_LEN).ok_or_else(invalid)?;
        let (port, mut rest) = rest.split_at_checked(2).ok_or_else(invalid)?;
        let mut lan = Vec::new();
        while let Some((&family, tail)) = rest.split_first() {
            let (ip, tail) = match family {
                4 => {
                    let (ip, tail) = tail.split_at_checked(4).ok_or_else(invalid)?;
                    let ip: [u8; 4] = ip.try_into().expect("length checked");
                    (IpAddr::V4(Ipv4Addr::from(ip)), tail)
                }
                6 => {
                    let (ip, tail) = tail.split_at_checked(16).ok_or_else(invalid)?;
                    let ip: [u8; 16] = ip.try_into().expect("length checked");
                    (IpAddr::V6(Ipv6Addr::from(ip)), tail)
                }
                _ => return Err(invalid()),
            };
            lan.push(ip);
            rest = tail;
        }
        if lan.len() > MAX_LAN_ADDRS {
            return Err(invalid());
        }
        Ok(Ticket {
            device: DeviceId(device.try_into().expect("length checked")),
            secret: secret.try_into().expect("length checked"),
            port: u16::from_be_bytes([port[0], port[1]]),
            lan,
        })
    }

    /// The DHT salt under which the sharer publishes this share's addresses.
    /// Derived from the secret, so the record can't be found without the ticket.
    pub fn record_salt(&self) -> Vec<u8> {
        record_salt(&self.secret)
    }
}

pub fn record_salt(secret: &ShareSecret) -> Vec<u8> {
    crypto::hmac(secret, &[b"beemr/share-record"])[..16].to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Ticket {
        Ticket {
            device: DeviceId([7; 32]),
            secret: [42; SECRET_LEN],
            port: 41000,
            lan: vec!["192.168.1.20".parse().unwrap(), "fd00::1".parse().unwrap()],
        }
    }

    #[test]
    fn round_trips() {
        let ticket = sample();
        let text = ticket.encode();
        assert!(
            !text.starts_with('-'),
            "must not be mistaken for a CLI flag"
        );
        assert!(
            text.len() < 100,
            "tickets should stay short: {}",
            text.len()
        );
        assert_eq!(Ticket::decode(&text).unwrap(), ticket);
        assert_eq!(Ticket::decode(&format!("  {text}\n")).unwrap(), ticket);
    }

    #[test]
    fn rejects_garbage() {
        assert!(Ticket::decode("").is_err());
        assert!(Ticket::decode("hello world").is_err());
        let text = sample().encode();
        assert!(Ticket::decode(&text[..text.len() - 3]).is_err());
    }

    #[test]
    fn salt_depends_on_secret() {
        let mut other = sample();
        other.secret[0] ^= 1;
        assert_ne!(sample().record_salt(), other.record_salt());
    }
}
