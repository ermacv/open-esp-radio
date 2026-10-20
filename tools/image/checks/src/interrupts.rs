//! The interrupt-stack gate: each hart's worst-case interrupt stack, bounded
//! from the image's machine code, its interrupt table and the chip's ROM ELF
//! (`oer-analysis-riscv-stack`), must leave the platform's margin below the usable
//! interrupt stack. An unknown bound fails, naming what is unresolved.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::Path;

use oer_analysis_riscv_stack::{
    Analysis, Dwarf, Field, HartStack, Stacks, TableLayout, interrupt_table, parse_summaries,
    trap_entry, vector_table,
};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

use crate::stack::budget::{Budget, MEMORY_LIMIT, TIMEOUT, WORK_UNITS};
use crate::stack::{ADMITTED, Image, contract::Contract};
use oer_analysis_riscv_model::WorkingMemory;
use oer_analysis_riscv_program::facts::ImageFacts;

/// The interrupt stacks' report in an image build's output directory.
pub const INTERRUPT_REPORT: &str = "interrupt-stack.txt";

/// Each hart's interrupt stack of the runtime ELF `elf`.
pub struct InterruptStacks {
    pub harts: Vec<HartStack>,
    /// The ROM summaries the image's analysis applied: those of the ROM
    /// functions it reaches.
    pub summaries: BTreeSet<String>,
    /// Each table entry's vector-slot symbol and the functions it calls
    /// itself (the handler, unless inlined into it).
    pub handlers: Vec<(u32, Vec<u32>)>,
    /// Function names by address, for the report.
    names: BTreeMap<u32, String>,
    /// Copies of a CLIC level or route writer outside the functions it may
    /// run inside (the contract's `level-writers`), or a writer of which the
    /// DWARF shows no copy: where a source may take a level other than its
    /// table entry's. Empty when the table levels are checked.
    pub level_writers: Vec<String>,
    /// What the interrupt handlers' code does that interrupt context should
    /// not, over what is resolved.
    pub handler_code: HandlerCode,
    /// Compiler panic entries, the platform handler, the optional image hook
    /// and all their resolved callees. This path must survive cache loss even
    /// when the image has no interrupt that deliberately panics.
    pub panic_code: HandlerCode,
    /// Static references from the image hook and its resolved callees into
    /// cached memory, by function, instruction site and referenced address.
    /// Compiler-created constants count just like named state or tables.
    pub panic_hook_data: Vec<(u32, u32, u64)>,
    /// The chip's address map and interrupt contract the bounds are held to.
    pub contract: Contract,
}

/// What the code every interrupt level reaches (its handlers and what they
/// call, over what is resolved; the exception's panic path apart) does that
/// interrupt context should not. A hole may hide more; what is listed is
/// there.
#[derive(Debug, Default)]
pub struct HandlerCode {
    /// Floating-point instructions and floating-point CSR accesses, by
    /// function and site: the handlers run with `mstatus.FS` off, so each
    /// raises an illegal-instruction exception.
    pub float: Vec<(u32, u32)>,
    /// Calls into the panic machinery (`core::panicking`), by caller and
    /// callee: an interrupt that can panic.
    pub panics: Vec<(u32, u32)>,
    /// Functions in a cached region (flash or PSRAM): code that cannot run
    /// while the cache is off.
    pub cached: Vec<u32>,
}

/// Sites of each kind the report lists.
const SHOWN: usize = 10;

/// What an image's interrupt stacks must reach: the gate's policy, apart
/// from the analysis, which always reports all it can.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Required {
    /// Every hart's bound is proven: product images.
    Proven,
    /// A hart may be `partial + ?` with its holes named, as long as the part
    /// it proves fits: images whose observers the product does not carry.
    Partial,
}

