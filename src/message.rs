//! Text messages: sending, plus the inbox and outbox stored on disk.
//!
//! The background service appends to the inbox while the CLI reads it, so
//! every access takes an exclusive file lock.

use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::connect::{self, Path as RoutePath, Purpose, Target};
use crate::discovery::{Dht, DEVICE_SALT};
use crate::identity::{DeviceId, Identity};
use crate::node::Node;
use crate::proto::{self, Framed, Message, Side, MAX_TEXT};
use crate::{Context as _, Error, Result};

const INBOX: &str = "inbox.jsonl";
const OUTBOX: &str = "outbox.jsonl";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(45);
const REPLY_TIMEOUT: Duration = Duration::from_secs(30);
/// Queued messages older than this are dropped.
pub const OUTBOX_MAX_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboxEntry {
    /// The sender's device ID (verified).
    pub from: String,
    /// The name the sender chose for itself (not verified).
    pub name: String,
    pub sent_at: u64,
    pub received_at: u64,
    pub body: String,
    #[serde(default)]
    pub read: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutboxEntry {
    pub to: String,
    pub body: String,
    pub queued_at: u64,
    #[serde(default)]
    pub attempts: u32,
}

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// An exclusive lock on a JSON-lines file, released when dropped.
struct Locked {
    path: PathBuf,
    _lock: File,
}

impl Locked {
    fn open(config: &Config, name: &str) -> Result<Locked> {
        let path = config.path(name);
        let lock_path = config.path(&format!("{name}.lock"));
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&lock_path)
            .context(lock_path.display())?;
        lock.lock().context(lock_path.display())?;
        Ok(Locked { path, _lock: lock })
    }

    fn read<T: for<'de> Deserialize<'de>>(&self) -> Result<Vec<T>> {
        let file = match File::open(&self.path) {
            Ok(file) => file,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(Error::from(e).context(self.path.display())),
        };
        // Skip lines that don't parse rather than losing the whole file.
        Ok(BufReader::new(file)
            .lines()
            .map_while(std::result::Result::ok)
            .filter_map(|line| serde_json::from_str(&line).ok())
            .collect())
    }

    fn append<T: Serialize>(&self, entry: &T) -> Result<()> {
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .context(self.path.display())?;
        writeln!(file, "{}", serde_json::to_string(entry)?).context(self.path.display())
    }

    /// Replace the whole file atomically.
    fn write<T: Serialize>(&self, entries: &[T]) -> Result<()> {
        let tmp = self.path.with_extension("jsonl.tmp");
        let mut text = String::new();
        for entry in entries {
            text.push_str(&serde_json::to_string(entry)?);
            text.push('\n');
        }
        fs::write(&tmp, text).context(tmp.display())?;
        fs::rename(&tmp, &self.path).context(self.path.display())
    }
}

pub fn store_received(config: &Config, entry: &InboxEntry) -> Result<()> {
    Locked::open(config, INBOX)?.append(entry)
}

/// All inbox messages, oldest first.
pub fn inbox(config: &Config) -> Result<Vec<InboxEntry>> {
    Locked::open(config, INBOX)?.read()
}

pub fn mark_all_read(config: &Config) -> Result<()> {
    let inbox = Locked::open(config, INBOX)?;
    let mut entries: Vec<InboxEntry> = inbox.read()?;
    if entries.iter().any(|e| !e.read) {
        entries.iter_mut().for_each(|e| e.read = true);
        inbox.write(&entries)?;
    }
    Ok(())
}

pub fn queue(config: &Config, entry: &OutboxEntry) -> Result<()> {
    Locked::open(config, OUTBOX)?.append(entry)
}

/// Take every queued message out of the outbox (to retry them).
pub fn take_outbox(config: &Config) -> Result<Vec<OutboxEntry>> {
    let outbox = Locked::open(config, OUTBOX)?;
    let entries = outbox.read()?;
    if !entries.is_empty() {
        outbox.write::<OutboxEntry>(&[])?;
    }
    Ok(entries)
}

pub fn outbox_len(config: &Config) -> Result<usize> {
    Ok(Locked::open(config, OUTBOX)?.read::<OutboxEntry>()?.len())
}

