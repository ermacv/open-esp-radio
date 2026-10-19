//! The `cargo hil` command line: argument parsing and calls into the stand
//! libraries (leases, owners, jobs), the ESP-IDF firmware catalog,
//! the run store's analyses (`oer-hil-analysis`), experiments and the
//! launch of the runner (`oer-hil-experiment`).
#![forbid(unsafe_code)]

pub mod experiment;

pub mod command;
pub mod dashboard;
pub mod experiments;
pub mod firmware_catalog;
pub mod images;
pub mod jobs;
pub mod peer;
pub(crate) mod pins;
pub mod sweep;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
