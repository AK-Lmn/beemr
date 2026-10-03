//! Device identity (an Ed25519 key pair created on first run) and saved contacts.

use std::fmt;
use std::fs;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

use libp2p::identity::ed25519;

use crate::config::Config;
use crate::error::Context as _;
use crate::{crypto, util, Error, Result};

const IDENTITY_FILE: &str = "identity";
const CONTACTS_FILE: &str = "contacts";

/// A device's public identity: its Ed25519 public key, shown as 52 base32 characters.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DeviceId(pub [u8; 32]);

impl DeviceId {
    /// Parse a device ID, ignoring case and any separators such as `-` or spaces.
    pub fn parse(s: &str) -> Option<DeviceId> {
        let clean: String = s.chars().filter(char::is_ascii_alphanumeric).collect();
        if clean.len() != 52 {
            return None;
        }
        let bytes = util::base32_decode(&clean)?;
        Some(DeviceId(bytes.get(..32)?.try_into().ok()?))
    }

    /// The first characters of the ID, for display.
    pub fn short(&self) -> String {
        self.to_string()[..8].to_string()
    }

    /// Check an Ed25519 signature made by this device.
    pub fn verify(&self, message: &[u8], signature: &[u8]) -> bool {
        ed25519::PublicKey::try_from_bytes(&self.0)
            .map(|key| key.verify(message, signature))
            .unwrap_or(false)
    }
}

impl fmt::Display for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&util::base32_encode(&self.0))
    }
}

impl fmt::Debug for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DeviceId({self})")
    }
}

/// This device's private identity.
#[derive(Clone)]
pub struct Identity {
    seed: [u8; 32],
    keypair: ed25519::Keypair,
}

impl Identity {
    pub fn from_seed(seed: [u8; 32]) -> Self {
        let mut bytes = seed;
        let secret = ed25519::SecretKey::try_from_bytes(&mut bytes)
            .expect("any 32 bytes are a valid Ed25519 secret key");
        Identity {
            seed,
            keypair: ed25519::Keypair::from(secret),
        }
    }

    pub fn generate() -> Self {
        Self::from_seed(crypto::random())
    }

    /// Load this device's identity, creating and saving one the first time.
    /// Returns the identity and whether it was just created.
    pub fn load_or_create(config: &Config) -> Result<(Identity, bool)> {
        let path = config.path(IDENTITY_FILE);
        if let Some(identity) = Self::read(&path)? {
            return Ok((identity, false));
        }
        // Write to a private temporary file, then link it into place. Linking
        // fails if the file already exists, so when several beemr
        // processes start at once on a new device, exactly one identity wins
        // and the others load it.
        let seed: [u8; 32] = crypto::random();
        let tmp = config.path(&format!(
            ".{IDENTITY_FILE}-{}",
            util::hex_encode(&crypto::random::<6>())
        ));
        write_private(&tmp, &format!("{}\n", util::hex_encode(&seed))).context(tmp.display())?;
        let linked = fs::hard_link(&tmp, &path);
        let _ = fs::remove_file(&tmp);
        match linked {
            Ok(()) => Ok((Self::from_seed(seed), true)),
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {
                let identity = Self::read(&path)?
                    .ok_or_else(|| Error::new(format!("{} disappeared", path.display())))?;
                Ok((identity, false))
            }
            Err(e) => Err(Error::from(e).context(path.display())),
        }
    }

    fn read(path: &Path) -> Result<Option<Identity>> {
        match fs::read_to_string(path) {
            Ok(text) => {
                let seed = util::hex_decode(text.trim())
                    .and_then(|b| <[u8; 32]>::try_from(b).ok())
                    .ok_or_else(|| Error::new(format!("{} is corrupted", path.display())))?;
                Ok(Some(Self::from_seed(seed)))
            }
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
            Err(e) => Err(Error::from(e).context(path.display())),
        }
    }

    pub fn id(&self) -> DeviceId {
        DeviceId(self.keypair.public().to_bytes())
    }

    pub fn sign(&self, message: &[u8]) -> [u8; 64] {
        self.keypair
            .sign(message)
            .try_into()
            .expect("Ed25519 signatures are 64 bytes")
    }

    /// A secret derived from this identity for a separate purpose (`label`).
    pub fn derive_seed(&self, label: &str) -> [u8; 32] {
        crypto::hmac(&self.seed, &[b"beemr/derive/", label.as_bytes()])
    }

    /// The same key in the form the Mainline DHT library uses to sign records.
    pub fn dht_signing_key(&self) -> mainline::SigningKey {
        mainline::SigningKey::from_bytes(&self.seed)
    }
}

/// Create a file readable only by the current user.
fn write_private(path: &Path, contents: &str) -> Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)?.write_all(contents.as_bytes())?;
    Ok(())
}

/// Named device IDs, stored one `name id` pair per line.
pub struct Contacts {
    path: PathBuf,
    entries: Vec<(String, DeviceId)>,
}

