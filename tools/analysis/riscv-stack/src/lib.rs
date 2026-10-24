//! Worst-case stack bounds of a static RV32 image from its machine code,
//! with the companions it calls into, such as the chip's ROM ELF.
//!
//! Every function is a defined code symbol of the image. Its frame is the
//! compiler's `.stack_sizes` entry, cross-checked with the deepest entry-relative
//! `sp` the bounded value analysis of `oer-analysis-riscv-analysis` observes; a function
//! without an entry takes the observed depth only when its control-flow graph
//! is complete. Its callees are every direct call and out-of-function jump of a
//! linear sweep over its whole extent, which also reaches code behind jump
//! tables the control-flow graph does not expand, plus the transfers whose
//! target the value analysis resolves. A transfer through `table[index]`
//! reaches each entry of the table when its length is exact: a bounds check
//! or mask on the index, or the size of a data object the table starts in an
//! unwritable section, such as an interrupt handler table. The sweep already
//! covers a jump's entries inside the function; a table of unknown length
//! stays unresolved. A function's bound is its frame or, if
//! deeper, a callee's bound below the `sp` of the transfer that reaches it: the
//! value analysis's depth at that site, or the whole frame at a site it did not
//! reach. It never underestimates the code it reaches.
//!
//! Bounds fail closed: a root has a number only when nothing it reaches is
//! unresolved. Otherwise [`Bound::reasons`] counts what is, by reason, and
//! [`Reason`] names the stage that closes it.

#![forbid(unsafe_code)]

mod cha;
mod contexts;
mod dwarf;
mod image;
mod interrupts;
mod producers;
mod recursion;
mod relocations;
mod resolutions;
mod summaries;
mod sweep;
mod task_spawns;
mod tasks;
mod trap;
mod trap_semantics;
mod wakers;
#[cfg(test)]
mod write_summary_tests;

use oer_analysis_riscv_model::{
    CallBehavior, CodeRange, Error, ErrorCode, FunctionRecord, JumpDispatch, Ledger, Result,
    RunControl, WorkingMemory,
};
use oer_analysis_riscv_program::facts::ImageFacts;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

pub use cha::{
    AddressUseAudit, CallbackLoadField, FunctionPointerAudit, FunctionPointerType, PcRelativeUse,
    StorageEvidence, TypeFacts, TypedStore, UnknownUse, audit_function_addresses,
    function_pointer_resolutions, typed_function_pointer_resolutions,
};
pub use contexts::{HartStack, LevelStack, Stacks, interrupt_stacks};
pub use dwarf::{DebugMetadata, Dwarf, read as read_debug};
pub use image::{Function, stack_sizes};
pub use interrupts::{Field, TableEntry, TableLayout, interrupt_table};
pub use oer_analysis_riscv_analysis::TrapSemantics;
use oer_analysis_riscv_analysis::{JumpTableSource, KnownJump, ProvenTables, RecognizedTables};
use oer_analysis_riscv_program::tables::ImageTables;
pub use producers::{BaseBarrier, LoadBase, RegisterWitness};
pub use relocations::taken_addresses;
pub use resolutions::{Assumption, Fact, Resolution, Resolutions, task_poll_sites};
pub use summaries::{Summary, parse as parse_summaries};
pub use sweep::{LevelDrop, Load, TableBase, TargetSource, Transfer, TransferKind};
pub use tasks::task_poll_resolutions;
pub use trap::{TrapEntry, trap_entry, vector_table};
pub use trap_semantics::{TrapFact, TrapFailure, prove_traps};
pub use wakers::{waker_resolutions, waker_vtables};

/// The common image-derived resolutions. Reuses the already captured ELF,
/// analysis and typed DWARF; callers add platform interrupt routing facts.
#[allow(clippy::too_many_arguments)]
pub fn image_resolutions<'a>(
    image: &ImageFacts<'_>,
    analysis: &Analysis,
    dwarf: &Dwarf,
    types: &TypeFacts,
    names: impl IntoIterator<Item = (u32, &'a str)>,
    keep: &mut Ledger<'_>,
    control: &mut dyn RunControl,
) -> Result<Resolutions> {
    let vtables = waker_vtables(image, dwarf, analysis, keep, control)?;
    let mut resolutions = waker_resolutions(analysis, dwarf, &vtables, keep, control)?;
    let audit = audit_function_addresses(image, analysis, types, keep, control)?;
    let pointers = function_pointer_resolutions(&audit, keep, control)?;
    resolutions.extend(task_poll_resolutions(
        image, analysis, dwarf, types, names, keep, control,
    )?);
    resolutions.extend(pointers);
    Ok(resolutions)
}

/// Where a function's frame comes from.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FrameSource {
    /// The compiler's `.stack_sizes` entry, equal to or above the observed depth.
    StackSizes,
    /// The deepest observed `sp` of a complete control-flow graph.
    Observed,
    /// A reviewed summary of a companion function the machine code alone
    /// does not bound.
    Summary,
}

