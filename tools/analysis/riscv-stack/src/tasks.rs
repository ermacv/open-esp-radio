//! Executor task polls. The executor (`oer-espressif-executor`) polls each
//! declared task by a direct call of `task::poll_task::<F>`, its boundary
//! with the task's code: every call of an instance is a task poll, and the
//! instance is the task's root. The calls are direct, so the polls assume
//! nothing; the boundary's name only marks where a context's own code ends.
use crate::{Analysis, Resolutions, TransferKind};
use oer_analysis_riscv_model::{Ledger, Result, RunControl};
use std::collections::BTreeSet;

/// The executor's boundary with a task's code.
const POLL_TASK: &str = "oer_espressif_executor::task::poll_task";

/// Whether `name`, demangled, is an instance of the executor's task poll.
pub fn is_task_poll(name: &str) -> bool {
    name.strip_prefix(POLL_TASK)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with("::<"))
}

/// Every direct call of a task poll among `functions` (address, demangled
/// name), as task poll sites.
pub fn task_poll_resolutions<'a>(
    analysis: &Analysis,
    functions: impl IntoIterator<Item = (u32, &'a str)>,
    keep: &mut Ledger<'_>,
    control: &mut dyn RunControl,
) -> Result<Resolutions> {
    let mut polls = BTreeSet::new();
    for (address, name) in functions {
        control.checkpoint(1)?;
        if is_task_poll(name) {
            polls.insert(address);
        }
    }
    let mut resolutions = Resolutions::new();
    for facts in analysis.functions.values() {
        for transfer in &facts.transfers {
            control.checkpoint(1)?;
            if matches!(transfer.kind, TransferKind::Call | TransferKind::Tail)
                && transfer
                    .target
                    .is_some_and(|target| polls.contains(&target))
            {
                keep.admit(size_of::<u32>(), control)?;
                resolutions.add_task_poll(transfer.site);
            }
        }
    }
    Ok(resolutions)
}

#[cfg(test)]
mod tests {
    use super::is_task_poll;

    #[test]
    fn only_the_executors_task_poll_is_a_task_poll() {
        assert!(is_task_poll(
            "oer_espressif_executor::task::poll_task::<oer_firmware_apps_w0::radio_actor::{closure#0}>"
        ));
        assert!(is_task_poll("oer_espressif_executor::task::poll_task"));
        assert!(!is_task_poll("oer_espressif_executor::task::poll_tasks"));
        assert!(!is_task_poll(
            "<oer_espressif_executor::task::Task<F, T>>::poll"
        ));
        assert!(!is_task_poll(
            "<embassy_executor::raw::TaskStorage<app::Task>>::poll"
        ));
    }
}
