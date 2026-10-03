//! Receiving: find the sharing device from a ticket and save what it sends.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use futures::io::{AsyncRead, AsyncWrite};
use tokio::io::AsyncWriteExt;

use crate::connect::{self, Connection, Target};
use crate::discovery::Dht;
use crate::node::{self, Mode, Node, NodeOptions};
use crate::path::PathKind;
use crate::profile::Profile;
use crate::progress::Progress;
use crate::proto::{self, Binding, EntryKind, Framed, Message, Side};
use crate::ticket::Ticket;
use crate::tor;
use crate::util::{format_bytes, hex_encode};
use crate::{crypto, Context as _, Error, Result};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(60);
const TRANSFER_TIMEOUT: Duration = Duration::from_secs(60);

pub struct GetOptions {
    pub ticket: String,
    pub output_dir: PathBuf,
}

/// Download a share. Returns where it was saved.
pub async fn run(options: GetOptions, profile: Profile) -> Result<PathBuf> {
    let ticket = Ticket::decode(&options.ticket)?;
    if !options.output_dir.is_dir() {
        bail!("{} is not a folder", options.output_dir.display());
    }
    let network = &profile.network;
    let node = Node::start(NodeOptions {
        mode: Mode::Client,
        port: 0,
        public_network: network.public,
        upnp: network.public,
        relays: Vec::new(),
        relay_for_others: false,
        key_seed: None,
    })
    .await?;
    let dht = Dht::start(network, false)?;

    eprintln!("Connecting…");
    let target = Target {
        lan: ticket
            .lan
            .iter()
            .flat_map(|ip| node::addrs_for(*ip, ticket.port))
            .collect(),
        record: dht.map(|dht| (dht, ticket.device, ticket.record_salt())),
        tor_dir: (network.public || tor::forced()).then(|| profile.config.dir().to_path_buf()),
    };
    match connect::connect(&node, target, CONNECT_TIMEOUT).await? {
        Connection::Libp2p(route) => {
            // Port prediction may have connected through a second endpoint.
            let node = route.endpoint.clone().unwrap_or(node);
            let stream = node
                .control()
                .open_stream(route.peer, proto::PROTOCOL)
                .await
                .map_err(|e| Error::new(e.to_string()))?;
            let binding = Binding::Libp2p {
                initiator: node.peer_id(),
                responder: route.peer,
            };
            let path = node.path_to(&route.peer);
            download(Framed::new(stream), &binding, path, &ticket, &options, &profile).await
        }
        Connection::Tor(mut tor) => {
            // A fresh nonce binds this connection's handshake (see PROTOCOL.md).
            let nonce: [u8; 32] = crypto::random();
            with_timeout(async { Ok(futures::AsyncWriteExt::write_all(&mut tor.stream, &nonce).await?) }).await?;
            let binding = Binding::Tor {
                onion: tor.onion,
                nonce,
            };
            let path = Some(PathKind::Tor);
            download(Framed::new(tor.stream), &binding, path, &ticket, &options, &profile).await
        }
    }
}

