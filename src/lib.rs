//! beemr: direct, end-to-end encrypted file sharing between devices.
//!
//! There are no servers. Devices connect directly when they can (same network,
//! IPv6, a UPnP-opened port), punch through NATs with the help of public relays
//! when they can't, and fall back to relaying through other beemr devices.
//! Devices find each other through signed records on the BitTorrent Mainline DHT.
//!
//! See `PROTOCOL.md` for the wire format.

#![forbid(unsafe_code)]

/// Return early with a formatted [`Error`].
macro_rules! bail {
    ($($arg:tt)*) => {
        return Err($crate::Error::new(format!($($arg)*)))
    };
}

pub mod config;
pub mod connect;
pub mod crypto;
pub mod discovery;
pub mod doctor;
mod error;
pub mod get;
pub mod identity;
pub mod nat;
pub mod node;
pub mod path;
mod portmap;
pub mod profile;
mod progress;
pub mod proto;
pub mod relay;
pub mod service;
pub mod share;
pub mod ticket;
pub mod util;

pub use error::{Context, Error, Result};
