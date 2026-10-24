//! Interrupt classes: where each entry's class may sit, and what code of the
//! `Hardware` class does that the class forbids. The class is the image's
//! declaration in its table (`oer_interrupt_table::Class`), never a level
//! number or a handler's name.

use oer_analysis_riscv_stack::{Analysis, Class, Fact, Resolutions, TableEntry, task_poll_sites};
use std::collections::BTreeSet;
use std::fmt;

/// The class of one hart's level: its core's executor there makes it the
/// executor's level; otherwise its entries are hardware handlers.
pub(super) fn context_class(table: &[TableEntry], core: u32, level: u32) -> Class {
    if executors(table, core, level) == 0 {
        Class::Hardware
    } else {
        Class::Executor
    }
}

fn executors(table: &[TableEntry], core: u32, level: u32) -> usize {
    table
        .iter()
        .filter(|entry| {
            entry.handler.is_some()
                && entry.core == core
                && entry.level == level
                && entry.class == Class::Executor
        })
        .count()
}

/// The entries of the image's build that break where their class may sit:
/// two executors on one level and core, a hardware handler on its core's
/// executor level, an exempt entry with no executor beside it. The table's
/// macro refuses each at compile time; the image is checked again here.
pub(super) fn misplaced(table: &[TableEntry]) -> Vec<String> {
    table
        .iter()
        .filter(|entry| entry.handler.is_some())
        .filter_map(|entry| {
            let (source, level, core) = (entry.source, entry.level, entry.core);
            match (entry.class, executors(table, core, level)) {
                (Class::Executor, count) if count > 1 => Some(format!(
                    "source {source} is one of {count} executors of level {level} on core {core}"
                )),
                (Class::Hardware, count) if count > 0 => Some(format!(
                    "the hardware handler of source {source} shares level {level} with core \
                     {core}'s executor"
                )),
                (Class::Exempt, 0) => Some(format!(
                    "source {source} is exempt, but no executor of core {core} shares its level \
                     {level}"
                )),
                _ => None,
            }
        })
        .collect()
}

/// What hardware code does that its class forbids.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Breach {
    /// A call that polls an executor's tasks.
    TaskPoll,
    /// A call through a waker's vtable.
    WakerCall,
    /// A static reference to data at this address in cached memory.
    CachedData(u64),
}

impl fmt::Display for Breach {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TaskPoll => out.write_str("polls tasks"),
            Self::WakerCall => out.write_str("calls a waker"),
            Self::CachedData(target) => write!(out, "references cached data at {target:#010x}"),
        }
    }
}

/// One breach of the hardware class.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Finding {
    /// The context or exempt entry whose code breaches it.
    pub subject: String,
    pub function: u32,
    pub site: u32,
    pub breach: Breach,
}

/// The breaches of the hardware class in `reached`, the code of `subject`
/// under its context's `resolutions`; `data` are the static references from
/// that code into cached memory (function, site, target), code targets
/// excluded.
pub(super) fn hardware_code(
    subject: &str,
    analysis: &Analysis,
    reached: &BTreeSet<u32>,
    resolutions: &Resolutions,
    data: &[(u32, u32, u64)],
) -> Vec<Finding> {
    let polls = task_poll_sites(resolutions);
    let finding = |function, site, breach| Finding {
        subject: subject.to_owned(),
        function,
        site,
        breach,
    };
    let mut findings: Vec<Finding> = reached
        .iter()
        .filter_map(|address| analysis.functions.get(address))
        .flat_map(|facts| {
            facts
                .transfers
                .iter()
                .map(move |transfer| (facts.function.address, transfer.site))
        })
        .filter_map(|(function, site)| {
            if polls.contains(&site) {
                Some(finding(function, site, Breach::TaskPoll))
            } else if resolutions
                .get(site)
                .is_some_and(|resolution| resolution.facts.contains(&Fact::WakerVtables))
            {
                Some(finding(function, site, Breach::WakerCall))
            } else {
                None
            }
        })
        .chain(
            data.iter()
                .filter(|(function, _, _)| reached.contains(function))
                .map(|&(function, site, target)| {
                    finding(function, site, Breach::CachedData(target))
                }),
        )
        .collect();
    findings.sort();
    findings.dedup();
    findings
}

#[cfg(test)]
mod tests;
