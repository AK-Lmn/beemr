//! The background service: receives messages, keeps this device findable by
//! its ID, retries queued messages, and relays for other devices when this
//! one is reachable from the internet.

use std::fs::{File, OpenOptions};
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use libp2p::{PeerId, Stream};

use crate::config::Config;
use crate::discovery::{Dht, DEVICE_SALT};
use crate::identity::{Contacts, DeviceId};
use crate::message::{self, InboxEntry, OutboxEntry};
use crate::node::{self, Mode, Node, NodeOptions};
use crate::profile::Profile;
use crate::proto::{self, Framed, Message, Side};
use crate::share::{find_full_relays, publish_record};
use crate::{Context as _, Error, Result};

const LOCK_FILE: &str = "daemon.lock";
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(20);
const OUTBOX_RETRY: Duration = Duration::from_secs(60);
const RELAY_ANNOUNCE_EVERY: Duration = Duration::from_secs(20 * 60);

/// Held for as long as the service runs, so only one runs per device.
pub struct InstanceLock(#[allow(dead_code)] File);

pub fn lock(config: &Config) -> Result<Option<InstanceLock>> {
    let path = config.path(LOCK_FILE);
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .context(path.display())?;
    match file.try_lock() {
        Ok(()) => Ok(Some(InstanceLock(file))),
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(std::fs::TryLockError::Error(e)) => Err(Error::from(e).context(path.display())),
    }
}

/// Whether the background service is currently running for this profile.
pub fn is_running(config: &Config) -> bool {
    matches!(lock(config), Ok(None))
}

pub async fn run(profile: Profile, port: u16) -> Result<()> {
    let Some(_lock) = lock(&profile.config)? else {
        bail!("the beemr background service is already running");
    };
    let network = profile.network.clone();
    let node = Node::start(NodeOptions {
        mode: Mode::Daemon,
        port,
        public_network: network.public,
        upnp: network.public,
        relays: profile.config.relays()?,
        key_seed: None,
    })
    .await?;
    let dht = Dht::start(&network, false)?
        .context("the background service needs the DHT, which is disabled in this environment")?;

    log(&format!(
        "started as \"{}\" ({}), listening on port {}",
        profile.name,
        profile.identity.id(),
        node.port()
    ));
    tokio::spawn(publish_record(
        node.clone(),
        dht.clone(),
        profile.identity.clone(),
        profile.name.clone(),
        DEVICE_SALT.to_vec(),
        !network.public,
    ));
    tokio::spawn(find_full_relays(node.clone(), dht.clone()));
    tokio::spawn(announce_relay_when_reachable(
        node.clone(),
        dht.clone(),
        network.public,
    ));
    let profile = Arc::new(profile);
    tokio::spawn(retry_outbox(
        node.clone(),
        dht.clone(),
        Arc::clone(&profile),
    ));

    let mut incoming = node
        .control()
        .accept(proto::PROTOCOL)
        .map_err(|e| Error::new(e.to_string()))?;
    let local = node.peer_id();
    let accept = async {
        while let Some((peer, stream)) = incoming.next().await {
            let profile = Arc::clone(&profile);
            tokio::spawn(async move {
                if let Err(e) = handle(&profile, local, peer, stream).await {
                    log(&format!("incoming connection failed: {e}"));
                }
            });
        }
    };
    tokio::select! {
        _ = accept => {}
        _ = tokio::signal::ctrl_c() => log("stopping"),
    }
    Ok(())
}

pub fn log(line: &str) {
    let time = message::now_secs();
    eprintln!("[{time}] {line}");
}

async fn handle(profile: &Profile, local: PeerId, peer: PeerId, stream: Stream) -> Result<()> {
    let mut framed = Framed::new(stream);
    let (info, request) = tokio::time::timeout(HANDSHAKE_TIMEOUT, async {
        let info = proto::handshake(
            &mut framed,
            &profile.identity,
            &profile.name,
            &local,
            &peer,
            Side::Responder,
        )
        .await?;
        let request = framed.recv().await?;
        Ok::<_, Error>((info, request))
    })
    .await
    .map_err(|_| Error::new("timed out"))??;

    match request {
        Message::Text { sent_at, body } => {
            let body = match message::validate_body(&body) {
                Ok(body) => body,
                Err(e) => {
                    framed.send(&Message::Fail(e.to_string())).await?;
                    framed.finish(Duration::from_secs(2)).await;
                    return Ok(());
                }
            };
            message::store_received(
                &profile.config,
                &InboxEntry {
                    from: info.device.to_string(),
                    name: info.name.clone(),
                    sent_at,
                    received_at: message::now_secs(),
                    body,
                    read: false,
                },
            )?;
            framed.send(&Message::Delivered).await?;
            framed.close().await;
            let contacts = Contacts::load(&profile.config)?;
            log(&format!(
                "message from {}",
                contacts.describe(&info.device, Some(&info.name))
            ));
        }
        _ => {
            framed
                .send(&Message::Fail(
                    "nothing is being shared from this device right now".into(),
                ))
                .await?;
            framed.finish(Duration::from_secs(2)).await;
        }
    }
    Ok(())
}

/// Offer to relay for others while this device is reachable from the internet.
async fn announce_relay_when_reachable(node: Node, dht: Dht, public: bool) {
    let mut announced_once = false;
    loop {
        let port = {
            let status = node.status();
            let status = status.borrow();
            status
                .external
                .iter()
                .filter(|a| node::ip_of(a).is_some_and(|ip| node::is_global(&ip) || !public))
                .find_map(port_of)
        };
        let mut wait = Duration::from_secs(30);
        if let Some(port) = port {
            if dht.announce_relay(port).await.is_ok() {
                if !announced_once {
                    log("reachable from the internet: relaying for other beemr devices");
                    announced_once = true;
                }
                wait = RELAY_ANNOUNCE_EVERY;
            }
        }
        tokio::time::sleep(wait).await;
    }
}

pub fn port_of(addr: &libp2p::Multiaddr) -> Option<u16> {
    addr.iter().find_map(|p| match p {
        libp2p::multiaddr::Protocol::Udp(port) | libp2p::multiaddr::Protocol::Tcp(port) => {
            Some(port)
        }
        _ => None,
    })
}

/// Periodically retry messages that couldn't be delivered.
async fn retry_outbox(node: Node, dht: Dht, profile: Arc<Profile>) {
    tokio::time::sleep(Duration::from_secs(10)).await;
    loop {
        if let Ok(entries) = message::take_outbox(&profile.config) {
            for mut entry in entries {
                deliver_queued(&node, &dht, &profile, &mut entry).await;
            }
        }
        tokio::time::sleep(OUTBOX_RETRY).await;
    }
}

async fn deliver_queued(node: &Node, dht: &Dht, profile: &Profile, entry: &mut OutboxEntry) {
    let age = message::now_secs().saturating_sub(entry.queued_at);
    if age > message::OUTBOX_MAX_AGE.as_secs() {
        log(&format!(
            "gave up on a message to {} after 7 days",
            entry.to
        ));
        return;
    }
    let Some(to) = DeviceId::parse(&entry.to) else {
        return;
    };
    let sent = message::send(
        node,
        dht,
        &profile.identity,
        &profile.name,
        to,
        &entry.body,
        entry.queued_at,
    )
    .await;
    match sent {
        Ok(_) => {
            let contacts = Contacts::load(&profile.config).ok();
            let who = contacts.map_or_else(|| to.short(), |c| c.describe(&to, None));
            log(&format!("delivered a queued message to {who}"));
        }
        Err(_) => {
            entry.attempts += 1;
            let _ = message::queue(&profile.config, entry);
        }
    }
}