/// What the analysis established about one function.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FunctionFacts {
    pub function: Function,
    /// Bytes below the entry `sp`, or `None` when no source establishes it.
    pub frame: Option<u64>,
    pub source: Option<FrameSource>,
    /// The deepest entry-relative `sp` the value analysis observed.
    pub observed: Option<u64>,
    /// Whether the control-flow graph covered the function without gaps.
    pub complete: bool,
    /// This function reached its local expression capacity.
    pub expression_limited: bool,
    /// Expression-capacity and value-alternative typed events, in that order.
    pub limit_events: [u64; 2],
    /// Every direct self-call constructs inputs that execute no further
    /// direct self-call, proved from instructions and stack payload bytes.
    pub terminal_recursive_callees: bool,
    /// Direct calls and out-of-function jumps, by site.
    pub transfers: Vec<Transfer>,
    /// The indirect sites whose targets the machine code itself gives, by
    /// site: their [`Self::transfers`] carry the targets.
    pub code_facts: BTreeMap<u32, CodeFact>,
    /// Every reaching word-load definition of each unresolved transfer's
    /// target, only when the local graph and every incoming path prove it.
    pub target_loads: BTreeMap<u32, Vec<Load>>,
    /// Target loads proved with only the transfer's own indirect exit open.
    /// A consumer must prove every target lies outside this function before
    /// using these; ordinary callback and base provenance excludes them.
    pub open_exit_target_loads: BTreeMap<u32, Vec<Load>>,
    /// Earlier equal register values for each target load's address base,
    /// bounded by the first merge, definition, memory write or unknown step.
    pub load_bases: BTreeMap<u32, LoadBase>,
    /// Bytes below the entry `sp` at each transfer the value analysis
    /// reached; a transfer it did not reach is taken at the whole frame.
    pub site_depths: BTreeMap<u32, u64>,
    /// The exact argument registers `a0..a7` at each direct call the value
    /// analysis reached, by site; `None` where a value is not exact.
    pub call_arguments: BTreeMap<u32, [Option<u32>; 8]>,
    /// The base of the table each unresolved transfer loads its target
    /// from as `table[index]`, where it is known, by site.
    pub table_bases: BTreeMap<u32, u32>,
    /// The exact integer registers at each unresolved transfer the value
    /// analysis reached, by site.
    pub unresolved_registers: BTreeMap<u32, [Option<u32>; 32]>,
    /// Instructions that can lower the interrupt level, by site.
    pub level_drops: Vec<(u32, LevelDrop)>,
    /// Floating-point instructions and floating-point CSR accesses, by site.
    pub float_sites: Vec<u32>,
}

/// How the machine code itself gives an indirect site's targets, with no
/// fact from outside it and no assumption.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum CodeFact {
    /// A table of exactly known entries: the link's relocations, a sized
    /// data object, or a bounds check or mask on the index.
    JumpTable,
    /// The value analysis: the target register holds known values at the
    /// site on every path.
    ValueAnalysis,
}

/// What an indirect site a bound's walk took reaches, and how its targets
/// are known.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TakenSite {
    /// The distinct functions (or exits) the site reaches.
    pub targets: usize,
    pub source: SiteSource,
}

/// Where a taken indirect site's targets come from.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SiteSource {
    /// The machine code itself.
    Code(CodeFact),
    /// Facts outside the machine code ([`Resolutions`]); each names what it
    /// assumes ([`Fact::assumption`]).
    Facts(BTreeSet<Fact>),
}

impl SiteSource {
    /// What the site's targets assume: none for the machine code's own.
    pub fn assumptions(&self) -> BTreeSet<Assumption> {
        match self {
            Self::Code(_) => BTreeSet::new(),
            Self::Facts(facts) => facts.iter().filter_map(|fact| fact.assumption()).collect(),
        }
    }
}

/// Why a reachable site or function leaves a bound unknown.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum Reason {
    /// An indirect call through a pointer the frame keeps across calls,
    /// which the local value analysis does not follow.
    StackSlotCall,
    /// An indirect call through a loaded vtable entry or function pointer.
    LoadedCall,
    /// An indirect call through another register value.
    RegisterCall,
    /// An indirect jump that is not a table of known length: a tail call
    /// through a pointer, such as `core::fmt::write` to `write_str`, or a
    /// `match` whose jump table no bounds check limits.
    IndirectJump,
    /// A transfer to an address outside the image's code: ROM.
    OutsideImage,
    /// A transfer into the middle of a function.
    IntoFunction,
    /// A function without a frame record and with an incomplete control-flow
    /// graph: hand-written assembly.
    Unframed,
    /// A call-graph cycle.
    Recursion,
    /// An indirect site whose facts found no target and do not prove it
    /// reaches none: a waker call without vtables, a field without candidates.
    NoCandidate,
}

impl Reason {
    /// The stage of the stack analysis that resolves this reason.
    pub fn closed_by(self) -> &'static str {
        match self {
            Reason::StackSlotCall => "2: MIR call facts",
            Reason::LoadedCall | Reason::RegisterCall => "2: MIR call facts",
            Reason::IndirectJump => "2: MIR call facts",
            Reason::OutsideImage => "1c: ROM companion",
            Reason::IntoFunction => "review",
            Reason::Unframed => "1c: assembly frames",
            Reason::Recursion => "a reviewed recursion bound",
            Reason::NoCandidate => "2: MIR call facts",
        }
    }
}

impl fmt::Display for Reason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Reason::StackSlotCall => "indirect call through a stack slot",
            Reason::LoadedCall => "indirect call through a loaded pointer",
            Reason::RegisterCall => "indirect call through a register",
            Reason::IndirectJump => "indirect jump out of a function",
            Reason::OutsideImage => "transfer outside the image",
            Reason::IntoFunction => "transfer into a function's middle",
            Reason::Unframed => "function without a frame",
            Reason::Recursion => "recursion",
            Reason::NoCandidate => "indirect site whose facts found no target",
        })
    }
}

