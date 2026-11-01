//! Executor task polls. The executor (`oer-espressif-executor`) polls each
//! declared task through its instance of `task::poll_task`, never inlined,
//! whose zero-sized first parameter has the type [`TASK_POLL_MARK`]
//! (`task::PollBoundary`). The image's DWARF names that type for every
//! instance: no function name, symbol or kept record decides a boundary.
//!
//! Every direct call of a boundary is a task poll, and the boundary is the
//! task's root. The analysis fails rather than find fewer boundaries than
//! the code polls: an inlined copy of a boundary (its task's code would
//! count as the executor's), a boundary no direct call reaches, or an image
//! that links the executor's waker and has no boundary, is an error. A task
//! set whose executor the image never runs links no boundary. The calls are
//! direct, so the polls assume nothing.
use crate::{Analysis, Dwarf, Resolutions, TransferKind};
use oer_analysis_riscv_model::{Error, ErrorCode, Ledger, Result, RunControl};
use std::collections::BTreeMap;

/// The DWARF path of the type that marks a task poll boundary: the
/// zero-sized first parameter of the executor's `task::poll_task`.
pub const TASK_POLL_MARK: &str = "oer_espressif_executor::task::PollBoundary";

/// The task polls of the image `analysis` and `dwarf` describe: the direct
/// calls of the boundaries the DWARF marks, each boundary a task root.
/// `executor` says whether the image links the executor's waker, which
/// polls tasks only through boundaries.
///
/// # Errors
/// An inlined boundary, a marked copy that starts no function, a boundary
/// no direct call reaches, or an image with the executor's waker and no
/// boundary.
pub fn task_poll_resolutions(
    analysis: &Analysis,
    dwarf: &Dwarf,
    executor: bool,
    keep: &mut Ledger<'_>,
    control: &mut dyn RunControl,
) -> Result<Resolutions> {
    let invalid = |message: String| Error::new(ErrorCode::Integrity, message);
    let name = |address: u32| {
        analysis
            .functions
            .get(&address)
            .and_then(|facts| facts.function.names.first())
            .map_or_else(
                || format!("{address:#010x}"),
                |name| format!("`{}` ({address:#010x})", oer_analysis_elf::demangle(name)),
            )
    };
    let marked = dwarf.poll_boundaries();
    if let Some(&start) = marked.inlined.first() {
        let chain = dwarf.inline_chain(start)?;
        return Err(invalid(format!(
            "a task poll boundary is inlined at {start:#010x} ({}): its task's code \
             would count as the executor's",
            chain.join(" in ")
        )));
    }
    // Each boundary, by address, and whether a direct call reaches it.
    let mut boundaries = BTreeMap::new();
    for &boundary in &marked.out_of_line {
        control.checkpoint(1)?;
        if !analysis.functions.contains_key(&boundary) {
            return Err(invalid(format!(
                "the DWARF marks a task poll boundary at {boundary:#010x}, which starts \
                 no function"
            )));
        }
        keep.insert(&mut boundaries, boundary, false, control)?;
    }
    if executor && boundaries.is_empty() {
        return Err(invalid(format!(
            "the image links the executor's waker but its DWARF marks no task poll \
             boundary (a parameter of `{TASK_POLL_MARK}`): its task polls cannot be found"
        )));
    }
    let mut resolutions = Resolutions::new();
    for facts in analysis.functions.values() {
        for transfer in &facts.transfers {
            control.checkpoint(1)?;
            if !matches!(transfer.kind, TransferKind::Call | TransferKind::Tail) {
                continue;
            }
            if let Some(called) = transfer
                .target
                .and_then(|target| boundaries.get_mut(&target))
            {
                *called = true;
                keep.admit(size_of::<u32>(), control)?;
                resolutions.add_task_poll(transfer.site);
            }
        }
    }
    for (&boundary, &called) in &boundaries {
        if !called {
            return Err(invalid(format!(
                "task poll boundary {} has no direct call: the executor polls the task \
                 another way",
                name(boundary)
            )));
        }
        keep.admit(size_of::<u32>(), control)?;
        resolutions.add_task_root(boundary);
    }
    Ok(resolutions)
}
