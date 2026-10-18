//! The stand (README.md): the owner's stand file, the OpenWrt lab router
//! that serves the stand's access point, and the SSH that reaches it.
//!
//! Every value of the stand file that is secret (an SSID, the SSH target, an
//! interface name, a passphrase) and every full MAC address stays out of
//! messages, errors and `Debug`.

#![forbid(unsafe_code)]

pub mod hub;
pub mod lock;
pub mod mac;
pub mod router;
pub mod ssh;
pub mod stand;

pub type Error = Box<dyn std::error::Error + Send + Sync>;
pub type Result<T> = std::result::Result<T, Error>;
