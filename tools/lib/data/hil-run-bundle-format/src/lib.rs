//! The HIL run bundle format: its documents and its one reader.
//!
//! - [`run`]: the bundle's typed documents, their structural validation and
//!   file listings;
//! - [`read`]: [`read::RunBundle`], the typed reader every consumer uses;
//! - [`build`]: the build provenance of a run's images;
//! - [`experiment`], [`lab`]: the A/B experiment and the laboratory cell a
//!   run records;
//! - [`observer`]: the observer's build identity a run records;
//!
//! The writer, the run store and the replay checks are
//! `oer-tool-service-hil-run-bundle`'s.
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

pub mod archived;
pub mod build;
pub mod experiment;
pub mod lab;
pub mod observer;
pub mod read;
pub mod run;

pub use read::RunBundle;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