impl InterruptStacks {
    /// The gate under `required`: every entry's handler runs from SRAM, and
    /// each hart's bound, or under [`Required::Partial`] its proven part, fits
    /// the usable interrupt stack with the margin. Returns a warning for each
    /// hart left `partial + ?`.
    pub fn check(&self, required: Required) -> Result<Vec<String>> {
        if let Some((function, site, target)) = self.panic_hook_data.first() {
            return Err(format!(
                "the panic hook references cached memory at {target:#010x} from {} \
                 at {site:#010x}:\n{}",
                self.name(*function),
                self.render()
            )
            .into());
        }
        if let Some(function) = self.panic_code.cached.first() {
            return Err(format!(
                "the panic path runs {} from cached memory:\n{}",
                self.name(*function),
                self.render()
            )
            .into());
        }
        let in_sram = |address: u32| {
            self.contract
                .memory
                .sram
                .contains_range(u64::from(address), u64::from(address) + 4)
        };
        // A floating-point instruction in a handler traps: the handlers run
        // with the FPU off.
        if let Some((function, site)) = self.handler_code.float.first() {
            return Err(format!(
                "interrupt context runs floating point at {site:#010x} in {}, with the FPU off",
                self.name(*function)
            )
            .into());
        }
        // The sum over levels needs each source at its table level.
        if !self.level_writers.is_empty() {
            return Err(format!(
                "the table levels are not checked: {}",
                self.level_writers.join("; ")
            )
            .into());
        }
        for (slot, calls) in &self.handlers {
            if let Some(outside) = std::iter::once(slot)
                .chain(calls)
                .find(|address| !in_sram(**address))
            {
                return Err(format!(
                    "the interrupt handler `{}` runs {} from outside SRAM",
                    self.name(*slot),
                    self.name(*outside)
                )
                .into());
            }
        }
        let mut warnings = Vec::new();
        for hart in &self.harts {
            // The sum over levels holds only where nothing lowers the level.
            let drops = hart.level_drops();
            if !drops.is_empty() {
                match required {
                    Required::Proven => {
                        return Err(format!(
                            "hart {}'s interrupt stack is conditional: its handlers can lower \
                             the interrupt level, so one level may nest in itself:\n{}",
                            hart.core,
                            self.render()
                        )
                        .into());
                    }
                    Required::Partial => warnings.push(format!(
                        "hart {}'s interrupt stack is conditional on the nesting rule, which {} \
                         sites may break: they are in the report",
                        hart.core,
                        drops.len()
                    )),
                }
            }
            let assumptions = hart.assumptions();
            if let Some(assumption) = assumptions
                .iter()
                .find(|assumption| !ADMITTED.contains(assumption))
            {
                return Err(format!(
                    "hart {}'s interrupt stack assumes {assumption}, which the gate does not \
                     admit:\n{}",
                    hart.core,
                    self.render()
                )
                .into());
            }
            for assumption in &assumptions {
                warnings.push(format!(
                    "hart {}'s interrupt stack is conditional on {assumption}",
                    hart.core
                ));
            }
            let bytes = match (hart.bytes, required) {
                (Some(bytes), _) => bytes,
                (None, Required::Proven) => {
                    return Err(format!(
                        "hart {}'s interrupt stack has no bound:\n{}",
                        hart.core,
                        self.render()
                    )
                    .into());
                }
                (None, Required::Partial) => {
                    warnings.push(format!(
                        "hart {}'s interrupt stack is {} + ? bytes: its holes are in the report",
                        hart.core, hart.partial
                    ));
                    hart.partial
                }
            };
            let percent = self.contract.interrupts.irq_stack_margin_percent;
            let usable = self.contract.irq_stack_usable_bytes();
            if with_margin(bytes, percent) > u64::from(usable) {
                return Err(format!(
                    "hart {}'s interrupt stack needs {bytes} bytes and a {}% margin, more than \
                     its usable {} bytes:\n{}",
                    hart.core,
                    percent,
                    usable,
                    self.render()
                )
                .into());
            }
        }
        Ok(warnings)
    }

    fn name(&self, address: u32) -> String {
        self.names
            .get(&address)
            .cloned()
            .unwrap_or_else(|| format!("{address:#010x}"))
    }