impl Contacts {
    pub fn load(config: &Config) -> Result<Contacts> {
        let path = config.path(CONTACTS_FILE);
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == ErrorKind::NotFound => String::new(),
            Err(e) => return Err(Error::from(e).context(path.display())),
        };
        let mut entries = Vec::new();
        for (n, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            // The name may contain spaces; the ID is always the last word.
            let parsed = line
                .rsplit_once(char::is_whitespace)
                .and_then(|(name, id)| Some((name.trim().to_string(), DeviceId::parse(id)?)));
            match parsed {
                Some(entry) => entries.push(entry),
                None => bail!(
                    "{} line {}: expected `name device-id`",
                    path.display(),
                    n + 1
                ),
            }
        }
        Ok(Contacts { path, entries })
    }

    pub fn save(&self) -> Result<()> {
        let text: String = self
            .entries
            .iter()
            .map(|(name, id)| format!("{name} {id}\n"))
            .collect();
        fs::write(&self.path, text).context(self.path.display())
    }

    pub fn entries(&self) -> &[(String, DeviceId)] {
        &self.entries
    }

    /// Add or replace a contact.
    pub fn add(&mut self, name: &str, id: DeviceId) -> Result<String> {
        let name = crate::config::clean_name(name)?;
        if DeviceId::parse(&name).is_some() {
            bail!("a contact name can't look like a device ID");
        }
        self.entries
            .retain(|(n, i)| !n.eq_ignore_ascii_case(&name) && *i != id);
        self.entries.push((name.clone(), id));
        Ok(name)
    }

    /// Remove a contact; returns whether it existed.
    pub fn remove(&mut self, name: &str) -> bool {
        let before = self.entries.len();
        self.entries.retain(|(n, _)| !n.eq_ignore_ascii_case(name));
        self.entries.len() != before
    }

    /// Accept either a saved contact name or a raw device ID.
    pub fn resolve(&self, name_or_id: &str) -> Result<DeviceId> {
        self.entries
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name_or_id.trim()))
            .map(|(_, id)| *id)
            .or_else(|| DeviceId::parse(name_or_id))
            .ok_or_else(|| {
                Error::new(format!(
                    "'{name_or_id}' is neither a saved contact nor a device ID (see `beemr contact list`)"
                ))
            })
    }

    pub fn name_of(&self, id: &DeviceId) -> Option<&str> {
        self.entries
            .iter()
            .find(|(_, i)| i == id)
            .map(|(n, _)| n.as_str())
    }

    /// A friendly label for a device. Saved contact names are trusted; a name
    /// the device chose for itself is shown in quotes and marked unverified.
    pub fn describe(&self, id: &DeviceId, claimed_name: Option<&str>) -> String {
        match (self.name_of(id), claimed_name) {
            (Some(name), _) => name.to_string(),
            (None, Some(claimed)) if !claimed.is_empty() => {
                format!("\"{claimed}\" (not in contacts, {}…)", id.short())
            }
            _ => format!("device {}…", id.short()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_config(name: &str) -> Config {
        let dir = std::env::temp_dir().join(format!(
            "beemr-test-{name}-{}",
            util::hex_encode(&crypto::random::<6>())
        ));
        Config::at(dir).unwrap()
    }

    #[test]
    fn device_id_round_trips() {
        let id = Identity::generate().id();
        let text = id.to_string();
        assert_eq!(text.len(), 52);
        assert_eq!(DeviceId::parse(&text), Some(id));
        assert_eq!(DeviceId::parse(&text.to_uppercase()), Some(id));
        assert_eq!(DeviceId::parse(&text[..51]), None);
    }

    #[test]
    fn signatures_verify() {
        let me = Identity::generate();
        let sig = me.sign(b"hello");
        assert!(me.id().verify(b"hello", &sig));
        assert!(!me.id().verify(b"other", &sig));
        assert!(!Identity::generate().id().verify(b"hello", &sig));
    }

    #[test]
    fn dht_key_matches_device_id() {
        let me = Identity::generate();
        assert_eq!(me.dht_signing_key().verifying_key().to_bytes(), me.id().0);
    }

    #[test]
    fn identity_is_created_once_and_reloaded() {
        let config = temp_config("identity");
        let (first, created) = Identity::load_or_create(&config).unwrap();
        assert!(created);
        let (second, created) = Identity::load_or_create(&config).unwrap();
        assert!(!created);
        assert_eq!(first.id(), second.id());
        fs::remove_dir_all(config.dir()).unwrap();
    }

    #[test]
    fn concurrent_first_runs_agree_on_one_identity() {
        let config = temp_config("race");
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let config = config.clone();
                std::thread::spawn(move || Identity::load_or_create(&config).unwrap().0.id())
            })
            .collect();
        let ids: Vec<DeviceId> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert!(ids.windows(2).all(|w| w[0] == w[1]), "{ids:?}");
        fs::remove_dir_all(config.dir()).unwrap();
    }

    #[test]
    fn contacts_save_load_and_resolve() {
        let config = temp_config("contacts");
        let alice = Identity::generate().id();
        let mut contacts = Contacts::load(&config).unwrap();
        contacts.add("Alice Smith", alice).unwrap();
        contacts.save().unwrap();

        let contacts = Contacts::load(&config).unwrap();
        assert_eq!(contacts.resolve("alice smith").unwrap(), alice);
        assert_eq!(contacts.resolve(&alice.to_string()).unwrap(), alice);
        assert!(contacts.resolve("bob").is_err());
        assert_eq!(contacts.describe(&alice, Some("whatever")), "Alice Smith");

        let stranger = Identity::generate().id();
        assert!(contacts
            .describe(&stranger, Some("Bob's phone"))
            .starts_with("\"Bob's phone\" (not in contacts"));
        fs::remove_dir_all(config.dir()).unwrap();
    }
}
