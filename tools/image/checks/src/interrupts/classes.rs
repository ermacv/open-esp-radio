//! Interrupt classes: where each entry's class may sit, and what code of the
//! `Hardware` class does that the class forbids. The class is the image's
//! declaration in its table (`oer_espressif_interrupt_table::Class`), never a level
//! number or a handler's name.

use oer_analysis_riscv_stack::{
    Analysis, Class, Fact, Resolutions, TableEntry, Violation, task_poll_sites,
};
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
                (Class::Hardware | Class::Exempt(Violation::WakerCall), count) if count > 0 => {
                    Some(format!(
                        "the hardware handler of source {source} shares level {level} with core \
                         {core}'s executor"
                    ))
                }
                (Class::Exempt(Violation::ExecutorLevel), 0) => Some(format!(
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
}

impl fmt::Display for Breach {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.write_str(match self {
            Self::TaskPoll => "polls tasks",
            Self::WakerCall => "calls a waker",
        })
    }
}

/// The data objects in cached memory that one context's strict code (or an
/// exempt entry's handler) references: reported, not gated.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CachedData {
    /// The context or exempt entry.
    pub subject: String,
    /// Static references (instruction sites) into cached memory.
    pub references: usize,
    /// Objects in writable sections (`.data`, `.bss`), by name; an address
    /// for a target no sized symbol covers.
    pub writable: BTreeSet<String>,
    /// Objects in read-only sections (`.rodata`: constants, panic arguments,
    /// switch tables).
    pub read_only: BTreeSet<String>,
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
/// under its context's `resolutions`.
pub(super) fn hardware_code(
    subject: &str,
    analysis: &Analysis,
    reached: &BTreeSet<u32>,
    resolutions: &Resolutions,
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
        .collect();
    findings.sort();
    findings.dedup();
    findings
}

/// Split `found`, the breaches of a hardware level's code, into those that
/// fail and those an exemption waives: a breach is waived only where the
/// level's dispatch path (`prefix`) does not reach its function, some entry
/// of the level does, and every entry that reaches it is exempt from exactly
/// that breach.
pub(super) fn waive(
    found: Vec<Finding>,
    prefix: &BTreeSet<u32>,
    entries: &[(Class, BTreeSet<u32>)],
) -> (Vec<Finding>, Vec<Finding>) {
    let waives = |class: Class, breach: Breach| {
        matches!(
            (class, breach),
            (Class::Exempt(Violation::WakerCall), Breach::WakerCall)
        )
    };
    found.into_iter().partition(|finding| {
        let mut reachers = entries
            .iter()
            .filter(|(_, reached)| reached.contains(&finding.function))
            .peekable();
        prefix.contains(&finding.function)
            || reachers.peek().is_none()
            || reachers.any(|&(class, _)| !waives(class, finding.breach))
    })
}

/// [`CachedData`] of `subject` from its static references into cached
/// memory, code targets excluded: each target named by the smallest sized
/// data symbol covering it, else by its address.
pub(super) fn cached_data(
    subject: String,
    elf: &oer_analysis_elf::Elf<'_>,
    references: &[(u32, u32, u64)],
) -> CachedData {
    let mut data = CachedData {
        subject,
        references: references.len(),
        ..CachedData::default()
    };
    for &(_, _, target) in references {
        let name = elf
            .symbols()
            .filter(|symbol| {
                symbol.defined
                    && symbol.size > 0
                    && symbol.kind == oer_analysis_elf::SymbolKind::Data
                    && target
                        .checked_sub(symbol.address)
                        .is_some_and(|offset| offset < symbol.size)
            })
            .min_by_key(|symbol| (symbol.size, symbol.address))
            .map_or_else(|| format!("{target:#010x}"), |symbol| symbol.demangled());
        let writable = elf
            .sections()
            .find(|section| section.allocated && section.contains(target))
            .is_some_and(|section| section.writable);
        if writable {
            data.writable.insert(name);
        } else {
            data.read_only.insert(name);
        }
    }
    data
}

#[cfg(test)]
mod tests;
