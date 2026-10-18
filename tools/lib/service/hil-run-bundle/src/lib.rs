//! The HIL run bundle's writer, store and verification; its format and
//! reader are `oer-tool-data-hil-run-bundle-format`.
//!
//! - [`run`]: the writer [`run::RunSession`] that creates, seals and
//!   interrupts a run;
//! - [`store`]: [`store::RunStore`], the run store every checkout shares, its
//!   sidecars (pins, quarantine, performance baselines, caches).
//! - [`receipt`]: the receipt a runner process names its runs in.
//! - [`build`]: the content-addressed object store and source capture of a
//!   run's images; the image builder fills their provenance.
//! - [`verify`]: the offline verification of a whole bundle, its build
//!   records and its replays against the image builder's recipe.
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

pub mod build;
pub mod receipt;
pub mod run;
pub mod store;
pub mod verify;

pub use receipt::RunId;
pub use store::RunStore;

#[cfg(test)]
mod read_tests;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
