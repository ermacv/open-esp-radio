//! HIL experiments: launching runs, A/B comparisons and bisection.
//!
//! - [`launch`]: [`launch::launch_run`], the one way a run is launched,
//!   whose runs come back from the runner's launch run list.
//! - [`job`]: the arbiter job a launch runs as.
//! - [`ab`]: two firmware variants on the same scenarios, compared with
//!   [`oer_hil_analysis::arms`].
//! - [`bisect`]: one step of a manual bisection, for `git bisect run`.
//!
//! A revision is checked out with [`oer_os_process::git::Worktree`], the one
//! worktree helper. Nothing here runs `cargo hil`.
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

pub mod ab;
pub mod bisect;
pub mod job;
pub mod launch;

#[cfg(test)]
mod lease_tests;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
