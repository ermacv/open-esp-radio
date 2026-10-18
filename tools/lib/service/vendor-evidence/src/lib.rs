//! The native vendor-comparison evidence index: a directory of shards, one
//! per scenario, so each scenario run rewrites only its own file. A scenario
//! writes its shard after it passed; qualification consumes the directory.
//!
//! The shard format, its reader and writer are `oer-tool-data-vendor-evidence-shard`.
//!
//! - [`policy`]: the verdict source policy — the path closure a shard may
//!   record, without the report packages that decide no verdict;
//! - [`diff`]: what two versions of one shard say differently;
//! - `baseline` (feature `producers`): which claimed vendor roots a change
//!   loses or narrows against the index `main` published, and a chip's
//!   reviewed acceptances of them;
//! - [`producer`]: the contract of whatever compares vendor code with
//!   compiled production code and writes shards (the Blobray scenario
//!   engine, a host stand), and [`producer::host_stand`], the host stands'
//!   shard;
//! - `run` (feature `producers`): building the probes and the Blobray
//!   scenarios, regenerating and checking a chip's shards.
//!
//! Its dependencies stay light (the repository model, serde, SHA-256), so
//! the Blobray workspace, the host stands, xtask and the qualification
//! evaluator all read and write shards through it.
#![forbid(unsafe_code)]

#[cfg(feature = "producers")]
pub mod baseline;
pub mod diff;
pub mod policy;
pub mod producer;
#[cfg(feature = "producers")]
pub mod run;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
