//! HIL experiments: launching runs, A/B comparisons and bisection.
//!
//! - [`launch`]: [`launch::launch_run`], the one way a run is launched,
//!   whose runs come back from the runner's receipt.
//! - [`job`]: the arbiter job a launch runs as.
//! - [`ab`]: two firmware variants on the same scenarios, compared with
//!   [`oer_tool_service_hil_analysis::arms`].
//! - [`revision`]: a revision's scenario set with the runner of its HIL
//!   wire, its own when the wire differs from this checkout's.
//! - [`bisect`]: the first commit at which a scenario stops passing.
//!
//! A revision is checked out with [`oer_tool_base_process::git::Worktree`], the one
//! worktree helper. Nothing here runs `cargo hil`.
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

pub mod ab;
pub mod bisect;
pub mod job;
pub mod launch;
pub mod revision;

#[cfg(test)]
mod lease_tests;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
