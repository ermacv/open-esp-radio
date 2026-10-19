//! The HIL run bundle's writer and store; its format and reader are
//! `oer-hil-run-bundle-format`.
//!
//! - [`run`]: the writer [`run::RunSession`] that creates, finishes and
//!   interrupts a run;
//! - [`store`]: [`store::RunStore`], the run store every checkout shares, its
//!   sidecars (pins, quarantine, performance baselines, caches).
//! - [`launch_runs`]: the list a runner process names its runs in.
//! - [`build`]: the content-addressed object store and source capture of a
//!   run's images; the image builder fills their provenance.
//! - [`replay`]: the archived firmware of an earlier run, its build records
//!   checked against the image builder's recipe before a run replays it.
#![forbid(unsafe_code)]

pub mod format;

pub mod build;
pub mod launch_runs;
pub mod replay;
pub mod run;
pub mod store;

pub use launch_runs::RunId;
pub use store::RunStore;

#[cfg(test)]
mod read_tests;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
