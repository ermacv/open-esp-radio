//! Shared resolution facts; construction and assumptions are unchanged.
pub use oer_analysis_riscv_model::{Assumption, Fact, Resolution, Resolutions};

/// The direct calls by which an executor polls its tasks
/// ([`crate::task_poll_resolutions`]).
pub fn task_poll_sites(resolutions: &Resolutions) -> std::collections::BTreeSet<u32> {
    resolutions.task_polls().clone()
}
