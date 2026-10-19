//! HIL workloads: what every radio family's workload runs on.
//!
//! - [`context::Context`]: one repetition's laboratory, target settings,
//!   captures, measurements and typed results;
//! - [`fixture::Fixtures`]: the type-erased set of fixtures the composed
//!   [`family::FixtureProvider`]s prepared for the repetition, and the
//!   repetition's cleanup scope ([`fixture::cleanup`]);
//! - [`boots::for_each_boot`]: the per-boot loop, each boot's typed
//!   observation recorded through [`results::Results`], the one writer of a
//!   repetition's `observations.json`;
//! - [`require_keys`]: the image-key gate of a workload;
//! - [`measurements`]: the repetition's measurements and the projection of
//!   each capture's decoded messages into them;
//! - [`failure::classify`]: scenario or infrastructure failure;
//! - [`family`]: the trait a radio family implements and the registry the
//!   runner composes, so the runner core dispatches without naming a family.
//!
//! Radio-family workloads and their fixtures live in the family packages
//! that depend on this crate; the `oer-hil-runner` binary composes them. A
//! step lives here only when several families run it against their own
//! clients and no family owns it.
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

pub mod boots;
pub mod context;
pub mod failure;
pub mod family;
pub mod fixture;
mod keys;
pub mod measurements;
pub mod output;
pub mod results;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
pub use keys::{IMAGE_KEYS_TIMEOUT, require_keys};
pub use output::emit_json;
