//! Repository commands behind `cargo xtask` that a person runs by hand to
//! build or test: API documentation, locks, worktrees, push, and the
//! compile-fail and feature-set tests. Domain work (images, vendor evidence,
//! registers) is its owners'; the binary parses arguments and calls them.

pub mod cargo;
pub mod checks;
pub mod doc;
pub mod graph;
pub mod push;
pub mod report;
pub mod worktree;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