/// A root's worst-case bound, or why it has none.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Bound {
    pub root: u32,
    /// Bytes below the root's entry `sp` on its deepest path; `None` unless
    /// nothing the root reaches is unresolved.
    pub bytes: Option<u64>,
    /// Bytes on the deepest path over what is resolved: the bound when
    /// nothing is unresolved, otherwise neither an upper nor a lower bound
    /// (`partial + ?`), since a hole may reach deeper and a path may be
    /// infeasible.
    pub partial: u64,
    /// What the root reaches that is unresolved: the site (or, for an
    /// unframed function or a cycle, the function) and its reason.
    pub unresolved: Vec<(u32, Reason)>,
    /// Deepest path over what is resolved, root first, with each function's
    /// frame. A partial path when [`Self::bytes`] is `None`.
    pub path: Vec<(u32, u64)>,
    /// Instructions that can lower the interrupt level in every function the
    /// root reaches over what is resolved.
    pub level_drops: Vec<(u32, LevelDrop)>,
    /// What the resolutions the root's walk took assume: a bound with any
    /// is conditional on them.
    pub assumptions: BTreeSet<Assumption>,
    /// Every indirect site the walk took, by site: the facts behind
    /// [`Self::assumptions`] and the machine code's own resolutions.
    pub sites: BTreeMap<u32, TakenSite>,
    /// Every function the root reaches over what is resolved, the root
    /// included: what checks of a context's code read.
    pub reached: BTreeSet<u32>,
    /// One actually traversed edge per non-root function, recorded at first
    /// discovery. Shared when interrupt vectors reuse a handler's bound.
    pub predecessors: std::rc::Rc<BTreeMap<u32, Predecessor>>,
}

/// The first edge by which a root's walk discovered a function.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Predecessor {
    pub caller: u32,
    pub site: u32,
}

impl Bound {
    /// One first-discovery witness, root first. Reconstructs parent links;
    /// it never searches the call graph. The root itself has an empty witness.
    pub fn witness(&self, target: u32) -> Option<Vec<(u32, Predecessor)>> {
        if !self.reached.contains(&target) {
            return None;
        }
        let mut path = Vec::new();
        let mut at = target;
        while at != self.root {
            if path.len() >= self.predecessors.len() {
                return None;
            }
            let parent = *self.predecessors.get(&at)?;
            path.push((at, parent));
            at = parent.caller;
        }
        path.reverse();
        Some(path)
    }

    /// The unresolved sites and functions, counted by reason.
    pub fn reasons(&self) -> BTreeMap<Reason, usize> {
        let mut counts = BTreeMap::new();
        for (_, reason) in &self.unresolved {
            *counts.entry(*reason).or_insert(0) += 1;
        }
        counts
    }
}

/// Frames, transfers and bounds of one image and its companions.
#[derive(Clone, Debug)]
pub struct Analysis {
    /// Illegal-instruction semantics used for every function.
    pub traps: TrapSemantics,
    pub functions: BTreeMap<u32, FunctionFacts>,
    /// Address ranges of the executable sections of every analysed ELF.
    pub code: Vec<(u32, u32)>,
    /// The summaries this image used: those of the functions it reaches.
    pub summaries: BTreeSet<String>,
}

