//! How a transfer travels, described the same way on both sides.

use std::fmt;
use std::net::IpAddr;

/// The path a connection takes between two devices.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PathKind {
    /// Direct, on the same local network.
    Lan,
    /// Direct, over the internet with IPv6.
    Ipv6,
    /// Direct, over the internet with IPv4. `mapped` names how the sharer's
    /// router opened a port (UPnP, PCP, NAT-PMP), when known.
    Internet { mapped: Option<&'static str> },
    /// Direct, after hole punching through both NATs.
    HolePunched,
    /// Direct, after predicting a strict NAT's port.
    PortPredicted,
    /// Through a relay.
    Relay { addr: Option<IpAddr> },
    /// Through the Tor network.
    Tor,
}

impl fmt::Display for PathKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PathKind::Lan => write!(
                f,
                "direct connection, same Wi-Fi/LAN (fastest; never leaves your network)"
            ),
            PathKind::Ipv6 => write!(f, "direct connection over the internet via IPv6"),
            PathKind::Internet { mapped: Some(how) } => write!(
                f,
                "direct connection through the router (port opened automatically with {how})"
            ),
            PathKind::Internet { mapped: None } => {
                write!(f, "direct connection over the internet")
            }
            PathKind::HolePunched => write!(
                f,
                "direct connection, punched through both firewalls (hole punching)"
            ),
            PathKind::PortPredicted => write!(
                f,
                "direct connection, punched through a strict firewall (port prediction)"
            ),
            PathKind::Relay { addr } => {
                write!(f, "relayed through beemr relay")?;
                if let Some(addr) = addr {
                    write!(f, " {addr}")?;
                }
                write!(
                    f,
                    " (slower; end-to-end encrypted, the relay can't read it)"
                )
            }
            PathKind::Tor => write!(
                f,
                "relayed through Tor (slowest; end-to-end encrypted and anonymous)"
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptions_name_the_path() {
        assert!(PathKind::Lan.to_string().contains("same Wi-Fi/LAN"));
        assert!(PathKind::Internet {
            mapped: Some("PCP")
        }
        .to_string()
        .contains("opened automatically with PCP"));
        let relay = PathKind::Relay {
            addr: Some("203.0.113.5".parse().unwrap()),
        };
        assert!(relay.to_string().contains("beemr relay 203.0.113.5"));
        assert!(PathKind::Tor.to_string().contains("Tor"));
    }
}
