//! Shared resolution facts; construction and assumptions are unchanged.
pub use oer_analysis_riscv_model::{Assumption, Fact, Resolution, Resolutions};

/// The dequeue calls whose task targets (including a proven empty queue)
/// are supplied by executor facts. No function name or address is a rule.
pub fn task_poll_sites(resolutions: &Resolutions) -> std::collections::BTreeSet<u32> {
    resolutions
        .iter()
        .filter(|(_, resolution)| {
            resolution.facts.contains(&Fact::TaskPolls) || resolution.facts.contains(&Fact::NoTasks)
        })
        .map(|(site, _)| site)
        .collect()
}