/// Analyze every function of the static RV32 executable `image` and of the
/// `companions` it calls into, such as the chip's ROM ELF: a transfer from
/// one into another reaches the callee's frame like any other call. No two
/// ELFs may place code at the same address. `summaries` stand for companion
/// functions the machine code alone does not bound ([`Summary`]). The
/// facts it keeps are admitted in `keep`, which the caller holds while it
/// keeps them; what it builds on the way, the value analysis included, is
/// admitted in the same working memory and released with its function. It
/// charges `control`; running out of either fails the analysis with
/// [`ErrorCode::ResourceLimited`].
/// `call_behavior` supplies explicit proofs, such as the [`Dwarf`] facts
/// from [`read_debug`]; pass [`oer_analysis_riscv_model::UnknownCallBehavior`]
/// when no such facts are available.
pub fn analyze(
    image: &ImageFacts<'_>,
    companions: &[&ImageFacts<'_>],
    summaries: &[Summary],
    call_behavior: &dyn CallBehavior,
    keep: &mut Ledger<'_>,
    control: &mut dyn RunControl,
) -> Result<Analysis> {
    analyze_with_traps(
        image,
        companions,
        summaries,
        call_behavior,
        TrapSemantics::Unknown,
        keep,
        control,
    )
}

/// Analyze with an image-wide illegal-instruction fact, established at Unknown
/// by [`prove_traps`] before applying Terminal to any function.
#[allow(clippy::too_many_arguments)]
pub fn analyze_with_traps(
    image: &ImageFacts<'_>,
    companions: &[&ImageFacts<'_>],
    summaries: &[Summary],
    call_behavior: &dyn CallBehavior,
    traps: TrapSemantics,
    keep: &mut Ledger<'_>,
    control: &mut dyn RunControl,
) -> Result<Analysis> {
    let memory = keep.memory();
    let mut analysis = Analysis {
        traps,
        functions: BTreeMap::new(),
        code: Vec::new(),
        summaries: BTreeSet::new(),
    };
    // The image's own functions, for the summaries; released with the call.
    let mut scratch = Ledger::new(memory, control)?;
    let mut own = None;
    for elf in std::iter::once(image).chain(companions.iter().copied()) {
        let Analysis {
            functions, code, ..
        } = analyze_one(elf, call_behavior, traps, memory, keep, control)?;
        for &(start, end) in &code {
            if analysis
                .code
                .iter()
                .any(|&(other_start, other_end)| start < other_end && other_start < end)
            {
                return Err(Error::new(
                    ErrorCode::Integrity,
                    format!("code at {start:#010x}..{end:#010x} overlaps another ELF's"),
                ));
            }
        }
        for range in code {
            keep.push(&mut analysis.code, range, control)?;
        }
        if own.is_none() {
            let mut addresses = BTreeSet::new();
            for &address in functions.keys() {
                scratch.add(&mut addresses, address, control)?;
            }
            own = Some(addresses);
        }
        analysis.functions.extend(functions);
    }
    analysis.summaries = summaries::apply(
        &mut analysis,
        &own.unwrap_or_default(),
        summaries,
        keep,
        control,
    )?;
    Ok(analysis)
}

/// The facts and executable ranges of one ELF.
fn analyze_one(
    elf: &ImageFacts<'_>,
    call_behavior: &dyn CallBehavior,
    traps: TrapSemantics,
    memory: &WorkingMemory,
    keep: &mut Ledger<'_>,
    control: &mut dyn RunControl,
) -> Result<Analysis> {
    let mut sized = Ledger::new(memory, control)?;
    let sizes = image::stack_sizes(elf, &mut sized, control)?;
    let functions = image::observing(
        elf,
        call_behavior,
        traps,
        memory,
        control,
        |observe, control| {
            let mut facts = BTreeMap::new();
            for function in elf.functions() {
                // What one function's analysis builds, released once its
                // facts are admitted in what the analysis keeps.
                let mut work = Ledger::new(memory, control)?;
                let names = function.names.iter().map(String::len).sum::<usize>()
                    + function.names.len() * size_of::<String>();
                work.admit(names, control)?;
                let function =
                    function_facts(elf, function.clone(), &sizes, observe, &mut work, control)?;
                keep.admit(function.requested_bytes(), control)?;
                keep.insert(&mut facts, function.function.address, function, control)?;
            }
            Ok(facts)
        },
    )?;
    let mut code = Vec::new();
    for &range in elf.executable_ranges() {
        keep.push(&mut code, range, control)?;
    }
    Ok(Analysis {
        traps,
        functions,
        code,
        summaries: BTreeSet::new(),
    })
}

/// The base of the table a transfer loads its target from, when known:
/// a constant, or the exact value of its register at the site.
fn table_base(transfer: &Transfer, registers: Option<&[Option<u32>; 32]>) -> Option<u32> {
    match transfer.table {
        Some(TableBase::Address(address)) => Some(address),
        Some(TableBase::Register(r)) => registers.and_then(|registers| registers[r as usize]),
        None => None,
    }
}

/// The entries of the table a transfer loads its target from, when the
/// table is a sized object or a compiler jump table the relocations name.
fn table_entries(
    elf: &ImageFacts<'_>,
    transfer: &Transfer,
    registers: Option<&[Option<u32>; 32]>,
    ledger: &mut Ledger<'_>,
    control: &mut dyn RunControl,
) -> Result<Option<Vec<u32>>> {
    let Some(address) = table_base(transfer, registers) else {
        return Ok(None);
    };
    match image::table(elf, address, ledger, control)? {
        Some(entries) => Ok(Some(entries)),
        None => image::jump_table(elf, address, ledger, control),
    }
}

/// The jump that follows a table's `entries` from `site` inside `function`:
/// its distinct non-null entries there, as code addresses in ascending
/// order.
fn table_jump(function: &Function, site: u32, entries: &[u32]) -> KnownJump {
    let inside =
        |target: u32| target >= function.address && target - function.address < function.size;
    let mut targets: Vec<u64> = entries
        .iter()
        .filter(|&&entry| entry != 0)
        .map(|entry| entry & !1)
        .filter(|&target| inside(target))
        .map(u64::from)
        .collect();
    targets.sort_unstable();
    targets.dedup();
    KnownJump {
        site: u64::from(site),
        targets,
    }
}

/// Inspect one function with the same swept and common-engine table proofs
/// used by the stack analysis. The sink receives canonical machine facts;
/// names and source aggregate layouts do not select effects.
pub fn inspect_function(
    image: &ImageFacts<'_>,
    function: &Function,
    call_behavior: &dyn CallBehavior,
    options: oer_analysis_riscv_analysis::ResearchOptions<'_>,
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
    sink: &mut dyn oer_analysis_riscv_model::FunctionSink,
) -> Result<()> {
    inspect_function_as(
        image,
        function,
        call_behavior,
        options,
        &[],
        &oer_analysis_riscv_lift::StructuralRiscvDecoder,
        TrapSemantics::Unknown,
        memory,
        control,
        sink,
    )
}

/// The same pipeline after a checked terminal-transfer refinement.
#[allow(clippy::too_many_arguments)]
pub fn inspect_stack_context(
    image: &ImageFacts<'_>,
    function: &Function,
    call_behavior: &dyn CallBehavior,
    options: oer_analysis_riscv_analysis::ResearchOptions<'_>,
    extra: &[KnownJump],
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
    sink: &mut dyn oer_analysis_riscv_model::FunctionSink,
) -> Result<()> {
    inspect_function_as(
        image,
        function,
        call_behavior,
        options,
        extra,
        &oer_analysis_riscv_lift::StackRiscvDecoder,
        TrapSemantics::Unknown,
        memory,
        control,
        sink,
    )
}

#[allow(clippy::too_many_arguments)]
fn inspect_function_as(
    image: &ImageFacts<'_>,
    function: &Function,
    call_behavior: &dyn CallBehavior,
    options: oer_analysis_riscv_analysis::ResearchOptions<'_>,
    extra: &[KnownJump],
    decoder: &dyn oer_analysis_riscv_model::FunctionSemantics,
    traps: TrapSemantics,
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
    sink: &mut dyn oer_analysis_riscv_model::FunctionSink,
) -> Result<()> {
    use oer_analysis_riscv_analysis::{
        FunctionInput, PreparedReferences, research_following_tables_with_options,
    };
    use oer_analysis_riscv_model::NoSummaries;
    use oer_analysis_riscv_program::ProgramView;
    let mut work = Ledger::new(memory, control)?;
    let swept = sweep::transfers(image, function, &mut work, control)?;
    let mut jumps = Vec::new();
    for jump in extra {
        control.checkpoint(1 + jump.targets.len() as u64)?;
        work.admit(jump.targets.len() * size_of::<u64>(), control)?;
        work.push(&mut jumps, jump.clone(), control)?;
    }
    for (site, entries) in &swept.jumps {
        work.admit(entries.len() * size_of::<u64>(), control)?;
        work.push(&mut jumps, table_jump(function, *site, entries), control)?;
    }
    let references = PreparedReferences::empty();
    let view = ProgramView::new(image.bytes(), image.file(), &[], memory, control)?;
    research_following_tables_with_options(
        FunctionInput {
            traps,
            image: Some(&view),
            section: function.section as u32,
            extent: CodeRange {
                start: function.address.into(),
                length: function.size.into(),
            },
            bytes: image::function_bytes(image.file(), function)?,
            relocations: &references,
            data_ranges: &[],
            jumps: &jumps,
            returns: &NoSummaries,
            call_behavior,
        },
        &DispatchTables {
            recognized: RecognizedTables {
                entries: &ImageTables { image },
                section: function.section as u32,
            },
            known: &jumps,
        },
        decoder,
        memory,
        control,
        sink,
        options,
    )?;
    Ok(())
}

/// The tables of a function's dispatches: the value analysis's `Dispatch`
/// records whose entries the image's facts prove ([`RecognizedTables`] over
/// [`ImageTables`]), but for the sites the sweep's own jumps already follow.
struct DispatchTables<'a> {
    recognized: RecognizedTables<'a>,
    known: &'a [KnownJump],
}

impl JumpTableSource for DispatchTables<'_> {
    type Table = JumpDispatch;

    fn same(&self, earlier: &JumpDispatch, later: &JumpDispatch) -> bool {
        self.recognized.same(earlier, later)
    }

    fn tables<'m>(
        &self,
        records: &[FunctionRecord],
        extent: CodeRange,
        memory: &'m WorkingMemory,
        control: &mut dyn RunControl,
    ) -> Result<ProvenTables<'m, JumpDispatch>> {
        let mut found = self.recognized.tables(records, extent, memory, control)?;
        found
            .tables
            .retain(|table| !self.known.iter().any(|jump| jump.site == table.site()));
        Ok(found)
    }

    fn jump(&self, table: &JumpDispatch) -> KnownJump {
        self.recognized.jump(table)
    }
}

