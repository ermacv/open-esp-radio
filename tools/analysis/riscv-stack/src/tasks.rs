//! Executor task polls. Each task the executor (`oer-espressif-executor`)
//! declares exports its poll boundary: a `#[used]` record named
//! `__OER_TASK_POLL.<module path>` whose one word, relocated, is the address
//! of the task's instance of `task::poll_task`, the never-inlined function
//! through which the executor polls the task (`task::PollBoundary`).
//!
//! Every direct call of a recorded boundary is a task poll, and the boundary
//! is the task's root. The records, not function names, mark where a
//! context's own code ends, and the analysis fails rather than find fewer
//! boundaries than the image declares: a record whose boundary no direct
//! call reaches (the poll was inlined, or the task is never polled), or an
//! image that links the executor's waker and records no boundary, is an
//! error. The calls are direct, so the polls assume nothing.
use crate::{Analysis, Resolutions, TransferKind};
use oer_analysis_riscv_model::{Error, ErrorCode, Ledger, Result, RunControl};
use oer_analysis_riscv_program::facts::ImageFacts;
use std::collections::BTreeMap;

/// The name prefix of every task's exported poll-boundary record.
pub const TASK_POLL_RECORD: &str = "__OER_TASK_POLL.";

/// The task polls of `image`: the direct calls of the boundaries its records
/// name, each boundary a task root. `executor` says whether the image links
/// the executor's waker, which polls tasks only through recorded boundaries.
///
/// # Errors
/// A record that is not one relocated read-only word naming a function, a
/// boundary no direct call reaches, or an image with the executor's waker
/// and no record.
pub fn task_poll_resolutions(
    image: &ImageFacts<'_>,
    analysis: &Analysis,
    executor: bool,
    keep: &mut Ledger<'_>,
    control: &mut dyn RunControl,
) -> Result<Resolutions> {
    let invalid = |message: String| Error::new(ErrorCode::Integrity, message);
    // Each boundary, by address, with the first record naming it.
    let mut boundaries = BTreeMap::new();
    for symbol in image.file().symbols() {
        control.checkpoint(1)?;
        if !symbol.name.starts_with(TASK_POLL_RECORD) {
            continue;
        }
        let name = symbol.name;
        let at = u32::try_from(symbol.address)
            .map_err(|_| invalid(format!("`{name}` lies beyond RV32")))?;
        let word = image
            .read_only_word(at)
            .ok_or_else(|| invalid(format!("`{name}` at {at:#010x} is not read-only data")))?;
        if image.word(at) != Some(word) {
            return Err(invalid(format!(
                "`{name}`'s boundary word at {at:#010x} has no relocation"
            )));
        }
        let boundary = word & !1;
        if !analysis.functions.contains_key(&boundary) {
            return Err(invalid(format!(
                "`{name}` names {boundary:#010x}, which starts no function"
            )));
        }
        if let std::collections::btree_map::Entry::Vacant(entry) = boundaries.entry(boundary) {
            keep.admit(size_of::<(u32, (String, bool))>() + name.len(), control)?;
            entry.insert((name.to_owned(), false));
        }
    }
    if executor && boundaries.is_empty() {
        return Err(invalid(format!(
            "the image links the executor's waker but records no task poll boundary \
             (`{TASK_POLL_RECORD}*`): its task polls cannot be found"
        )));
    }
    let mut resolutions = Resolutions::new();
    for facts in analysis.functions.values() {
        for transfer in &facts.transfers {
            control.checkpoint(1)?;
            if !matches!(transfer.kind, TransferKind::Call | TransferKind::Tail) {
                continue;
            }
            if let Some((_, called)) = transfer
                .target
                .and_then(|target| boundaries.get_mut(&target))
            {
                *called = true;
                keep.admit(size_of::<u32>(), control)?;
                resolutions.add_task_poll(transfer.site);
            }
        }
    }
    for (&boundary, (name, called)) in &boundaries {
        if !called {
            return Err(invalid(format!(
                "task poll boundary `{name}` ({boundary:#010x}) has no direct call: the \
                 executor's poll of the task was inlined, or the task is never polled"
            )));
        }
        keep.admit(size_of::<u32>(), control)?;
        resolutions.add_task_root(boundary);
    }
    Ok(resolutions)
}