    /// Each hart's bound, its levels and their critical paths.
    pub fn render(&self) -> String {
        let name = |address: u32| self.name(address);
        let mut out = String::new();
        if self.level_writers.is_empty() {
            let _ = writeln!(
                out,
                "levels: checked, every CLIC level and route writer runs in start-up or the table"
            );
        } else {
            let _ = writeln!(out, "levels: not checked");
            for problem in &self.level_writers {
                let _ = writeln!(out, "  {problem}");
            }
        }
        let code = &self.handler_code;
        let _ = writeln!(
            out,
            "handler code: {} floating-point sites, {} calls into the panic machinery, {} \
             functions in cached memory",
            code.float.len(),
            code.panics.len(),
            code.cached.len()
        );
        for (function, site) in code.float.iter().take(SHOWN) {
            let _ = writeln!(
                out,
                "  floating point at {site:#010x} in {}",
                name(*function)
            );
        }
        for (caller, callee) in code.panics.iter().take(SHOWN) {
            let _ = writeln!(out, "  {} calls {}", name(*caller), name(*callee));
        }
        for function in code.cached.iter().take(SHOWN) {
            let _ = writeln!(out, "  in cached memory: {}", name(*function));
        }
        let _ = writeln!(
            out,
            "panic code: {} functions in cached memory",
            self.panic_code.cached.len()
        );
        for function in self.panic_code.cached.iter().take(SHOWN) {
            let _ = writeln!(out, "  in cached memory: {}", name(*function));
        }
        let _ = writeln!(
            out,
            "panic hook data: {} static references into cached memory",
            self.panic_hook_data.len()
        );
        for (function, site, target) in self.panic_hook_data.iter().take(SHOWN) {
            let _ = writeln!(
                out,
                "  {} at {site:#010x} references {target:#010x}",
                name(*function)
            );
        }
        for hart in &self.harts {
            let _ = writeln!(
                out,
                "hart {}: {} of {} usable bytes, {}",
                hart.core,
                hart.bytes
                    .map_or(format!("{} + ?", hart.partial), |bytes| bytes.to_string()),
                self.contract.irq_stack_usable_bytes(),
                match hart.bytes {
                    None => "partial",
                    Some(_) if hart.assumptions().is_empty() && hart.level_drops().is_empty() => {
                        "proven"
                    }
                    Some(_) => "conditional",
                }
            );
            for assumption in hart.assumptions() {
                let _ = writeln!(out, "  assumes {assumption}");
            }
            let drops = hart.level_drops();
            if drops.is_empty() {
                let _ = writeln!(
                    out,
                    "  nesting: checked, no handler can lower the interrupt level"
                );
            } else {
                let _ = writeln!(
                    out,
                    "  nesting: conditional, a level may nest in itself where a handler lowers \
                     the interrupt level:"
                );
                for (site, drop) in drops {
                    let _ = writeln!(out, "    {drop} at {site:#010x}");
                }
            }
            for level in hart.levels.iter().chain([&hart.exception]) {
                let label = if level.level == 0 {
                    "exception".to_owned()
                } else {
                    format!("level {}", level.level)
                };
                let _ = writeln!(
                    out,
                    "  {label}: {} (entry frame {})",
                    level
                        .bytes
                        .map_or(format!("{} + ?", level.partial), |bytes| bytes.to_string()),
                    level.frame
                );
                for (function, frame) in &level.bound.path {
                    let _ = writeln!(out, "    {frame:>6} {}", name(*function));
                }
                for (site, reason) in &level.bound.unresolved {
                    let _ = writeln!(out, "    unresolved {reason} at {site:#010x}");
                }
            }
        }
        out
    }
}

/// `bytes` with the platform's margin.
fn with_margin(bytes: u64, percent: u32) -> u64 {
    bytes + bytes * u64::from(percent) / 100
}

/// The names of the reviewed ROM summaries of `profile`'s chip; none for a
/// chip that reviews none.
pub fn rom_summaries(root: &Path, profile: &oer_chip_profile::Profile) -> Result<Vec<String>> {
    let Some(path) = profile.rom.as_ref().and_then(|rom| rom.summaries.as_ref()) else {
        return Ok(Vec::new());
    };
    let summaries = parse_summaries(&std::fs::read_to_string(root.join(path))?)?;
    Ok(summaries.into_iter().map(|summary| summary.name).collect())
}

/// Bound every hart's interrupt stack of the runtime ELF bytes `elf`, with
/// the pinned ROM of the repository at `root`.
pub fn interrupt_stacks(
    root: &Path,
    profile: &oer_chip_profile::Profile,
    elf: Vec<u8>,
) -> Result<InterruptStacks> {
    interrupt_stacks_of(&Image::of_bytes(root, profile, elf)?)
}