/// One function's facts: its sweep, then the value analysis over a graph
/// that follows the jump tables the sweep names, and those its `Dispatch`
/// records locate ([`DispatchTables`]). What it builds on the way is
/// admitted in `work`.
fn function_facts(
    elf: &ImageFacts<'_>,
    function: Function,
    sizes: &BTreeMap<u32, u64>,
    observe: &mut image::Observe<'_>,
    work: &mut Ledger<'_>,
    control: &mut dyn RunControl,
) -> Result<FunctionFacts> {
    control.checkpoint(u64::from(function.size))?;
    let swept = sweep::transfers(elf, &function, work, control)?;
    let inside =
        |target: u32| target >= function.address && target - function.address < function.size;
    let mut jumps: Vec<KnownJump> = Vec::new();
    for (site, entries) in &swept.jumps {
        work.admit(entries.len() * size_of::<u64>(), control)?;
        let jump = table_jump(&function, *site, entries);
        work.push(&mut jumps, jump, control)?;
    }
    let tables = DispatchTables {
        recognized: RecognizedTables {
            entries: &ImageTables { image: elf },
            section: function.section as u32,
        },
        known: &jumps,
    };
    let observation = observe(&function, &jumps, &tables, work, control)?;
    let mut transfers = swept.transfers;
    for transfer in &mut transfers {
        if let Some(load) = &mut transfer.load {
            load.exclusive_producer = observation.closed_graph()
                && observation
                    .predecessors
                    .get(&transfer.site)
                    .is_some_and(|incoming| incoming.as_slice() == [load.site]);
        }
    }
    let producers::TargetLoads {
        closed: target_loads,
        open_exit: open_exit_target_loads,
    } = producers::loads(elf, &function, &transfers, &observation, work, control)?;
    let load_bases = producers::bases(elf, &function, &target_loads, &observation, work, control)?;
    let mut table_bases = BTreeMap::new();
    let mut unresolved_registers = BTreeMap::new();
    for t in transfers.iter().filter(|t| t.target.is_none()) {
        let registers = observation.site_registers.get(&t.site);
        if let Some(base) = table_base(t, registers) {
            work.insert(&mut table_bases, t.site, base, control)?;
        }
        if let Some(registers) = registers {
            work.insert(&mut unresolved_registers, t.site, *registers, control)?;
        }
    }
    // The indirect sites before a table or the value analysis resolves them.
    let mut indirect = BTreeSet::new();
    for transfer in transfers.iter().filter(|t| t.target.is_none()) {
        work.add(&mut indirect, transfer.site, control)?;
    }
    let mut dispatched = Vec::new();
    // Sites whose table the link's relocations or a sized object name:
    // their entries replace whatever the value analysis read.
    let mut tabled = BTreeSet::new();
    for transfer in std::mem::take(&mut transfers) {
        let Some(entries) = table_entries(
            elf,
            &transfer,
            observation.site_registers.get(&transfer.site),
            work,
            control,
        )?
        else {
            work.push(&mut dispatched, transfer, control)?;
            continue;
        };
        work.add(&mut tabled, transfer.site, control)?;
        let mut targets = Vec::new();
        // A Rust function pointer is never null: a zero entry is an empty
        // `Option<fn>` slot the code tests before calling.
        for &entry in &entries {
            let target = entry & !1;
            if entry != 0 && (transfer.kind == TransferKind::Call || !inside(target)) {
                work.push(&mut targets, target, control)?;
            }
        }
        targets.sort_unstable();
        targets.dedup();
        for target in targets {
            let transfer = Transfer {
                target: Some(target),
                source: None,
                table: None,
                load: None,
                ..transfer
            };
            work.push(&mut dispatched, transfer, control)?;
        }
    }
    transfers = dispatched;
    // A jump whose every target the value analysis found inside the
    // function stays there: the sweep covers its code.
    transfers.retain(|t| t.target.is_some() || !observation.local_jumps.contains(&t.site));
    // The value analysis's targets of a site are a superset of its
    // runtime targets; a jump's targets inside the function are already
    // in the sweep.
    for (site, target, kind) in observation.resolved {
        if tabled.contains(&site) {
            continue;
        }
        transfers.retain(|t| !(t.site == site && t.target.is_none()));
        if kind == TransferKind::Tail && inside(target) {
            continue;
        }
        if !transfers
            .iter()
            .any(|t| t.site == site && t.target == Some(target))
        {
            let transfer = Transfer {
                site,
                target: Some(target),
                kind,
                source: None,
                table: None,
                load: None,
            };
            work.push(&mut transfers, transfer, control)?;
        }
    }
    transfers.sort_by_key(|t| (t.site, t.target));
    let mut code_facts = BTreeMap::new();
    for transfer in transfers.iter().filter(|t| t.target.is_some()) {
        let fact = if tabled.contains(&transfer.site)
            || swept.tables.binary_search(&transfer.site).is_ok()
        {
            CodeFact::JumpTable
        } else if indirect.contains(&transfer.site) {
            CodeFact::ValueAnalysis
        } else {
            continue;
        };
        work.insert(&mut code_facts, transfer.site, fact, control)?;
    }
    let mut call_arguments = BTreeMap::new();
    for t in &transfers {
        if t.kind == TransferKind::Call
            && t.target.is_some()
            && let Some(registers) = observation.site_registers.get(&t.site)
        {
            let arguments: [Option<u32>; 8] = core::array::from_fn(|i| registers[10 + i]);
            work.insert(&mut call_arguments, t.site, arguments, control)?;
        }
    }
    let depth = observation.depth;
    let recorded = sizes.get(&function.address).copied();
    let (frame, source) = match (recorded, depth) {
        (Some(recorded), Some(depth)) if depth > recorded => {
            return Err(Error::new(
                ErrorCode::Integrity,
                format!(
                    "{}: observed sp depth {depth} exceeds its .stack_sizes {recorded}",
                    function.label()
                ),
            ));
        }
        (Some(recorded), _) => (Some(recorded), Some(FrameSource::StackSizes)),
        (None, Some(depth)) if observation.complete => (Some(depth), Some(FrameSource::Observed)),
        (None, _) => (None, None),
    };
    let terminal_recursive_callees =
        recursion::terminal_callees(elf, &function, &transfers, &swept.jumps, work, control)?;
    Ok(FunctionFacts {
        function,
        frame,
        source,
        observed: depth,
        call_arguments,
        table_bases,
        unresolved_registers,
        complete: observation.complete,
        expression_limited: observation.expression_limited,
        limit_events: observation.limit_events,
        terminal_recursive_callees,
        target_loads,
        open_exit_target_loads,
        load_bases,
        transfers,
        code_facts,
        site_depths: observation.site_depths,
        level_drops: swept.level_drops,
        float_sites: swept.float_sites,
    })
}

