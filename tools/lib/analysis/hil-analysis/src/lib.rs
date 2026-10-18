//! Analysis of HIL run bundles, read through [`oer_hil_run_bundle_format::RunBundle`].
//!
//! - [`run`]: one run as the analyses read it, its bundle and suite.
//! - [`samples`]: the one aggregation of a run's typed measurements
//!   ([`samples::samples`]), the one comparison of two sets of values
//!   ([`samples::compare`], noise-aware) and the one judgement against a
//!   baseline ([`samples::change`]).
//! - [`runs`]: `cargo hil runs` queries (list, show, why, compare, history,
//!   flaky) and following a run to its end.
//! - [`retention`]: the prune rule and the store's size budget.
//! - [`perf`]: gated measurements across commits and their reviewed
//!   baselines.
//! - [`arms`]: the comparison of an A/B experiment's arms.
//! - [`dashboard`]: the runs of the stand's live page.
//! - [`report`]: the JUnit and HTML views a run seals beside its suite.
//! - [`profile`]: the symbolized report of a repetition's program-counter
//!   profile.
//!
//! Nothing here decides qualification.
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

pub mod arms;
pub mod dashboard;
pub mod perf;
pub mod profile;
pub mod report;
pub mod retention;
pub mod run;
pub mod runs;
pub mod samples;

pub use run::Run;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
