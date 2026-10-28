//! The typed vendor-comparison scenarios: a chip-neutral engine, the
//! chip's scenarios and the reviewer reports.
//!
//! The engine modules are chip-neutral: authenticated artifacts, Blobray
//! sessions, the comparison harness, observation and state analysis. A
//! chip's module ([`esp32s31`]) installs its [`chip::Chip`] and builds its
//! scenarios on them; chip addresses, pins and reviewed decisions stay
//! there. [`report`] holds the reviewer reports and the command line the
//! `oer-tool-vendor-scenario-cli` binary runs.

#![forbid(unsafe_code)]
pub mod artifacts;
pub mod chip;
pub mod discovery;
pub mod esp32s31;
pub mod evidence;
pub mod failure;
pub mod findings;
pub mod harness;
pub mod leaf;
pub mod observation;
pub mod phy;
pub mod registers;
pub mod report;
pub mod rule_use;
pub mod session;
pub mod state;

pub use chip::{Chip, Isa, chip, install};