impl FunctionFacts {
    /// The heap bytes these facts hold, as a [`Ledger`] admits them: what
    /// the analysis keeps of the function once its work is released.
    fn requested_bytes(&self) -> usize {
        let names = &self.function.names;
        names.capacity() * size_of::<String>()
            + names.iter().map(String::capacity).sum::<usize>()
            + self.transfers.capacity() * size_of::<Transfer>()
            + self.target_loads.len() * size_of::<(u32, Vec<Load>)>()
            + self.open_exit_target_loads.len() * size_of::<(u32, Vec<Load>)>()
            + self.load_bases.len() * size_of::<(u32, LoadBase)>()
            + self
                .load_bases
                .values()
                .map(|base| base.witnesses.capacity() * size_of::<RegisterWitness>())
                .sum::<usize>()
            + self
                .target_loads
                .values()
                .chain(self.open_exit_target_loads.values())
                .map(|loads| loads.capacity() * size_of::<Load>())
                .sum::<usize>()
            + self.code_facts.len() * size_of::<(u32, CodeFact)>()
            + self.site_depths.len() * size_of::<(u32, u64)>()
            + self.call_arguments.len() * size_of::<(u32, [Option<u32>; 8])>()
            + self.table_bases.len() * size_of::<(u32, u32)>()
            + self.unresolved_registers.len() * size_of::<(u32, [Option<u32>; 32])>()
            + self.level_drops.capacity() * size_of::<(u32, LevelDrop)>()
            + self.float_sites.capacity() * size_of::<u32>()
    }
}

