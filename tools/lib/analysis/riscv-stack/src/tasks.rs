//! Executor task polls. Embassy's executor runs a task through the
//! `poll_fn` its header holds: the call it leaves unresolved where the DWARF
//! says `SyncExecutor::poll`'s dequeue closure is inlined reaches every
//! task's `TaskStorage<F>::poll` of the image, and `poll_exited`, the
//! `poll_fn` of a task that has returned. A `poll_fn` is set only by its
//! task's spawn, so this is every function the site can reach under the
//! executor invariant. An executor running on an interrupt stack (the radio
//! executor) thereby bounds its tasks' polls on that stack.
use crate::dwarf::Dwarf;
use crate::{Analysis, Fact, Resolutions, TransferKind};
use oer_tool_analysis_riscv_model::{Ledger, Result, RunControl};
use std::collections::BTreeSet;

/// The closure of `SyncExecutor::poll` that calls each dequeued task's
/// `poll_fn`.
const DEQUEUE: &str = "<embassy_executor::raw::SyncExecutor>::poll::{closure#0}";

/// Whether `name`, demangled, is a function a task header's `poll_fn` holds.
fn is_task_poll(name: &str) -> bool {
    (name.starts_with("<embassy_executor::raw::TaskStorage<") && name.ends_with(">>::poll"))
        || name == "embassy_executor::raw::poll_exited"
}

/// The targets of every unresolved call inside the executor's dequeue
/// closure: each task poll among `functions` (address, demangled name).
pub fn task_poll_resolutions<'a>(
    analysis: &Analysis,
    dwarf: &Dwarf,
    functions: impl IntoIterator<Item = (u32, &'a str)>,
    keep: &mut Ledger<'_>,
    control: &mut dyn RunControl,
) -> Result<Resolutions> {
    let polls: BTreeSet<u32> = functions
        .into_iter()
        .filter(|(address, name)| is_task_poll(name) && analysis.functions.contains_key(address))
        .map(|(address, _)| address)
        .collect();
    let mut resolutions = Resolutions::new();
    if polls.is_empty() {
        return Ok(resolutions);
    }
    for facts in analysis.functions.values() {
        for transfer in &facts.transfers {
            if transfer.target.is_some()
                || !matches!(transfer.kind, TransferKind::Call | TransferKind::Tail)
            {
                continue;
            }
            control.checkpoint(1)?;
            let chain = dwarf.inline_chain(transfer.site)?;
            if chain.first().map(String::as_str) != Some(DEQUEUE) {
                continue;
            }
            keep.admit(Resolutions::admission(polls.len()), control)?;
            resolutions.add(transfer.site, Fact::TaskPolls, polls.iter().copied());
        }
    }
    Ok(resolutions)
}

#[cfg(test)]
mod tests {
    use super::is_task_poll;

    #[test]
    fn a_task_storage_poll_and_the_exited_poll_are_task_polls() {
        assert!(is_task_poll(
            "<embassy_executor::raw::TaskStorage<app::__radio_actor_task::__radio_actor_task_inner_function::{closure#0}>>::poll"
        ));
        assert!(is_task_poll("embassy_executor::raw::poll_exited"));
        assert!(!is_task_poll("<embassy_executor::raw::SyncExecutor>::poll"));
        assert!(!is_task_poll(
            "<embassy_executor::raw::TaskStorage<app::Task>>::spawn"
        ));
    }
}