/// [`interrupt_stacks`] of an analysed runtime image.
pub fn interrupt_stacks_of(image: &Image) -> Result<InterruptStacks> {
    let Image {
        elf,
        analysis,
        functions,
        names,
        dwarf,
        resolutions,
        contract,
        ..
    } = image;
    let contract = contract
        .as_ref()
        .ok_or("the image's chip names no [memory] map or [interrupts] contract")?;
    let interrupts = &contract.interrupts;
    let symbol = |name: &str| -> Result<u32> {
        image
            .symbol(name)
            .ok_or_else(|| format!("the runtime has no `{name}`").into())
    };
    let field = |(offset, size): (u32, u32)| Field { offset, size };
    let memory = WorkingMemory::new(MEMORY_LIMIT)?;
    let mut control = Budget::new(WORK_UNITS, TIMEOUT);
    let facts = ImageFacts::new(elf, &[], &memory, &mut control)?;
    let table = interrupt_table(
        &facts,
        &interrupts.table_symbol,
        &TableLayout {
            entry: interrupts.table_entry_bytes,
            source: field(interrupts.table_source.into()),
            level: field(interrupts.table_level.into()),
            core: field(interrupts.table_core.into()),
            handler: interrupts.table_handler,
        },
        &mut control,
    )?;
    let vectors = vector_table(&facts, &interrupts.vector_table_symbol, &mut control)?
        .into_iter()
        .flatten()
        .map(|entry| trap_entry(&facts, entry, &mut control))
        .collect::<oer_analysis_riscv_model::Result<Vec<_>>>()?;
    let exception = trap_entry(
        &facts,
        symbol(&interrupts.exception_entry_symbol)?,
        &mut control,
    )?;
    // The handler a slot calls is a direct call of the slot's own code: a
    // call site no inlined function owns. Calls from a handler inlined into
    // the slot are the handler's.
    let handlers = table
        .iter()
        .filter_map(|entry| entry.handler)
        .map(|slot| {
            let calls = analysis
                .functions
                .get(&slot)
                .map(|facts| {
                    facts
                        .transfers
                        .iter()
                        .filter_map(|transfer| {
                            let target = transfer.target?;
                            let own = dwarf
                                .inline_chain(transfer.site)
                                .map_or(true, |chain| chain.len() <= 1);
                            own.then_some(target)
                        })
                        .collect()
                })
                .unwrap_or_default();
            (slot, calls)
        })
        .collect();
    let level_writers = level_writers(dwarf, interrupts)?;

    let sources = source_table(elf, &interrupts.source_table_symbol)?;
    let harts = oer_analysis_riscv_stack::interrupt_stacks(
        analysis,
        &Stacks {
            table: &table,
            cores: &interrupts.harts,
            always: &interrupts.always_levels,
            vectors: &vectors,
            exception,
            sources,
            resolutions,
        },
    )?;
    let reached: BTreeSet<u32> = harts
        .iter()
        .flat_map(|hart| hart.levels.iter())
        .flat_map(|level| level.bound.reached.iter().copied())
        .collect();
    let interrupt_code = handler_code(analysis, &reached, names, &contract.memory);
    let panic_code = panic_path_code(analysis, names, resolutions, &contract.memory)?;
    let hook = names
        .iter()
        .find(|(_, name)| *name == "oer_platform_panic_hook");
    let panic_hook_data = match hook {
        Some((&address, _)) => cached_hook_references(
            &oer_analysis_elf::Elf::parse(elf)?,
            functions,
            &analysis.bound_with(address, resolutions)?.reached,
            &contract.memory,
        )?,
        None => Vec::new(),
    };
    Ok(InterruptStacks {
        harts,
        handlers,
        summaries: analysis.summaries.clone(),
        names: names.clone(),
        level_writers,
        handler_code: interrupt_code,
        panic_code,
        panic_hook_data,
        contract: contract.clone(),
    })
}

/// The retained relocations reveal compiler-created constants as well as
/// globals. The hook only receives metadata already safe for its context;
/// unlike task-only metadata extraction in the platform handler, none of its
/// own static references may depend on the cache.
fn cached_hook_references(
    elf: &oer_analysis_elf::Elf<'_>,
    functions: &[oer_analysis_riscv_stack::Function],
    reached: &BTreeSet<u32>,
    memory: &oer_chip_profile::Memory,
) -> Result<Vec<(u32, u32, u64)>> {
    use oer_analysis_elf::rv32::{Role, kind};
    let mut references = BTreeSet::new();
    for section in elf.sections().filter(|section| section.executable) {
        for relocation in elf.relocations(section.index)? {
            if matches!(
                kind(relocation.r_type).role,
                Role::Hint | Role::PcRelativeLow
            ) {
                continue;
            }
            let Some(function) = functions
                .get(
                    ..functions
                        .partition_point(|function| u64::from(function.address) <= relocation.at),
                )
                .and_then(|before| before.last())
                .filter(|function| {
                    relocation.at < u64::from(function.address) + u64::from(function.size)
                        && reached.contains(&function.address)
                })
            else {
                continue;
            };
            let target = elf.target_address(&relocation)?;
            if target.checked_add(1).is_some_and(|end| {
                memory
                    .cached()
                    .iter()
                    .any(|region| region.contains_range(target, end))
            }) {
                references.insert((function.address, u32::try_from(relocation.at)?, target));
            }
        }
    }
    Ok(references.into_iter().collect())
}

