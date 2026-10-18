//! The vendor-comparison evidence shard format: a directory of shards, one
//! per scenario, so each scenario run rewrites only its own file.
//!
//! - [`index`]: the shard ([`Index`]), its schema, validation and whether
//!   its recorded sources are still current, and the cross-scenario views
//!   of a whole directory ([`Evidence`]);
//! - [`store`]: the one reader and writer of shard files, and which shards
//!   of a directory are stale.
#![forbid(unsafe_code)]

pub mod index;
pub mod store;

pub use index::*;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