/// Identify both sides, prove we hold the ticket, then receive and save.
async fn download<S: AsyncRead + AsyncWrite + Unpin>(
    mut framed: Framed<S>,
    binding: &Binding,
    path: Option<PathKind>,
    ticket: &Ticket,
    options: &GetOptions,
    profile: &Profile,
) -> Result<PathBuf> {
    let sender = with_timeout(proto::handshake(
        &mut framed,
        &profile.identity,
        &profile.name,
        binding,
        Side::Initiator,
    ))
    .await?;
    if sender.device != ticket.device {
        bail!("reached a different device than the one that made this ticket");
    }
    let proof = proto::download_proof(&ticket.secret, binding);
    with_timeout(framed.send(&Message::Download { proof })).await?;

    let (name, total_bytes, file_count) = match with_timeout(framed.recv()).await? {
        Message::Header {
            name,
            total_bytes,
            file_count,
        } => (name, total_bytes, file_count),
        Message::Fail(reason) => return Err(Error::new(reason)),
        _ => bail!("protocol error: expected a header"),
    };
    if !is_safe_name(&name) {
        bail!("the sender used an unsafe name: {name:?}");
    }
    eprintln!(
        "Receiving \"{name}\" ({file_count} file{}, {}) from {}",
        if file_count == 1 { "" } else { "s" },
        format_bytes(total_bytes),
        profile
            .contacts
            .describe(&sender.device, Some(&sender.name))
    );
    if let Some(path) = path {
        eprintln!("  How: {path}");
    }

    // Everything lands in a hidden staging folder first, so an interrupted
    // transfer never leaves a half-written file under the real name.
    let staging = options.output_dir.join(format!(
        ".beemr-partial-{}",
        hex_encode(&crypto::random::<6>())
    ));
    fs::create_dir(&staging).context(staging.display())?;

    let transfer = async {
        receive_content(&mut framed, &staging, &name, total_bytes).await?;
        move_into_place(&staging, &options.output_dir, &name)
    };
    let saved = tokio::select! {
        saved = transfer => saved,
        _ = tokio::signal::ctrl_c() => Err(Error::new("cancelled")),
    };
    match saved {
        Ok(path) => {
            let _ = fs::remove_dir(&staging);
            // The files are safely saved even if the sender misses this, but
            // wait for it to arrive so the sender can count the download.
            let _ = framed.send(&Message::Ack).await;
            framed.finish(Duration::from_secs(5)).await;
            Ok(path)
        }
        Err(e) => {
            let _ = fs::remove_dir_all(&staging);
            let _ = framed.send(&Message::Fail(e.to_string())).await;
            framed.finish(Duration::from_secs(2)).await;
            Err(e)
        }
    }
}

async fn with_timeout<T>(future: impl std::future::Future<Output = Result<T>>) -> Result<T> {
    tokio::time::timeout(TRANSFER_TIMEOUT, future)
        .await
        .map_err(|_| Error::new("the other device stopped responding"))?
}

async fn receive_content(
    framed: &mut Framed<impl AsyncRead + AsyncWrite + Unpin>,
    staging: &Path,
    root: &str,
    total: u64,
) -> Result<()> {
    let mut progress = Progress::new("Receiving", total);
    let mut received = 0u64;
    loop {
        match with_timeout(framed.recv()).await? {
            Message::Entry {
                kind: EntryKind::Dir,
                path,
                ..
            } => {
                let dest = entry_path(staging, root, &path)?;
                fs::create_dir_all(&dest).context(&path)?;
            }
            Message::Entry {
                kind: EntryKind::File,
                mode,
                size,
                path,
            } => {
                let dest = entry_path(staging, root, &path)?;
                receive_file(framed, &dest, size, mode, &mut progress)
                    .await
                    .map_err(|e| e.context(&path))?;
                received += size;
            }
            Message::End { total_bytes } => {
                if total_bytes != received {
                    bail!("transfer incomplete: got {received} of {total_bytes} bytes");
                }
                progress.finish();
                return Ok(());
            }
            Message::Fail(reason) => bail!("the sender stopped: {reason}"),
            _ => bail!("protocol error: unexpected message"),
        }
    }
}

async fn receive_file(
    framed: &mut Framed<impl AsyncRead + AsyncWrite + Unpin>,
    dest: &Path,
    size: u64,
    mode: u32,
    progress: &mut Progress,
) -> Result<()> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    // `create_new` refuses to overwrite, so a duplicate entry can't clobber a file.
    let file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dest)
        .await?;
    let mut out = tokio::io::BufWriter::new(file);
    let mut remaining = size;
    while remaining > 0 {
        let chunk = match with_timeout(framed.recv()).await? {
            Message::Data(chunk) => chunk,
            Message::Fail(reason) => bail!("the sender stopped: {reason}"),
            _ => bail!("transfer ended before the file was complete"),
        };
        let len = chunk.len() as u64;
        if len > remaining {
            bail!("the sender sent more data than it announced");
        }
        out.write_all(&chunk).await?;
        remaining -= len;
        progress.add(len);
    }
    out.flush().await?;
    apply_executable_bits(dest, mode)
}

