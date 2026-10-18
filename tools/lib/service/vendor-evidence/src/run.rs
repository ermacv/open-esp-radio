//! The producers' orchestration, behind the `producers` feature so the
//! Blobray workspace and the host stands keep the light format crate:
//!
//! - [`probes`]: each chip's Rust comparison probe images, built and their
//!   probe catalogs validated;
//! - [`scenario`]: the typed Blobray vendor scenarios, built in the Blobray
//!   workspace and run one at a time;
//! - [`regenerate`]: which producer computes which shard of the derived
//!   index, whole or named, and the untriaged listing.
//!
//! Each long step prints a `phase <name>: <seconds> s` line ([`phase`]).

pub mod phase;
pub mod probes;
pub mod regenerate;
pub mod scenario;
