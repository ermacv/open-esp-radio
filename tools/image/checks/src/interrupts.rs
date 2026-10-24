//! The interrupt gate checks code placement and each hart's worst-case stack,
//! bounded from the image's machine code, its interrupt table and the chip's
//! ROM ELF (`oer-analysis-riscv-stack`). The bound must leave the platform's
//! margin below the usable interrupt stack. Diagnostic images report placement
//! violations and unresolved bounds; handler entries and panic safety are
//! mandatory under either policy.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::Path;

use oer_analysis_riscv_stack::{
    Analysis, Dwarf, Field, HartStack, Stacks, TableLayout, interrupt_table, parse_summaries,
    trap_entry, vector_table,
};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

use crate::stack::budget::{Budget, LIMITS};
use crate::stack::{ADMITTED, Image, contract::Contract};
use oer_analysis_riscv_model::{Ledger, RunControl, WorkingMemory};
use oer_analysis_riscv_program::facts::ImageFacts;

mod placement;
pub use placement::{INTERRUPT_PLACEMENT_REPORT, InterruptPlacement};
mod code;
pub use code::CodePlacement;
pub(crate) mod allocation;
mod guard;
pub use allocation::LinkedStack;

/// The interrupt stacks' report in an image build's output directory.
pub const INTERRUPT_REPORT: &str = "interrupt-stack.txt";

/// Each hart's interrupt stack of the runtime ELF `elf`.
pub struct InterruptStacks {
    pub resources: Box<oer_analysis_riscv_model::AnalysisResources>,
    /// Each hart's actual linked storage, independently of the contract maximum.
    pub allocations: BTreeMap<u32, LinkedStack>,
    pub expression_limited_functions: usize,
    pub witness_memory_peak: u64,
    pub traps: oer_analysis_riscv_stack::TrapFact,
    pub harts: Vec<HartStack>,
    /// The ROM summaries the image's analysis applied: those of the ROM
    /// functions it reaches.
    pub summaries: BTreeSet<String>,
    /// Each table entry's vector-slot symbol and the functions it calls
    /// itself (the handler, unless inlined into it).
    pub handlers: Vec<(u32, Vec<u32>)>,
    /// Function names by address, for the report.
    names: BTreeMap<u32, String>,
    /// Own function frames, read from the analysis rather than a bound's
    /// critical path: a shorter path can still contain the largest frame.
    pub frames: BTreeMap<u32, Option<u64>>,
    /// Copies of a CLIC level or route writer outside the functions it may
    /// run inside (the contract's `level-writers`), or a writer of which the
    /// DWARF shows no copy: where a source may take a level other than its
    /// table entry's. Empty when the table levels are checked.
    pub level_writers: Vec<String>,
    /// What the interrupt handlers' code does that interrupt context should
    /// not, over what is resolved.
    pub handler_code: HandlerCode,
    /// Strict executor/handler paths and P2 task paths, separated by facts.
    pub code_placement: CodePlacement,
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

/// The interrupt gate's policy for code placement and stack bounds, apart
/// from the analysis, which always reports all it can.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Required {
    /// Enforce strict/P2 code placement and require every hart's bound to be proven.
    Product,
    /// Report strict/P2 code placement violations; a hart may be `partial + ?`
    /// with its holes named, as long as its proven part fits. Handler entries
    /// and panic paths retain the product requirements.
    Diagnostic,
}