impl Analysis {
    /// Number of distinct functions that reached their expression capacity.
    pub fn expression_limited_functions(&self) -> usize {
        self.functions
            .values()
            .filter(|facts| facts.expression_limited)
            .count()
    }

    /// The worst-case bound of the function at `root`. First-discovery
    /// predecessor storage is admitted in `keep`, which the caller holds
    /// while the bound counts against its operation's working memory.
    pub fn bound(
        &self,
        root: u32,
        keep: &mut Ledger<'_>,
        control: &mut dyn RunControl,
    ) -> Result<Bound> {
        self.bound_with(root, &Resolutions::new(), keep, control)
    }

    /// [`Self::bound`] with `resolutions` for the indirect sites the
    /// analysis left unresolved: such a site reaches each of its targets.
    pub fn bound_with(
        &self,
        root: u32,
        resolutions: &Resolutions,
        keep: &mut Ledger<'_>,
        control: &mut dyn RunControl,
    ) -> Result<Bound> {
        self.bound_cut(root, resolutions, None, keep, control)
    }

    /// Reachability under the same rules as [`Bound::reached`], without
    /// following transfers at `cut` sites. Their owning functions remain
    /// reached. An empty cut gives exactly [`Self::bound_with`]'s inventory.
    pub fn reached_cut(
        &self,
        root: u32,
        resolutions: &Resolutions,
        cut: &BTreeSet<u32>,
        keep: &mut Ledger<'_>,
        control: &mut dyn RunControl,
    ) -> Result<BTreeSet<u32>> {
        Ok(self
            .bound_cut(root, resolutions, Some(cut), keep, control)?
            .reached)
    }

    fn bound_cut(
        &self,
        root: u32,
        resolutions: &Resolutions,
        cut: Option<&BTreeSet<u32>>,
        keep: &mut Ledger<'_>,
        control: &mut dyn RunControl,
    ) -> Result<Bound> {
        if !self.functions.contains_key(&root) {
            return Err(Error::new(
                ErrorCode::NotFound,
                format!("{root:#010x} is not a function start"),
            ));
        }
        let mut walk = Walk {
            analysis: self,
            resolutions,
            cut,
            memo: BTreeMap::new(),
            stack: Vec::new(),
            unresolved: BTreeSet::new(),
            assumptions: BTreeSet::new(),
            sites: BTreeMap::new(),
            root,
            predecessors: BTreeMap::new(),
            keep,
            control,
        };
        let (bytes, path) = walk.visit(root)?;
        // An unfolded recursive function has both ordinary and terminal
        // memo entries; code checks need each reached function only once.
        let reached: BTreeSet<_> = walk.memo.keys().map(|(address, _)| *address).collect();
        let level_drops = reached
            .iter()
            .flat_map(|function| self.functions[function].level_drops.iter().copied())
            .collect();
        walk.keep
            .admit(size_of::<BTreeMap<u32, Predecessor>>(), walk.control)?;
        Ok(Bound {
            root,
            bytes: walk.unresolved.is_empty().then_some(bytes),
            partial: bytes,
            unresolved: walk.unresolved.into_iter().collect(),
            path,
            level_drops,
            assumptions: walk.assumptions,
            sites: walk.sites,
            reached,
            predecessors: std::rc::Rc::new(walk.predecessors),
        })
    }

    /// The exact values argument register `a{register}` holds at every
    /// direct call of `callee`, or the first call site where it is not
    /// exact. Complete only for a callee whose address nothing takes
    /// ([`taken_addresses`]): every call of it is then a direct one.
    pub fn constant_arguments(&self, callee: u32, register: usize) -> Result<BTreeSet<u32>> {
        let mut values = BTreeSet::new();
        for facts in self.functions.values() {
            for transfer in &facts.transfers {
                if transfer.kind != TransferKind::Call || transfer.target != Some(callee) {
                    continue;
                }
                let value = facts
                    .call_arguments
                    .get(&transfer.site)
                    .and_then(|arguments| arguments.get(register).copied().flatten())
                    .ok_or_else(|| {
                        Error::new(
                            ErrorCode::Unavailable,
                            format!(
                                "a{register} at the call of {callee:#010x} at {:#010x} is not exact",
                                transfer.site
                            ),
                        )
                    })?;
                values.insert(value);
            }
        }
        Ok(values)
    }

    fn in_code(&self, address: u32) -> bool {
        self.code
            .iter()
            .any(|&(start, end)| address >= start && address < end)
    }
}

type DepthPath = (u64, Vec<(u32, u64)>);

