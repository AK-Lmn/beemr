//! Everything about this device that commands need: identity, name, contacts
//! and network settings.

use crate::config::{default_device_name, Config, NetworkSettings};
use crate::identity::{Contacts, Identity};
use crate::Result;

pub struct Profile {
    pub config: Config,
    pub identity: Identity,
    /// This device's human-readable name.
    pub name: String,
    pub contacts: Contacts,
    pub network: NetworkSettings,
    /// Whether the identity was created just now (first run).
    pub created: bool,
}

impl Profile {
    pub fn load() -> Result<Profile> {
        Self::load_from(Config::load()?)
    }

    pub fn load_from(config: Config) -> Result<Profile> {
        let (identity, created) = Identity::load_or_create(&config)?;
        let name = match config.device_name()? {
            Some(name) => name,
            None => default_device_name(),
        };
        Ok(Profile {
            contacts: Contacts::load(&config)?,
            network: NetworkSettings::from_env(),
            identity,
            name,
            config,
            created,
        })
    }
}