#[cfg(unix)]
fn apply_executable_bits(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    if mode & 0o111 != 0 {
        let mut permissions = fs::metadata(path)?.permissions();
        permissions.set_mode(permissions.mode() | (mode & 0o111));
        fs::set_permissions(path, permissions)?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn apply_executable_bits(_path: &Path, _mode: u32) -> Result<()> {
    Ok(())
}

/// Map a `/`-separated path from the sender to a location inside `staging`,
/// rejecting anything that could escape it.
fn entry_path(staging: &Path, root: &str, path: &str) -> Result<PathBuf> {
    let mut parts = path.split('/');
    if parts.next() != Some(root) {
        bail!("the sender sent a file outside the share: {path:?}");
    }
    let mut dest = staging.join(root);
    for part in parts {
        if !is_safe_name(part) {
            bail!("the sender sent an unsafe file name: {path:?}");
        }
        dest.push(part);
    }
    Ok(dest)
}

/// A single path component that can't traverse directories on any platform.
fn is_safe_name(name: &str) -> bool {
    const WINDOWS_RESERVED: &[char] = &[':', '<', '>', '"', '|', '?', '*'];
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains(['/', '\\', '\0'])
        && !(cfg!(windows) && name.contains(WINDOWS_RESERVED))
}

/// Move the finished download out of staging, picking a free name if needed.
fn move_into_place(staging: &Path, output_dir: &Path, name: &str) -> Result<PathBuf> {
    let source = staging.join(name);
    let dest = free_path(output_dir, name, source.is_dir());
    fs::rename(&source, &dest).context(dest.display())?;
    Ok(dest)
}

/// `name`, or `name (1)`, `name (2)`… (before the extension for files).
fn free_path(dir: &Path, name: &str, is_dir: bool) -> PathBuf {
    let taken = |p: &Path| fs::symlink_metadata(p).is_ok();
    let first = dir.join(name);
    if !taken(&first) {
        return first;
    }
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 && !is_dir => name.split_at(i),
        _ => (name, ""),
    };
    (1..)
        .map(|n| dir.join(format!("{stem} ({n}){ext}")))
        .find(|p| !taken(p))
        .expect("an unused name exists")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_paths_stay_inside_staging() {
        let staging = Path::new("staging");
        assert_eq!(
            entry_path(staging, "photos", "photos/2024/a.jpg").unwrap(),
            staging.join("photos").join("2024").join("a.jpg")
        );
        assert_eq!(
            entry_path(staging, "a.txt", "a.txt").unwrap(),
            staging.join("a.txt")
        );
        for bad in [
            "other/a.jpg",
            "photos/../../etc/passwd",
            "photos/./a",
            "photos//a",
            "photos/a\\..\\b",
            "/photos/a",
            "",
        ] {
            assert!(entry_path(staging, "photos", bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn safe_names() {
        assert!(is_safe_name("report.pdf"));
        assert!(is_safe_name("ünïcode name"));
        for bad in ["", ".", "..", "a/b", "a\\b", "a\0b"] {
            assert!(!is_safe_name(bad), "{bad:?}");
        }
    }

    #[test]
    fn free_path_avoids_existing_files() {
        let dir = std::env::temp_dir().join(format!(
            "beemr-test-free-{}",
            hex_encode(&crypto::random::<6>())
        ));
        fs::create_dir_all(&dir).unwrap();
        assert_eq!(free_path(&dir, "a.txt", false), dir.join("a.txt"));
        fs::write(dir.join("a.txt"), "x").unwrap();
        assert_eq!(free_path(&dir, "a.txt", false), dir.join("a (1).txt"));
        fs::create_dir(dir.join("my.folder")).unwrap();
        assert_eq!(
            free_path(&dir, "my.folder", true),
            dir.join("my.folder (1)")
        );
        fs::remove_dir_all(dir).unwrap();
    }
}
