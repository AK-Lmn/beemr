//! The last resort: carry a transfer over Tor, as OnionShare does.
//!
//! A sharer that is hard to reach (strict NAT and no beemr relay) starts a
//! Tor onion service and adds its address to the DHT record. A receiver that
//! can't connect any other way dials it through Tor. Slow, but it gets
//! through any NAT or firewall that allows outgoing connections, and both
//! devices' IP addresses stay hidden from each other.
//!
//! Each share gets a fresh onion address, so separate shares can't be linked.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use arti_client::config::TorClientConfigBuilder;
use arti_client::{DataStream, TorClient};
use futures::{Stream, StreamExt};
use libp2p::multiaddr::{Onion3Addr, Protocol};
use libp2p::Multiaddr;
use tor_cell::relaycell::msg::Connected;
use tor_hsservice::config::OnionServiceConfigBuilder;
use tor_hsservice::{HsId, RunningOnionService};
use tor_proto::stream::IncomingStreamRequest;

use crate::util::hex_encode;
use crate::{crypto, Error, Result};

/// The onion service's virtual port.
pub const PORT: u16 = 1;
/// Per-process state folders older than this belong to processes that
/// didn't exit cleanly.
const STALE_AFTER: Duration = Duration::from_secs(24 * 60 * 60);

type Client = Arc<TorClient<tor_rtcompat::PreferredRuntime>>;

/// A running Tor client. Streams stay usable while this is alive.
pub struct Tor {
    client: Client,
    state_dir: PathBuf,
}

impl Drop for Tor {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.state_dir);
    }
}

impl Tor {
    /// Connect to the Tor network. The directory cache under `config_dir` is
    /// shared between runs, which makes later starts much faster; everything
    /// else (keys, guards) is private to this process and deleted afterwards.
    pub async fn start(config_dir: &Path) -> Result<Tor> {
        let root = config_dir.join("tor");
        remove_stale(&root);
        let state_dir = root.join(format!("run-{}", hex_encode(&crypto::random::<6>())));
        let mut config = TorClientConfigBuilder::from_directories(&state_dir, root.join("cache"));
        // Only public directory data and this share's throwaway onion key are
        // stored here, so don't refuse to run over folder permissions (Arti
        // is strict about them, and test machines and containers vary).
        config.storage().permissions().dangerously_trust_everyone();
        let config = config.build().map_err(tor_error)?;
        let tor = Tor {
            client: TorClient::builder().config(config).create_unbootstrapped().map_err(tor_error)?,
            state_dir,
        };
        tor.client.bootstrap().await.map_err(tor_error)?;
        Ok(tor)
    }

    /// Start a fresh onion service. Returns its address (for the DHT record),
    /// its identity (for the handshake) and the incoming streams.
    pub fn launch(
        &self,
    ) -> Result<(
        OnionService,
        impl Stream<Item = DataStream> + Send + 'static,
    )> {
        let config = OnionServiceConfigBuilder::default()
            .nickname("beemr".parse().map_err(tor_error)?)
            .build()
            .map_err(tor_error)?;
        let (service, requests) = self
            .client
            .launch_onion_service(config)
            .map_err(tor_error)?
            .ok_or_else(|| Error::new("the Tor onion service is disabled"))?;
        let id = service
            .onion_address()
            .ok_or_else(|| Error::new("the Tor onion service has no address"))?;
        let streams = tor_hsservice::handle_rend_requests(requests).filter_map(|request| async {
            match request.request() {
                IncomingStreamRequest::Begin(begin) if begin.port() == PORT => {
                    request.accept(Connected::new_empty()).await.ok()
                }
                _ => {
                    let _ = request.shutdown_circuit();
                    None
                }
            }
        });
        Ok((OnionService { id, service }, streams))
    }

    /// Open a stream to an onion service address taken from a DHT record.
    pub async fn connect(&self, addr: &Multiaddr) -> Result<(DataStream, [u8; 32])> {
        let id = onion_id(addr).ok_or_else(|| Error::new("not an onion address"))?;
        let host = display(&id);
        let stream = self
            .client
            .connect((host.as_str(), PORT))
            .await
            .map_err(tor_error)?;
        Ok((stream, id_bytes(&id)))
    }
}

/// A running onion service; it stops when dropped.
pub struct OnionService {
    id: HsId,
    service: Arc<RunningOnionService>,
}

impl OnionService {
    /// The service's identity, which the handshake binds to.
    pub fn id(&self) -> [u8; 32] {
        id_bytes(&self.id)
    }

    /// The address to publish, as an `/onion3/…:1` multiaddr.
    pub fn multiaddr(&self) -> Multiaddr {
        to_multiaddr(&self.id)
    }

    /// Whether the service has published its descriptor and can be reached.
    pub fn is_running(&self) -> bool {
        matches!(
            self.service.status().state(),
            tor_hsservice::status::State::Running | tor_hsservice::status::State::DegradedReachable
        )
    }
}

/// `BEEMR_FORCE_PATH=tor`: a diagnostic switch that skips every other path.
pub fn forced() -> bool {
    std::env::var("BEEMR_FORCE_PATH").is_ok_and(|v| v == "tor")
}

/// Whether `addr` is an onion address.
pub fn is_onion(addr: &Multiaddr) -> bool {
    onion_id(addr).is_some()
}

fn display(id: &HsId) -> String {
    use safelog::DisplayRedacted as _;
    id.display_unredacted().to_string()
}

fn id_bytes(id: &HsId) -> [u8; 32] {
    *id.as_ref()
}

fn to_multiaddr(id: &HsId) -> Multiaddr {
    // The onion3 multiaddr holds the 35 decoded bytes of the address.
    let host = display(id);
    let name = host.trim_end_matches(".onion").to_ascii_uppercase();
    let decoded = data_encoding::BASE32_NOPAD
        .decode(name.as_bytes())
        .expect("arti produces valid onion addresses");
    let hash: [u8; 35] = decoded
        .try_into()
        .expect("v3 onion addresses are 35 bytes");
    Multiaddr::empty().with(Protocol::Onion3(Onion3Addr::from((hash, PORT))))
}

fn onion_id(addr: &Multiaddr) -> Option<HsId> {
    addr.iter().find_map(|p| match p {
        Protocol::Onion3(onion) => {
            let mut name = data_encoding::BASE32_NOPAD.encode(onion.hash());
            name.make_ascii_lowercase();
            format!("{name}.onion").parse().ok()
        }
        _ => None,
    })
}

/// Delete per-process state left behind by processes that crashed.
fn remove_stale(root: &Path) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let stale = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| SystemTime::now().duration_since(t).ok())
            .is_some_and(|age| age > STALE_AFTER);
        if stale && entry.file_name().to_string_lossy().starts_with("run-") {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

fn tor_error(e: impl std::fmt::Display) -> Error {
    Error::new(format!("Tor: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn onion_addresses_round_trip_through_multiaddrs() {
        let id: HsId = "pg6mmjiyjmcrsslvykfwnntlaru7p5svn6y2ymmju6nubxndf4pscryd.onion"
            .parse()
            .unwrap();
        let addr = to_multiaddr(&id);
        assert!(addr.to_string().starts_with("/onion3/pg6mmjiyjmcrsslv"));
        assert!(is_onion(&addr));
        assert_eq!(onion_id(&addr), Some(id));
        assert!(!is_onion(&"/ip4/1.2.3.4/udp/1/quic-v1".parse().unwrap()));
    }
}
