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

/// The existing IPC-post fact, shared by image gates and context diagnostics.
/// The platform selects the posting and dispatch functions; machine call
/// arguments determine every posted callback. No address interval is supplied.
pub fn ipc_resolutions(
    analysis: &crate::Analysis,
    taken: &std::collections::BTreeSet<u32>,
    posts: impl IntoIterator<Item = u32>,
    argument: usize,
    dispatches: impl IntoIterator<Item = u32>,
    keep: &mut oer_analysis_riscv_model::Ledger<'_>,
    control: &mut dyn oer_analysis_riscv_model::RunControl,
) -> oer_analysis_riscv_model::Result<Resolutions> {
    let mut targets = std::collections::BTreeSet::new();
    for post in posts {
        control.checkpoint(1)?;
        if taken.contains(&post) {
            return Err(oer_analysis_riscv_model::Error::new(
                oer_analysis_riscv_model::ErrorCode::Integrity,
                format!("IPC post {post:#010x} is called through a pointer"),
            ));
        }
        targets.extend(analysis.constant_arguments(post, argument)?);
    }
    let mut resolutions = Resolutions::new();
    for function in dispatches {
        control.checkpoint(1)?;
        if let Some(function) = analysis.functions.get(&function) {
            for transfer in &function.transfers {
                control.checkpoint(1)?;
                if transfer.target.is_none() {
                    keep.admit(Resolutions::admission(targets.len()), control)?;
                    resolutions.add(transfer.site, Fact::IpcPosts, targets.iter().copied());
                }
            }
        }
    }
    Ok(resolutions)
}