impl InterruptStacks {
    /// Check handler entries, panic safety, code placement and stack bounds.
    /// Under [`Required::Diagnostic`], report strict/P2 placement violations
    /// and partial bounds as warnings; each hart's proven part must still fit
    /// the usable interrupt stack with the margin.
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
        for context in &self.code_placement.contexts {
            use code::Location::{Flash, Other, Psram};
            for (addresses, role, subject, region, locations) in [
                (
                    &context.strict,
                    "strict",
                    "interrupt handler",
                    "cached memory",
                    &[Psram, Flash][..],
                ),
                (
                    &context.strict,
                    "strict",
                    "interrupt handler",
                    "outside SRAM/ROM",
                    &[Other][..],
                ),
                (
                    &context.tasks,
                    "P2 task",
                    "P2 task",
                    "cached flash",
                    &[Flash][..],
                ),
                (
                    &context.tasks,
                    "P2 task",
                    "P2 task",
                    "outside SRAM/ROM/PSRAM",
                    &[Other][..],
                ),
            ] {
                let functions: Vec<_> = addresses
                    .iter()
                    .filter_map(|address| {
                        self.code_placement
                            .functions
                            .get(address)
                            .map(|function| (address, function))
                    })
                    .filter(|(_, function)| locations.contains(&function.location))
                    .collect();
                let Some((first, _)) = functions.first() else {
                    continue;
                };
                match required {
                    Required::Product => {
                        return Err(format!(
                            "{subject} code runs {} from {region}, reached from {context}:\n{}",
                            self.name(**first),
                            self.render()
                        )
                        .into());
                    }
                    Required::Diagnostic => {
                        let bytes: u64 = functions
                            .iter()
                            .map(|(_, function)| u64::from(function.size))
                            .sum();
                        let preposition = if region.starts_with("outside") {
                            ""
                        } else {
                            "in "
                        };
                        warnings.push(format!(
                            "{context}: {} {role} functions, {bytes} bytes {preposition}{region} \
                             (diagnostic image, reported only); see {INTERRUPT_PLACEMENT_REPORT}",
                            functions.len()
                        ));
                    }
                }
            }
        }
        for hart in &self.harts {
            // The sum over levels holds only where nothing lowers the level.
            let drops = hart.level_drops();
            if !drops.is_empty() {
                match required {
                    Required::Product => {
                        return Err(format!(
                            "hart {}'s interrupt stack is conditional: its handlers can lower \
                             the interrupt level, so one level may nest in itself:\n{}",
                            hart.core,
                            self.render()
                        )
                        .into());
                    }
                    Required::Diagnostic => warnings.push(format!(
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
                (None, Required::Product) => {
                    return Err(format!(
                        "hart {}'s interrupt stack has no bound:\n{}",
                        hart.core,
                        self.render()
                    )
                    .into());
                }
                (None, Required::Diagnostic) => {
                    warnings.push(format!(
                        "hart {}'s interrupt stack is {} + ? bytes: its holes are in the report",
                        hart.core, hart.partial
                    ));
                    hart.partial
                }
            };
            let percent = self.contract.interrupts.irq_stack_margin_percent;
            let usable = self.usable_bytes(hart.core);
            if self.contract.interrupts.irq_stack_with_margin(bytes)? > u64::from(usable) {
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
            if let Some(problem) = self.guard_problem(hart) {
                match required {
                    Required::Product => {
                        return Err(format!("{problem}:\n{}", self.render()).into());
                    }
                    Required::Diagnostic => warnings.push(format!(
                        "{problem} (diagnostic image, reported only):\n{}",
                        self.render_guards()
                    )),
                }
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
        self.render_report(None)
    }

    /// The stack report with placement measurements after its handler inventory.
    pub fn render_with_placement(&self, placement: &InterruptPlacement) -> String {
        self.render_report(Some(placement))
    }

    fn render_report(&self, placement: Option<&InterruptPlacement>) -> String {
        let name = |address: u32| self.name(address);
        let mut resources = (*self.resources).clone();
        if self.expression_limited_functions != 0 {
            resources
                .limited
                .result(oer_analysis_riscv_model::LimitReason::LocalBudget(
                    oer_analysis_riscv_model::LocalBudgetScope::FunctionExpressions,
                ));
        }
        let mut out = crate::stack::budget::header(&resources, self.witness_memory_peak);
        let _ = writeln!(out, "{}", self.traps);
        if self.expression_limited_functions != 0 {
            let _ = writeln!(
                out,
                "expression limits: {} functions",
                self.expression_limited_functions
            );
        }
        out.push_str(&self.render_allocations());
        out.push_str(&self.render_guards());
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
        if let Some(placement) = placement {
            out.push_str(&placement.summary());
        } else {
            out.push_str(&self.code_placement.summary(name));
        }
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
        if self
            .code_placement
            .contexts
            .iter()
            .any(|context| !context.cached_paths.is_empty())
        {
            out.push_str(&self.code_placement.render_paths(name));
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
                self.usable_bytes(hart.core),
                crate::stack::bound_state(
                    hart.bytes,
                    &hart.assumptions(),
                    !hart.level_drops().is_empty()
                )
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

/// The names of the reviewed ROM summaries of `profile`'s chip; none for a
/// chip that reviews none.
pub fn rom_summaries(root: &Path, profile: &oer_chip_profile::Profile) -> Result<Vec<String>> {
    let Some(path) = profile.rom.as_ref().and_then(|rom| rom.summaries.as_ref()) else {
        return Ok(Vec::new());
    };
    let summaries = parse_summaries(&std::fs::read_to_string(root.join(path))?)?;
    Ok(summaries.into_iter().map(|summary| summary.name).collect())
}

/// A core without an interrupt stack takes no interrupt: an entry that routes
/// a source to it would run a handler on a stack nobody bounded.
fn reject_unstacked_cores(
    table: &[oer_analysis_riscv_stack::TableEntry],
    harts: &[u32],
) -> Result<()> {
    match table.iter().find(|entry| !harts.contains(&entry.core)) {
        Some(entry) => Err(format!(
            "the interrupt table routes source {} (level {}) to core {}, whose interrupt \
             stack the image declares unused or the chip does not have",
            entry.source, entry.level, entry.core
        )
        .into()),
        None => Ok(()),
    }
}

/// Bound every hart's interrupt stack of the runtime ELF bytes `elf`, with
/// the pinned ROM of the repository at `root`. A hart whose linked stack is
/// empty is one the image's stack policy declares unused, which the image
/// gate proved at build time; it has no stack to bound.
pub fn interrupt_stacks(
    root: &Path,
    profile: &oer_chip_profile::Profile,
    elf: Vec<u8>,
) -> Result<InterruptStacks> {
    let mut profile = profile.clone();
    if let Some(contract) = profile.interrupts.as_mut() {
        let file = oer_analysis_elf::Elf::parse(&elf)?;
        let mut harts = Vec::new();
        for &hart in &contract.harts {
            let range = oer_analysis_elf::StorageBounds::range(
                &file,
                &format!("__runtime_cpu{hart}_irq_stack_bottom"),
                &format!("__runtime_cpu{hart}_irq_stack_top"),
            )?;
            if range.is_none_or(|range| range.capacity != 0) {
                harts.push(hart);
            }
        }
        contract.harts = harts;
    }
    interrupt_stacks_of(&Image::of_bytes(root, &profile, elf)?)
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
    let memory = WorkingMemory::new(LIMITS.working_memory_bytes)?;
    let mut control = Budget::new(
        LIMITS.work_units,
        std::time::Duration::from_millis(LIMITS.timeout_ms),
    );
    let _retained = memory.reserve(image.retained_bytes, control.position())?;
    let mut keep = Ledger::new(&memory, &mut control)?;
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
    reject_unstacked_cores(&table, &interrupts.harts)?;
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
    let inputs = Stacks {
        table: &table,
        cores: &interrupts.harts,
        vectors: &vectors,
        exception,
        sources,
        resolutions,
    };
    let harts =
        oer_analysis_riscv_stack::interrupt_stacks(analysis, &inputs, &mut keep, &mut control)?;
    let reached: BTreeSet<u32> = harts
        .iter()
        .flat_map(|hart| hart.levels.iter())
        .flat_map(|level| level.bound.reached.iter().copied())
        .collect();
    let interrupt_code = handler_code(analysis, &reached, names, &contract.memory);
    let panic_code = panic_path_code(
        analysis,
        names,
        resolutions,
        &contract.memory,
        &mut keep,
        &mut control,
    )?;
    let code_placement = CodePlacement::of(
        analysis,
        functions,
        &harts,
        &inputs,
        panic_reached(analysis, names, resolutions, &mut keep, &mut control)?,
        &contract.memory,
        &mut keep,
        &mut control,
    )?;
    let hook = names
        .iter()
        .find(|(_, name)| *name == "oer_platform_panic_hook");
    let panic_hook_data = match hook {
        Some((&address, _)) => cached_hook_references(
            &oer_analysis_elf::Elf::parse(elf)?,
            functions,
            &analysis
                .bound_with(address, resolutions, &mut keep, &mut control)?
                .reached,
            &contract.memory,
        )?,
        None => Vec::new(),
    };
    Ok(InterruptStacks {
        resources: image.resources.clone(),
        allocations: allocation::read(
            &oer_analysis_elf::Elf::parse(elf)?,
            interrupts,
            &contract.memory,
        )?,
        expression_limited_functions: analysis.expression_limited_functions(),
        witness_memory_peak: memory.peak().max(image.resources.usage.peak_admitted_bytes),
        traps: image.traps.clone(),
        harts,
        handlers,
        summaries: analysis.summaries.clone(),
        names: names.clone(),
        frames: analysis
            .functions
            .iter()
            .map(|(&address, facts)| (address, facts.frame))
            .collect(),
        level_writers,
        handler_code: interrupt_code,
        code_placement,
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
    keep: &mut Ledger<'_>,
    control: &mut dyn RunControl,
) -> Result<HandlerCode> {
    Ok(handler_code(
        analysis,
        &panic_reached(analysis, names, resolutions, keep, control)?,
        names,
        memory,
    ))
}

fn panic_reached(
    analysis: &Analysis,
    names: &BTreeMap<u32, String>,
    resolutions: &oer_analysis_riscv_stack::Resolutions,
    keep: &mut Ledger<'_>,
    control: &mut dyn RunControl,
) -> Result<BTreeSet<u32>> {
    let mut reached = BTreeSet::new();
    for (&address, _) in names.iter().filter(|(_, name)| panic_entry(name)) {
        reached.extend(
            analysis
                .bound_with(address, resolutions, keep, control)?
                .reached,
        );
    }
    Ok(reached)
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

#[cfg(test)]
mod tests {
    #[test]
    fn a_table_entry_for_a_core_without_an_interrupt_stack_is_refused() {
        use oer_analysis_riscv_stack::TableEntry;
        let entry = |core| TableEntry {
            source: 7,
            level: 2,
            core,
            handler: Some(0x4000_0000),
        };
        reject_unstacked_cores(&[entry(0), entry(0)], &[0]).unwrap();
        reject_unstacked_cores(&[entry(0), entry(1)], &[0, 1]).unwrap();
        let error = reject_unstacked_cores(&[entry(0), entry(1)], &[0])
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("source 7") && error.contains("to core 1"),
            "{error}"
        );
    }

    use super::*;
    use oer_analysis_riscv_stack::Assumption;

    fn contract() -> Contract {
        [
            include_str!("../../../../chips/esp32c5/platform/chip.toml"),
            include_str!("../../../../chips/esp32s31/platform/chip.toml"),
        ]
        .into_iter()
        .map(|text| toml::from_str::<oer_chip_profile::Profile>(text).unwrap())
        .find_map(|profile| Contract::of(&profile))
        .expect("a chip names an interrupt contract")
    }

    /// The `TIMER` slot's handler: in the contract's SRAM.
    fn timer() -> u32 {
        contract().memory.sram.origin + 0x1000
    }

    pub(super) fn ledger() -> Ledger<'static> {
        let memory = Box::leak(Box::new(
            WorkingMemory::new(LIMITS.working_memory_bytes).unwrap(),
        ));
        Ledger::new(memory, &mut || Ok(())).unwrap()
    }

    pub(super) fn stacks(handlers: Vec<(u32, Vec<u32>)>) -> InterruptStacks {
        InterruptStacks {
            resources: Box::new(crate::stack::budget::resources(0, 0, Default::default())),
            allocations: BTreeMap::new(),
            expression_limited_functions: 0,
            witness_memory_peak: 0,
            traps: Default::default(),
            harts: Vec::new(),
            summaries: BTreeSet::new(),
            handlers,
            names: BTreeMap::from([(timer(), "TIMER".to_owned())]),
            frames: BTreeMap::new(),
            level_writers: Vec::new(),
            handler_code: HandlerCode::default(),
            code_placement: CodePlacement::default(),
            panic_code: HandlerCode::default(),
            panic_hook_data: Vec::new(),
            contract: contract(),
        }
    }

    pub(super) fn partial(partial: u64) -> InterruptStacks {
        let bound = oer_analysis_riscv_stack::Bound {
            root: 0,
            bytes: None,
            partial,
            unresolved: Vec::new(),
            path: Vec::new(),
            level_drops: Vec::new(),
            assumptions: BTreeSet::new(),
            sites: BTreeMap::new(),
            reached: BTreeSet::new(),
            predecessors: Default::default(),
        };
        let level = oer_analysis_riscv_stack::LevelStack {
            level: 0,
            bytes: None,
            partial,
            frame: 0,
            bound,
        };
        InterruptStacks {
            resources: Box::new(crate::stack::budget::resources(0, 0, Default::default())),
            allocations: BTreeMap::new(),
            expression_limited_functions: 0,
            witness_memory_peak: 0,
            traps: Default::default(),
            harts: vec![HartStack {
                core: 0,
                levels: Vec::new(),
                exception: level,
                bytes: None,
                partial,
            }],
            summaries: BTreeSet::new(),
            handlers: Vec::new(),
            names: BTreeMap::new(),
            frames: BTreeMap::new(),
            level_writers: Vec::new(),
            handler_code: HandlerCode::default(),
            code_placement: CodePlacement::default(),
            panic_code: HandlerCode::default(),
            panic_hook_data: Vec::new(),
            contract: contract(),
        }
    }

    #[test]
    fn allocation_uses_only_a_proven_product_bound_and_reports_its_source() {
        let mut stacks = partial(1000);
        let select = |stacks: &InterruptStacks, diagnostic| {
            crate::linker::irq_stacks::select(stacks, diagnostic)
                .unwrap()
                .remove(0)
        };
        let partial = select(&stacks, false);
        assert!(!partial.from_bound);
        assert_eq!(partial.bytes, stacks.contract.interrupts.irq_stack_bytes);
        stacks.harts[0].bytes = Some(1000);
        let diagnostic = select(&stacks, true);
        assert!(!diagnostic.from_bound);
        assert_eq!(diagnostic.bytes, stacks.contract.interrupts.irq_stack_bytes);
        let product = select(&stacks, false);
        assert!(product.from_bound);
        assert_eq!(product.bytes, 5200);
        stacks.allocations.insert(
            0,
            LinkedStack {
                bottom: u64::from(stacks.contract.memory.sram.origin),
                bytes: product.bytes,
                from_bound: true,
            },
        );
        assert_eq!(stacks.usable_bytes(0), 1104);
        assert!(stacks.check(Required::Product).is_ok());
        assert!(
            stacks
                .render()
                .contains("0\t1000\t100 (10%)\t4096\t5200\tproven bound")
        );
        stacks.harts[0].bytes = Some(1010);
        assert!(stacks.check(Required::Product).is_err());
    }

    #[test]
    fn a_handler_called_from_outside_sram_fails_the_gate() {
        let sram = timer();
        let cached = contract().memory.flash_xip.origin;
        // A slot whose handler was inlined into it calls nothing itself.
        assert!(
            stacks(vec![(sram, Vec::new())])
                .check(Required::Product)
                .is_ok()
        );
        assert!(
            stacks(vec![(sram, vec![sram + 0x40])])
                .check(Required::Product)
                .is_ok()
        );
        for policy in [Required::Product, Required::Diagnostic] {
            let error = stacks(vec![(sram, vec![cached])])
                .check(policy)
                .unwrap_err()
                .to_string();
            assert!(error.contains("`TIMER`"), "{error}");
            assert!(stacks(vec![(cached, Vec::new())]).check(policy).is_err());
        }
    }

    #[test]
    fn panic_helpers_and_transitive_hook_callees_must_be_uncached() {
        use oer_analysis_riscv_stack::{
            FrameSource, Function, FunctionFacts, Transfer, TransferKind,
        };
        let memory = contract().memory;
        let sram = memory.sram.origin + 0x1000;
        let cached = memory.psram.unwrap().origin + 0x1000;
        let facts = |address, name: &str, callee: Option<u32>| FunctionFacts {
            function: Function {
                address,
                size: 4,
                names: vec![name.to_owned()],
                section: 0,
            },
            frame: Some(0),
            source: Some(FrameSource::Observed),
            observed: Some(0),
            complete: true,
            expression_limited: false,
            limit_events: [0; 2],
            terminal_recursive_callees: false,
            target_loads: BTreeMap::new(),
            open_exit_target_loads: BTreeMap::new(),
            load_bases: BTreeMap::new(),
            code_facts: BTreeMap::new(),
            transfers: callee
                .map(|target| Transfer {
                    site: address,
                    target: Some(target),
                    kind: TransferKind::Tail,
                    source: None,
                    table: None,
                    load: None,
                })
                .into_iter()
                .collect(),
            site_depths: BTreeMap::new(),
            call_arguments: BTreeMap::new(),
            table_bases: BTreeMap::new(),
            unresolved_registers: BTreeMap::new(),
            level_drops: Vec::new(),
            float_sites: Vec::new(),
        };
        for entry in [
            "core::option::unwrap_failed",
            "core::option::expect_failed",
            "core::result::unwrap_failed",
            "core::slice::index::slice_index_order_fail",
            "core::cell::panic_already_borrowed",
            "__rustc::rust_begin_unwind",
            "oer_platform_panic_hook",
        ] {
            // No interrupt calls this entry. Its own path still must be
            // checked, including a helper outlined outside SRAM.
            for root in [sram, cached] {
                let leaf = sram + 0x40;
                let helper = if root == sram { cached } else { leaf };
                let mut analysis = Analysis {
                    traps: Default::default(),
                    functions: BTreeMap::from([
                        (root, facts(root, entry, Some(helper))),
                        (helper, facts(helper, "outlined_helper", None)),
                    ]),
                    code: Vec::new(),
                    summaries: BTreeSet::new(),
                };
                let names = BTreeMap::from([
                    (root, entry.to_owned()),
                    (helper, "outlined_helper".to_owned()),
                ]);
                let resolutions = oer_analysis_riscv_stack::Resolutions::new();
                let mut report = stacks(Vec::new());
                report.names = names.clone();
                report.panic_code = panic_path_code(
                    &analysis,
                    &names,
                    &resolutions,
                    &memory,
                    &mut ledger(),
                    &mut || Ok(()),
                )
                .unwrap();
                for policy in [Required::Product, Required::Diagnostic] {
                    let error = report.check(policy).unwrap_err().to_string();
                    assert!(error.contains("panic path"), "{entry}: {error}");
                    assert!(error.contains("cached memory"), "{entry}: {error}");
                }
                // Moving the whole path into uncached SRAM makes it pass.
                analysis.functions = BTreeMap::from([
                    (sram, facts(sram, entry, Some(leaf))),
                    (leaf, facts(leaf, "outlined_helper", None)),
                ]);
                let names = BTreeMap::from([
                    (sram, entry.to_owned()),
                    (leaf, "outlined_helper".to_owned()),
                ]);
                report.panic_code = panic_path_code(
                    &analysis,
                    &names,
                    &resolutions,
                    &memory,
                    &mut ledger(),
                    &mut || Ok(()),
                )
                .unwrap();
                assert!(report.check(Required::Product).is_ok());
            }
        }
    }

    #[test]
    fn an_sram_hook_cannot_copy_a_compiler_constant_from_cached_memory() {
        let mut report = stacks(Vec::new());
        let hook = timer();
        let constant = u64::from(report.contract.memory.psram.unwrap().origin) + 0x1000;
        report
            .names
            .insert(hook, "oer_platform_panic_hook".to_owned());
        report.panic_hook_data = vec![(hook, hook + 4, constant)];
        assert!(report.panic_code.cached.is_empty());
        for policy in [Required::Product, Required::Diagnostic] {
            let error = report.check(policy).unwrap_err().to_string();
            assert!(
                error.contains("panic hook references cached memory"),
                "{error}"
            );
            assert!(error.contains("oer_platform_panic_hook"), "{error}");
        }
        report.panic_hook_data.clear();
        assert!(report.check(Required::Product).is_ok());
    }

    #[test]
    fn a_partial_bound_passes_only_where_the_policy_allows_it_and_its_part_fits() {
        assert!(partial(1000).check(Required::Product).is_err());
        let warnings = partial(1000).check(Required::Diagnostic).unwrap();
        assert_eq!(warnings.len(), 1);
        assert!(
            warnings.iter().any(|warning| warning.contains("1000 + ?")),
            "{warnings:?}"
        );
        // The proven part alone must fit the usable stack with the margin.
        let over = u64::from(contract().irq_stack_usable_bytes(0));
        assert!(partial(over).check(Required::Diagnostic).is_err());
    }

    #[test]
    fn a_bound_on_an_admitted_assumption_passes_conditional() {
        let mut stacks = partial(1000);
        stacks.harts[0].bytes = Some(1000);
        stacks.allocations.insert(
            0,
            LinkedStack {
                bottom: 0,
                bytes: stacks.contract.interrupts.irq_stack_bytes,
                from_bound: true,
            },
        );
        assert!(stacks.render().contains(", proven\n"));
        assert!(stacks.render().contains("\tproven bound\n"));
        stacks.harts[0]
            .exception
            .bound
            .assumptions
            .insert(Assumption::ExecutorInvariant);
        // The gate names it in a warning and the report; neither the hart nor
        // the size source of its allocation reads proven.
        let report = stacks.render();
        assert!(
            report.contains(", conditional (ExecutorInvariant)\n"),
            "{report}"
        );
        assert!(
            report.contains("\tconditional bound (ExecutorInvariant)\n"),
            "{report}"
        );
        assert!(!report.contains("proven"), "{report}");
        let warnings = stacks.check(Required::Product).unwrap();
        assert_eq!(warnings.len(), 1);
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("executor invariant")),
            "{warnings:?}"
        );
        assert!(stacks.render().contains("assumes the executor invariant"));
    }

    #[test]
    fn a_handler_that_can_lower_the_level_leaves_its_hart_conditional() {
        // A bound with no instruction that lowers the level is checked.
        let mut stacks = partial(1000);
        stacks.harts[0].bytes = Some(1000);
        assert!(stacks.check(Required::Product).is_ok());
        assert!(stacks.render().contains("nesting: checked"));
        // One such instruction breaks the sum over levels: no proof, and a
        // warning under the diagnostic policy.
        stacks.harts[0].exception.bound.level_drops =
            vec![(0x2f00_1000, oer_analysis_riscv_stack::LevelDrop::Return)];
        let error = stacks.check(Required::Product).unwrap_err().to_string();
        assert!(error.contains("conditional"), "{error}");
        let warnings = stacks.check(Required::Diagnostic).unwrap();
        assert!(warnings[0].contains("nesting rule"), "{warnings:?}");
        assert!(stacks.render().contains("a trap return at 0x2f001000"));
    }

    #[test]
    fn a_level_writer_outside_its_contexts_fails_every_policy() {
        let mut stacks = partial(1000);
        stacks.harts[0].bytes = Some(1000);
        assert!(stacks.render().contains("levels: checked"));
        stacks.level_writers =
            vec!["`esp_hal::interrupt::map_raw` at 0x2f001000 runs inside main".into()];
        for required in [Required::Product, Required::Diagnostic] {
            let error = stacks.check(required).unwrap_err().to_string();
            assert!(error.contains("table levels are not checked"), "{error}");
        }
        assert!(stacks.render().contains("levels: not checked"));
    }

    #[test]
    fn floating_point_in_a_handler_fails_and_the_rest_is_reported() {
        let mut stacks = partial(1000);
        stacks.harts[0].bytes = Some(1000);
        stacks.handler_code.panics = vec![(0x2f00_1000, 0x4000_0000)];
        stacks.handler_code.cached = vec![0x4000_0000];
        assert!(stacks.check(Required::Product).is_ok());
        let report = stacks.render();
        assert!(
            report.contains("1 calls into the panic machinery"),
            "{report}"
        );
        assert!(report.contains("1 functions in cached memory"), "{report}");
        stacks.handler_code.float = vec![(0x2f00_1000, 0x2f00_1004)];
        let error = stacks.check(Required::Diagnostic).unwrap_err().to_string();
        assert!(error.contains("floating point at 0x2f001004"), "{error}");
    }

    #[test]
    fn the_margin_is_a_share_of_the_bound() {
        assert_eq!(
            contract().interrupts.irq_stack_with_margin(1000).unwrap(),
            1000 + 1000 * u64::from(contract().interrupts.irq_stack_margin_percent) / 100
        );
    }
}