pub fn validate_body(body: &str) -> Result<String> {
    let body = body.trim();
    if body.is_empty() {
        bail!("the message is empty");
    }
    if body.len() > MAX_TEXT {
        bail!("the message is too long (max {} KB)", MAX_TEXT / 1024);
    }
    Ok(body.to_string())
}

/// Deliver a message directly to `to`'s device. Returns the path used.
pub async fn send(
    node: &Node,
    dht: &Dht,
    me: &Identity,
    my_name: &str,
    to: DeviceId,
    body: &str,
    sent_at: u64,
) -> Result<RoutePath> {
    let target = Target {
        lan: Vec::new(),
        record: Some((dht.clone(), to, DEVICE_SALT.to_vec())),
    };
    let route = connect::connect(node, target, Purpose::Message, CONNECT_TIMEOUT).await?;
    let stream = node
        .control()
        .open_stream(route.peer, proto::PROTOCOL)
        .await
        .map_err(|e| Error::new(e.to_string()))?;
    let mut framed = Framed::new(stream);
    let exchange = async {
        let peer = proto::handshake(
            &mut framed,
            me,
            my_name,
            &node.peer_id(),
            &route.peer,
            Side::Initiator,
        )
        .await?;
        if peer.device != to {
            bail!("reached a different device than the one addressed");
        }
        framed
            .send(&Message::Text {
                sent_at,
                body: body.to_string(),
            })
            .await?;
        match framed.recv().await? {
            Message::Delivered => Ok(()),
            Message::Fail(reason) => Err(Error::new(reason)),
            _ => bail!("protocol error: expected a delivery receipt"),
        }
    };
    tokio::time::timeout(REPLY_TIMEOUT, exchange)
        .await
        .map_err(|_| Error::new("the other device stopped responding"))??;
    framed.finish(Duration::from_secs(2)).await;
    Ok(route.path)
}

/// Format a timestamp relative to now, e.g. "5 min ago".
pub fn ago(timestamp: u64) -> String {
    let secs = now_secs().saturating_sub(timestamp);
    match secs {
        0..=59 => "just now".into(),
        60..=3599 => format!("{} min ago", secs / 60),
        3600..=86_399 => format!("{} h ago", secs / 3600),
        86_400..=172_799 => "yesterday".into(),
        _ => format!("{} days ago", secs / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_config() -> Config {
        Config::at(std::env::temp_dir().join(format!(
            "beemr-test-msg-{}",
            crate::util::hex_encode(&crate::crypto::random::<6>())
        )))
        .unwrap()
    }

    fn entry(body: &str) -> InboxEntry {
        InboxEntry {
            from: "x".repeat(52),
            name: "Sara".into(),
            sent_at: 1,
            received_at: 2,
            body: body.into(),
            read: false,
        }
    }

    #[test]
    fn inbox_append_read_and_mark() {
        let config = temp_config();
        assert!(inbox(&config).unwrap().is_empty());
        store_received(&config, &entry("hi")).unwrap();
        store_received(&config, &entry("there")).unwrap();
        let all = inbox(&config).unwrap();
        assert_eq!(all.len(), 2);
        assert!(all.iter().all(|e| !e.read));
        mark_all_read(&config).unwrap();
        assert!(inbox(&config).unwrap().iter().all(|e| e.read));
        fs::remove_dir_all(config.dir()).unwrap();
    }

    #[test]
    fn outbox_take_empties_it() {
        let config = temp_config();
        let queued = OutboxEntry {
            to: "y".repeat(52),
            body: "later".into(),
            queued_at: now_secs(),
            attempts: 0,
        };
        queue(&config, &queued).unwrap();
        assert_eq!(outbox_len(&config).unwrap(), 1);
        assert_eq!(take_outbox(&config).unwrap(), vec![queued]);
        assert_eq!(outbox_len(&config).unwrap(), 0);
        fs::remove_dir_all(config.dir()).unwrap();
    }

    #[test]
    fn bodies_are_validated() {
        assert_eq!(validate_body("  hi \n").unwrap(), "hi");
        assert!(validate_body("   ").is_err());
        assert!(validate_body(&"x".repeat(MAX_TEXT + 1)).is_err());
    }
}