struct Walk<'a, 'k, 'm> {
    analysis: &'a Analysis,
    resolutions: &'a Resolutions,
    cut: Option<&'a BTreeSet<u32>>,
    // A terminal recursive callee still has its full frame and nonrecursive
    // callees, but its direct self-edges are infeasible. Never reuse that
    // summary for an ordinary invocation of the same address.
    memo: BTreeMap<(u32, bool), DepthPath>,
    stack: Vec<(u32, bool)>,
    unresolved: BTreeSet<(u32, Reason)>,
    assumptions: BTreeSet<Assumption>,
    sites: BTreeMap<u32, TakenSite>,
    root: u32,
    predecessors: BTreeMap<u32, Predecessor>,
    keep: &'k mut Ledger<'m>,
    control: &'k mut dyn RunControl,
}

impl Walk<'_, '_, '_> {
    /// The deepest bytes below `address`'s entry `sp` over what is resolved,
    /// with its path. Unknown frames count as zero; they are reported.
    fn visit(&mut self, address: u32) -> Result<(u64, Vec<(u32, u64)>)> {
        self.visit_context(address, false)
    }

    fn visit_context(&mut self, address: u32, terminal: bool) -> Result<(u64, Vec<(u32, u64)>)> {
        let key = (address, terminal);
        // Only the deliberately unfolded direct self-call may reenter the
        // same address. An indirect or mutual cycle remains unresolved.
        if self.stack.iter().any(|&(at, _)| at == address)
            && !(terminal && self.stack.last() == Some(&(address, false)))
        {
            self.unresolved.insert((address, Reason::Recursion));
            return Ok((0, Vec::new()));
        }
        if let Some(known) = self.memo.get(&key) {
            return Ok(known.clone());
        }
        let facts = &self.analysis.functions[&address];
        let frame = facts.frame.unwrap_or_else(|| {
            self.unresolved.insert((address, Reason::Unframed));
            0
        });
        self.stack.push(key);
        // The deepest point of this function alone is its frame.
        let mut deepest = (frame, Vec::new());
        for transfer in &facts.transfers {
            if self.cut.is_some_and(|cut| cut.contains(&transfer.site)) {
                continue;
            }
            let direct_self = transfer.kind == TransferKind::Call
                && transfer.target == Some(address)
                && facts.terminal_recursive_callees;
            if terminal && direct_self {
                continue;
            }
            let targets: Vec<u32> = match (transfer.target, self.resolutions.get(transfer.site)) {
                (Some(target), _) => {
                    if let Some(&fact) = facts.code_facts.get(&transfer.site)
                        && !self.sites.contains_key(&transfer.site)
                    {
                        let targets = facts
                            .transfers
                            .iter()
                            .filter(|t| t.site == transfer.site)
                            .count();
                        let site = TakenSite {
                            targets,
                            source: SiteSource::Code(fact),
                        };
                        self.keep
                            .insert(&mut self.sites, transfer.site, site, self.control)?;
                    }
                    vec![target]
                }
                (None, Some(resolved)) if resolved.resolves() => {
                    self.assumptions
                        .extend(resolved.facts.iter().filter_map(|fact| fact.assumption()));
                    if !self.sites.contains_key(&transfer.site) {
                        let site = TakenSite {
                            targets: resolved.targets.len(),
                            source: SiteSource::Facts(resolved.facts.clone()),
                        };
                        self.keep
                            .insert(&mut self.sites, transfer.site, site, self.control)?;
                    }
                    resolved.targets.iter().copied().collect()
                }
                (None, Some(_)) => {
                    self.unresolved.insert((transfer.site, Reason::NoCandidate));
                    continue;
                }
                (None, None) => {
                    let reason = match (transfer.kind, transfer.source) {
                        (TransferKind::Tail, _) => Reason::IndirectJump,
                        (TransferKind::Call, Some(TargetSource::StackSlot)) => {
                            Reason::StackSlotCall
                        }
                        (TransferKind::Call, Some(TargetSource::Memory)) => Reason::LoadedCall,
                        (TransferKind::Call, _) => Reason::RegisterCall,
                    };
                    self.unresolved.insert((transfer.site, reason));
                    continue;
                }
            };
            let at = facts
                .site_depths
                .get(&transfer.site)
                .copied()
                .unwrap_or(frame);
            for target in targets {
                if self.resolutions.is_exit(target) {
                    // The path ends here; the site's own depth counts.
                    if at > deepest.0 {
                        deepest = (at, Vec::new());
                    }
                    continue;
                }
                if !self.analysis.functions.contains_key(&target) {
                    let reason = if self.analysis.in_code(target) {
                        Reason::IntoFunction
                    } else {
                        Reason::OutsideImage
                    };
                    self.unresolved.insert((transfer.site, reason));
                    continue;
                }
                if target != self.root && !self.predecessors.contains_key(&target) {
                    self.keep.insert(
                        &mut self.predecessors,
                        target,
                        Predecessor {
                            caller: address,
                            site: transfer.site,
                        },
                        self.control,
                    )?;
                }
                let below = if direct_self {
                    self.visit_context(target, true)
                } else {
                    self.visit(target)
                }?;
                if at + below.0 > deepest.0 {
                    deepest = (at + below.0, below.1);
                }
            }
        }
        self.stack.pop();
        let mut path = vec![(address, frame)];
        path.extend(deepest.1);
        let result = (deepest.0, path);
        self.memo.insert(key, result.clone());
        Ok(result)
    }
}

#[cfg(test)]
mod tests;
