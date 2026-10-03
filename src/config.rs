//! Where beemr keeps its state, and small persistent settings.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use libp2p::Multiaddr;

use crate::error::Context as _;
use crate::Result;

const NAME_FILE: &str = "name";
const RELAYS_FILE: &str = "relays";
/// Longest device name accepted, in characters.
pub const MAX_NAME_LEN: usize = 64;

/// Which networks beemr may use. Overridable for isolated test setups:
///
/// - `BEEMR_ISOLATED=1` disables all public networks (IPFS, UPnP, public DHT).
/// - `BEEMR_DHT_BOOTSTRAP=host:port,…` uses a private Mainline DHT instead.
/// - `BEEMR_DHT_BIND=127.0.0.1` binds the DHT socket to one address.
/// - `BEEMR_DHT_PORT=6881` fixes the DHT's UDP port (for test bootstrap nodes).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NetworkSettings {
    pub public: bool,
    pub dht_bootstrap: Option<Vec<String>>,
    pub dht_bind: Option<std::net::Ipv4Addr>,
    pub dht_port: Option<u16>,
}

impl NetworkSettings {
    pub fn from_env() -> Self {
        let isolated = std::env::var("BEEMR_ISOLATED").is_ok_and(|v| v == "1");
        let dht_bootstrap = std::env::var("BEEMR_DHT_BOOTSTRAP").ok().map(|list| {
            list.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect()
        });
        NetworkSettings {
            public: !isolated,
            dht_bootstrap,
            dht_bind: std::env::var("BEEMR_DHT_BIND")
                .ok()
                .and_then(|ip| ip.parse().ok()),
            dht_port: std::env::var("BEEMR_DHT_PORT")
                .ok()
                .and_then(|port| port.parse().ok()),
        }
    }
}

/// The configuration folder and the settings stored in it.
#[derive(Clone, Debug)]
pub struct Config {
    dir: PathBuf,
}

impl Config {
    /// Open the default folder (or `BEEMR_HOME`), creating it if needed.
    pub fn load() -> Result<Config> {
        Self::at(default_dir()?)
    }

    pub fn at(dir: PathBuf) -> Result<Config> {
        fs::create_dir_all(&dir).context(dir.display())?;
        Ok(Config { dir })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn path(&self, file: &str) -> PathBuf {
        self.dir.join(file)
    }

    /// This device's human-readable name, if one has been set.
    pub fn device_name(&self) -> Result<Option<String>> {
        Ok(read_optional(&self.path(NAME_FILE))?
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()))
    }

    pub fn set_device_name(&self, name: &str) -> Result<String> {
        let name = clean_name(name)?;
        let path = self.path(NAME_FILE);
        fs::write(&path, format!("{name}\n")).context(path.display())?;
        Ok(name)
    }

    /// Relays the user saved with `beemr relay use`.
    pub fn relays(&self) -> Result<Vec<Multiaddr>> {
        let text = read_optional(&self.path(RELAYS_FILE))?.unwrap_or_default();
        Ok(text
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .filter_map(|l| l.parse().ok())
            .collect())
    }

    pub fn set_relays(&self, relays: &[Multiaddr]) -> Result<()> {
        let path = self.path(RELAYS_FILE);
        let text: String = relays.iter().map(|r| format!("{r}\n")).collect();
        fs::write(&path, text).context(path.display())
    }
}

fn read_optional(path: &Path) -> Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(e) => Err(crate::Error::from(e).context(path.display())),
    }
}

/// Validate a device name: printable, single line, reasonably short.
pub fn clean_name(name: &str) -> Result<String> {
    let name: String = name.split_whitespace().collect::<Vec<_>>().join(" ");
    if name.is_empty() {
        bail!("the device name can't be empty");
    }
    if name.chars().count() > MAX_NAME_LEN {
        bail!("the device name can be at most {MAX_NAME_LEN} characters");
    }
    if name.chars().any(char::is_control) {
        bail!("the device name can't contain control characters");
    }
    Ok(name)
}

/// The platform's usual place for per-user app data.
fn default_dir() -> Result<PathBuf> {
    use std::env::var_os;
    if let Some(dir) = var_os("BEEMR_HOME") {
        return Ok(dir.into());
    }
    let base: Option<PathBuf> = if cfg!(windows) {
        var_os("APPDATA").map(Into::into)
    } else if cfg!(target_os = "macos") {
        var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support"))
    } else {
        var_os("XDG_CONFIG_HOME")
            .map(Into::into)
            .or_else(|| var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
    };
    base.map(|b| b.join("beemr"))
        .context("can't find a folder for beemr's settings; set BEEMR_HOME")
}

/// A friendly default name for this device, such as "Osman's Mac mini".
pub fn default_device_name() -> String {
    let from_command = |program: &str, args: &[&str]| {
        std::process::Command::new(program)
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|s| !s.is_empty())
    };
    let name = if cfg!(target_os = "macos") {
        from_command("scutil", &["--get", "ComputerName"])
    } else if cfg!(windows) {
        std::env::var("COMPUTERNAME").ok()
    } else {
        fs::read_to_string("/etc/hostname")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .or_else(|| from_command("hostname", &[]))
    };
    name.and_then(|n| clean_name(&n).ok())
        .unwrap_or_else(|| "My device".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_cleaned() {
        assert_eq!(clean_name("  Sara's   laptop ").unwrap(), "Sara's laptop");
        assert!(clean_name("   ").is_err());
        assert!(clean_name(&"x".repeat(MAX_NAME_LEN + 1)).is_err());
        assert!(clean_name("bad\u{7}name").is_err());
    }

    #[test]
    fn settings_round_trip() {
        let dir = std::env::temp_dir().join(format!(
            "beemr-test-config-{}",
            crate::util::hex_encode(&crate::util::random::<6>())
        ));
        let config = Config::at(dir.clone()).unwrap();
        assert_eq!(config.device_name().unwrap(), None);
        config.set_device_name("Work laptop").unwrap();
        assert_eq!(
            config.device_name().unwrap().as_deref(),
            Some("Work laptop")
        );

        let relay: Multiaddr = "/ip4/203.0.113.5/udp/4545/quic-v1".parse().unwrap();
        config.set_relays(std::slice::from_ref(&relay)).unwrap();
        assert_eq!(config.relays().unwrap(), vec![relay]);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn default_name_is_valid() {
        assert!(clean_name(&default_device_name()).is_ok());
    }
}
