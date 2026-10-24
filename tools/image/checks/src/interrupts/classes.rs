//! Interrupt classes: where each entry's class may sit, and what code of the
//! `Hardware` class does that the class forbids. The class is the image's
//! declaration in its table (`oer_interrupt_table::Class`), never a level
//! number or a handler's name.

use oer_analysis_riscv_stack::{
    Analysis, Class, Fact, Resolutions, TableEntry, Violation, task_poll_sites,
};
use std::collections::{BTreeMap, BTreeSet};
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
/// exempt entry's handler) references. The image's own objects fail a
/// product image; the platform's and panic arguments are reported.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CachedData {
    /// The context or exempt entry.
    pub subject: String,
    /// Static references (instruction sites) into cached memory.
    pub references: usize,
    /// The image's own objects in writable sections (`.data`, `.bss`), by
    /// name; an address for a target no sized symbol covers.
    pub writable: BTreeSet<String>,
    /// The image's own objects in read-only sections (`.rodata`: constants,
    /// switch tables), panic arguments apart.
    pub read_only: BTreeSet<String>,
    /// Objects only the esp-hal fork's own functions reference (see
    /// [`PLATFORM_CRATES`]).
    pub platform: BTreeSet<String>,
    /// Read-only objects only calls into the panic machinery take
    /// (`Location`s, messages).
    pub panic_arguments: BTreeSet<String>,
    /// The symbols each own object's input section defines it by: the
    /// image linker moves those sections into SRAM.
    pub symbols: BTreeMap<String, Vec<String>>,
}

impl CachedData {
    /// The image's own objects: those the gate counts.
    pub fn own(&self) -> usize {
        self.writable.len() + self.read_only.len()
    }
}

/// Whom one static reference of strict code serves.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) enum Use {
    /// A call into the panic machinery takes the address.
    PanicArgument,
    /// The esp-hal fork's own function makes it.
    Platform,
    /// The image's code makes it.
    Own,
}

/// The crates of the esp-hal fork, by their source directory. Its own
/// functions keep their data where esp-hal places it until our HAL
/// replaces them (#561): `INTERRUPTED_FRAME` of `handle_interrupts`, its
/// constants and its panic arguments.
pub const PLATFORM_CRATES: &[&str] = &["esp-hal", "esp-riscv-rt", "esp-sync", "esp-rom-sys"];

/// Whether `file`, a function's source file, is in a crate of the esp-hal
/// fork: a `<crate>/src/` directory of [`PLATFORM_CRATES`].
pub(super) fn platform_file(file: &str) -> bool {
    let parts: Vec<_> = file.split(['/', '\\']).collect();
    parts
        .windows(2)
        .any(|pair| pair[1] == "src" && PLATFORM_CRATES.contains(&pair[0]))
}

/// Whether the reference at `site` in `function` is an argument of a call
/// into the panic machinery: the next call or tail call of the function is
/// one (`panic_entry` of its target's name), and nothing between transfers
/// control.
pub(super) fn panic_argument(
    analysis: &Analysis,
    names: &BTreeMap<u32, String>,
    elf: &oer_analysis_elf::Elf<'_>,
    function: u32,
    site: u32,
    panic_entry: impl Fn(&str) -> bool,
) -> bool {
    let Some(call) = analysis.functions.get(&function).and_then(|facts| {
        facts
            .transfers
            .iter()
            .filter(|transfer| transfer.site > site)
            .min_by_key(|transfer| transfer.site)
    }) else {
        return false;
    };
    if !call
        .target
        .and_then(|target| names.get(&target))
        .is_some_and(|name| panic_entry(name))
    {
        return false;
    }
    straight(elf, site, call.site)
}

/// No branch, jump or return between `from` and `to` (exclusive).
fn straight(elf: &oer_analysis_elf::Elf<'_>, from: u32, to: u32) -> bool {
    use oer_analysis_riscv_decode::{Extension, Extensions, Inst, Instruction, decode};
    let Some(bytes) = usize::try_from(to - from).ok().and_then(|length| {
        elf.segments()
            .find_map(|segment| segment.bytes(u64::from(from), length))
    }) else {
        return false;
    };
    let mut at = 0;
    while at < bytes.len() {
        let Some((instruction, width)) = decode(&bytes[at..], Extensions::ALL) else {
            return false;
        };
        if matches!(
            instruction,
            Instruction::Unimp
                | Instruction::Base(
                    Inst::Jal { .. }
                        | Inst::Jalr { .. }
                        | Inst::Beq { .. }
                        | Inst::Bne { .. }
                        | Inst::Blt { .. }
                        | Inst::Bge { .. }
                        | Inst::Bltu { .. }
                        | Inst::Bgeu { .. }
                )
                | Instruction::Extension(Extension::Pop { .. })
        ) {
            return false;
        }
        at += width;
    }
    true
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
/// data symbol covering it, else by its address. `use_of` tells whom the
/// reference at a function's site serves, given whether its target is
/// read-only; an object is the image's own when one reference is, else the
/// platform's when one reference is, else a panic argument.
pub(super) fn cached_data(
    subject: String,
    elf: &oer_analysis_elf::Elf<'_>,
    references: &[(u32, u32, u64)],
    use_of: impl Fn(u32, u32, bool) -> Use,
) -> CachedData {
    let mut data = CachedData {
        subject,
        references: references.len(),
        ..CachedData::default()
    };
    let mut objects: BTreeMap<String, (bool, Use, Vec<String>)> = BTreeMap::new();
    for &(function, site, target) in references {
        let symbol = elf
            .symbols()
            .filter(|symbol| {
                symbol.defined
                    && symbol.size > 0
                    && symbol.kind == oer_analysis_elf::SymbolKind::Data
                    && target
                        .checked_sub(symbol.address)
                        .is_some_and(|offset| offset < symbol.size)
            })
            .min_by_key(|symbol| (symbol.size, symbol.address));
        let (name, symbols) = match symbol {
            Some(symbol) => (symbol.demangled(), vec![symbol.name.to_owned()]),
            // A constant pool or label: the names at its address.
            None => (
                format!("{target:#010x}"),
                elf.symbols()
                    .filter(|symbol| {
                        symbol.defined
                            && symbol.address == target
                            && !symbol.name.is_empty()
                            && !symbol.name.starts_with('$')
                    })
                    .map(|symbol| symbol.name.to_owned())
                    .collect(),
            ),
        };
        let writable = elf
            .sections()
            .find(|section| section.allocated && section.contains(target))
            .is_some_and(|section| section.writable);
        let reference = use_of(function, site, !writable);
        objects
            .entry(name)
            .and_modify(|(_, served, _)| *served = (*served).max(reference))
            .or_insert((writable, reference, symbols));
    }
    for (name, (writable, served, symbols)) in objects {
        match served {
            Use::PanicArgument => {
                data.panic_arguments.insert(name);
            }
            Use::Platform => {
                data.platform.insert(name);
            }
            Use::Own => {
                if writable {
                    data.writable.insert(name.clone());
                } else {
                    data.read_only.insert(name.clone());
                }
                data.symbols.insert(name, symbols);
            }
        }
    }
    data
}

#[cfg(test)]
mod tests;