fn panic_path_code(
    analysis: &Analysis,
    names: &BTreeMap<u32, String>,
    resolutions: &oer_analysis_riscv_stack::Resolutions,
    memory: &oer_chip_profile::Memory,
) -> Result<HandlerCode> {
    let mut reached = BTreeSet::new();
    for (&address, _) in names.iter().filter(|(_, name)| panic_entry(name)) {
        reached.extend(analysis.bound_with(address, resolutions)?.reached);
    }
    Ok(handler_code(analysis, &reached, names, memory))
}

/// Demangled compiler entry points in the pinned toolchain, plus the image
/// hook. Check their final machine-code call graph, rather than source names
/// or linker-script text: helpers can be outlined or removed by optimization.
fn panic_entry(name: &str) -> bool {
    name.starts_with("core::panicking::")
        || name.starts_with("core::option::unwrap_failed")
        || name.starts_with("core::option::expect_failed")
        || name.starts_with("core::result::unwrap_failed")
        || (name.starts_with("core::slice::index::slice_index") && name.contains("fail"))
        || name.starts_with("core::cell::panic_already")
        || name.ends_with("::rust_begin_unwind")
        || name == "oer_platform_panic_hook"
}

/// What the functions in `reached` do that interrupt context should not.
fn handler_code(
    analysis: &Analysis,
    reached: &BTreeSet<u32>,
    names: &BTreeMap<u32, String>,
    memory: &oer_chip_profile::Memory,
) -> HandlerCode {
    let panicking = |address: u32| {
        names
            .get(&address)
            .is_some_and(|name| name.starts_with("core::panicking::"))
    };
    let mut code = HandlerCode::default();
    for &function in reached {
        let Some(facts) = analysis.functions.get(&function) else {
            continue;
        };
        code.float
            .extend(facts.float_sites.iter().map(|&site| (function, site)));
        if !panicking(function) {
            code.panics.extend(
                facts
                    .transfers
                    .iter()
                    .filter_map(|transfer| transfer.target)
                    .filter(|&target| panicking(target))
                    .map(|target| (function, target)),
            );
        }
        let at = (u64::from(function), u64::from(function) + 4);
        if memory
            .cached()
            .iter()
            .any(|region| region.contains_range(at.0, at.1))
        {
            code.cached.push(function);
        }
    }
    code.panics.sort_unstable();
    code.panics.dedup();
    code
}

/// Where a CLIC level or route writer runs outside the functions it may run
/// inside, or a writer of which the DWARF shows no copy. Each copy's inline
/// chain must hold one of the writer's contexts.
fn level_writers(
    dwarf: &Dwarf,
    interrupts: &oer_chip_profile::InterruptContract,
) -> Result<Vec<String>> {
    let writers: Vec<&str> = interrupts
        .level_writers
        .iter()
        .map(|writer| writer.writer.as_str())
        .collect();
    let copies = dwarf.instances(&writers);
    let mut problems = Vec::new();
    for level_writer in &interrupts.level_writers {
        let (writer, contexts) = (&level_writer.writer, &level_writer.contexts);
        let starts = &copies[writer.as_str()];
        if starts.is_empty() {
            problems.push(format!("the DWARF shows no copy of `{writer}`"));
        }
        for &start in starts {
            let chain = dwarf.inline_chain(start)?;
            if !chain.iter().any(|function| contexts.contains(function)) {
                problems.push(format!(
                    "`{writer}` at {start:#010x} runs inside {}",
                    chain.last().map_or("no function", String::as_str)
                ));
            }
        }
    }
    Ok(problems)
}

/// The address of esp-hal's per-source handler table.
fn source_table(elf: &[u8], symbol: &str) -> Result<u32> {
    oer_analysis_elf::Elf::parse(elf)?
        .address(symbol)
        .map(|address| address as u32)
        .ok_or_else(|| format!("the runtime has no `{symbol}`").into())
}
