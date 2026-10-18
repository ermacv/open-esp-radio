//! Wi-Fi radio evidence, analyzed from what the host observed independently
//! of the target: the typed frames of a monitor capture ([`air`]), how a
//! station follows BSS protection on the air ([`protection`]) and the
//! assessment of a session's receive delivery against the datagrams the host
//! sent ([`rx_delivery`]).
//!
//! The Wi-Fi fixtures produce the captures and the workloads judge with
//! these analyses; neither owns them.
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

pub mod air;
pub mod protection;
pub mod rx_delivery;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
