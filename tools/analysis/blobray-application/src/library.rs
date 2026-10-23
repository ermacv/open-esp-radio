//! Every function captured libraries define, analyzed in process.
use crate::captured::{Member, visit_members};
use crate::in_process::Executable;
use crate::*;
use oer_analysis_blobray_analysis::navigation::Facts;
use oer_analysis_elf::{SymbolKind, classify as elf};
use std::collections::BTreeSet;
mod queries;
mod summaries;
pub use queries::{
    CallerResult, CallersSummary, FieldSummary, FunctionResult, QuerySummary, call_arguments,
    callers, field_accesses,
};

/// One analyzed function with its records, borrowed for a visit.
pub struct AnalyzedFunction<'a> {
    pub limits: FunctionLimits,
    pub function: &'a LibraryFunction,
    pub records: &'a [FunctionRecord],
    pub coverage: FunctionCoverage,
    pub semantics: SemanticSummary,
    /// Ordered tables or complete target sets the analysis followed.
    pub jump_tables: &'a [JumpDispatch],
    /// The shared callee port used for these final records.
    pub summaries: &'a dyn CallSummaries,
    pub conflicts: &'a [SignatureConflict],
    pub rom_signature_diagnostics: &'a [RomSignatureDiagnostic],
    pub summary_outcome: Option<SummaryOutcome<()>>,
    pub rom_signature_summary_outcome: Option<SummaryOutcome<()>>,
    pub signature_audit_outcome: Option<SummaryOutcome<()>>,
}
impl AnalyzedFunction<'_> {
    /// Coverage and value semantics are complete.
    pub fn complete(&self) -> bool {
        self.coverage.complete() && self.semantics.complete
    }

    /// This function's causes of being partial, each 0 or 1, all 0 when it
    /// is complete: its coverage, and the reasons of its semantic gaps, each
    /// visited record charged to `c`.
    pub fn partial_causes(&self, c: &mut dyn RunControl) -> Result<PartialCauses> {
        if self.complete() {
            return Ok(PartialCauses::default());
        }
        let mut causes = PartialCauses {
            decoding: u64::from(!self.coverage.decoding),
            control_flow: u64::from(!self.coverage.control_flow),
            references: u64::from(!self.coverage.references),
            ..PartialCauses::default()
        };
        for record in self.records {
            c.checkpoint(1)?;
            let FunctionRecord::SemanticGap { reason, .. } = record else {
                continue;
            };
            let cause = match reason {
                SemanticGapReason::UnsupportedInstruction | SemanticGapReason::WaitMemory => {
                    &mut causes.decoding
                }
                SemanticGapReason::UnexpandedControlFlow
                | SemanticGapReason::ConflictingBoundary => &mut causes.control_flow,
                SemanticGapReason::UnresolvedRelocation => &mut causes.references,
                SemanticGapReason::OpaqueCall => &mut causes.opaque_calls,
                SemanticGapReason::AlternativeLimit => &mut causes.value_limits,
                SemanticGapReason::ExpressionLimit => &mut causes.expression_limits,
            };
            *cause = 1;
        }
        if causes == PartialCauses::default() {
            causes.other = 1;
        }
        Ok(causes)
    }
}

/// One outcome of a library analysis. With summaries, analyzed functions
/// arrive in callee-first SCC order; otherwise in input, object and symbol order.
pub enum LibraryOutcome<'a> {
    Analyzed(AnalyzedFunction<'a>),
    /// A function that cannot be analyzed; its behavior is unknown.
    Blocked {
        function: &'a LibraryFunction,
        error: &'a Error,
    },
    /// Code no function is selected or analyzed from.
    Gap {
        input: u64,
        object: Option<&'a ObjectId>,
        reason: &'a str,
    },
}

type Visit<'v> = dyn FnMut(LibraryOutcome<'_>, &mut dyn RunControl) -> Result<()> + 'v;

/// Errors that block one function's analysis without failing the others.
fn blocks_function(error: &Error) -> bool {
    matches!(
        error.code,
        ErrorCode::NeedsExtent
            | ErrorCode::Incompatible
            | ErrorCode::InvalidRequest
            | ErrorCode::Unavailable
    )
}

/// Analyze every captured function. With call summaries, the first analysis
/// records the direct-call graph; the second emits final records in callee-first
/// SCC order and publishes each callee's result/argument contract once.
pub fn analyze_library(
    inputs: &[Executable],
    assumptions: Assumptions<'_>,
    decoder: &dyn FunctionSemantics,
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
    visit: &mut Visit<'_>,
) -> Result<AnalysisLimited> {
    let mut limited = AnalysisLimited::default();
    let mut counted_visit = |outcome: LibraryOutcome<'_>, c: &mut dyn RunControl| {
        if let LibraryOutcome::Analyzed(a) = &outcome {
            limited.function(&a.limits);
        }
        visit(outcome, c)
    };
    assumptions.validate()?;
    if assumptions.call_summaries {
        let catalog = crate::registers::Callees::capture(inputs, memory, control)?;
        let mut conflict_count = 0;
        let mut first_conflict = None;
        summaries::analyze(
            inputs,
            assumptions,
            &catalog,
            decoder,
            memory,
            control,
            &mut |outcome, c| {
                if let LibraryOutcome::Analyzed(a) = &outcome {
                    conflict_count += a.conflicts.len();
                    if first_conflict.is_none() {
                        first_conflict = a.conflicts.first().map(SignatureConflict::description);
                    }
                }
                counted_visit(outcome, c)
            },
        )?;
        if conflict_count != 0 {
            return Err(Error::new(
                ErrorCode::Conflict,
                format!(
                    "{conflict_count} signature conflicts; {}",
                    first_conflict.unwrap_or_default()
                ),
            ));
        }
        Ok(limited)
    } else {
        analyze_without_summaries(
            inputs,
            assumptions,
            decoder,
            memory,
            control,
            &mut counted_visit,
        )?;
        Ok(limited)
    }
}

fn analyze_without_summaries(
    inputs: &[Executable],
    assumptions: Assumptions<'_>,
    decoder: &dyn FunctionSemantics,
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
    visit: &mut Visit<'_>,
) -> Result<()> {
    for (index, executable) in inputs.iter().enumerate() {
        let input = index as u64;
        let mut position = RunPosition {
            input: Some(input),
            ..RunPosition::default()
        };
        position.artifact(executable.id());
        control.set_position(position);
        let container = visit_members(executable, memory, control, &mut |member, c| {
            analyze_object(
                input,
                member,
                assumptions.abi,
                assumptions.read_only,
                &NoSummaries,
                decoder,
                memory,
                c,
                visit,
            )
        })?;
        for reason in [
            container
                .framing
                .is_some()
                .then_some("archive member enumeration incomplete"),
            (container.kind == ContainerKind::Unsupported).then_some("unsupported input format"),
        ]
        .into_iter()
        .flatten()
        {
            visit(
                LibraryOutcome::Gap {
                    input,
                    object: None,
                    reason,
                },
                control,
            )?;
        }
    }
    Ok(())
}

/// Attach the physical function identity where the library owns it; the
/// shared index reports the conflicting class, site and original records.
fn function_facts<'a, 'm>(
    function: &LibraryFunction,
    records: &'a [FunctionRecord],
    memory: &'m WorkingMemory,
    control: &mut dyn RunControl,
) -> Result<Facts<'a, 'm>> {
    Facts::new(records, memory, control).map_err(|mut error| {
        if error.code == ErrorCode::Integrity {
            error.message = format!(
                "function {:?}, name {:?}: {}",
                function.symbol,
                function
                    .name
                    .as_ref()
                    .map(|name| name.escape_ascii().to_string()),
                error.message
            );
        }
        error
    })
}

/// The constants a complete function's `records` leave in `a0` and `a1` at
/// every return; none for a register any return leaves something else in,
/// and none at all when the function has no return or leaves other than by
/// a return.
fn returned_constants(
    records: &[FunctionRecord],
    calls: &dyn CallSummaries,
    links: &crate::registers::CallLinks<'_>,
    c: &mut dyn RunControl,
) -> Result<[Option<BTreeSet<u32>>; 2]> {
    let mut results: [Option<BTreeSet<u32>>; 2] = [Some(BTreeSet::new()), Some(BTreeSet::new())];
    let mut returns = 0;
    for record in records {
        c.checkpoint(1)?;
        match record {
            FunctionRecord::ReturnValue { low, high, .. } => {
                returns += 1;
                for (slot, value) in results.iter_mut().zip([low, high]) {
                    if let Some(set) = slot
                        && !constants(value, set)
                    {
                        *slot = None;
                    }
                }
            }
            // A tail call, or any other transfer out but a call, leaves
            // through the code it reaches: its results are not this
            // function's.
            FunctionRecord::Transfer {
                offset,
                call: false,
                ..
            }
            | FunctionRecord::Edge {
                from: offset,
                relation: EdgeKind::Jump,
                external: true,
                ..
            } => {
                returns += 1;
                let summary = links
                    .callee(*offset)
                    .and_then(|callee| calls.summary(callee));
                for (index, slot) in results.iter_mut().enumerate() {
                    if let Some(values) = summary
                        .and_then(|s| s.results.get(index))
                        .and_then(Option::as_deref)
                    {
                        if let Some(set) = slot {
                            set.extend(values.iter().copied());
                            if set.len() > MAX_VALUE_ALTERNATIVES {
                                *slot = None;
                            }
                        }
                    } else {
                        *slot = None;
                    }
                }
            }
            FunctionRecord::Edge {
                relation,
                external: true,
                ..
            } if !matches!(
                relation,
                EdgeKind::Call | EdgeKind::PossibleContinuation | EdgeKind::Return | EdgeKind::Jump
            ) =>
            {
                return Ok([None, None]);
            }
            _ => {}
        }
    }
    Ok(if returns == 0 { [None, None] } else { results })
}

/// Whether `value` is constants only, added to `set`.
fn constants(value: &AbstractValue, set: &mut BTreeSet<u32>) -> bool {
    match value {
        AbstractValue::Constant { value } => {
            set.insert(*value);
            set.len() <= MAX_VALUE_ALTERNATIVES
        }
        AbstractValue::Alternatives { values } => values.values().iter().all(|value| match value {
            ValueAlternative::Constant { value } => {
                set.insert(*value);
                set.len() <= MAX_VALUE_ALTERNATIVES
            }
            _ => false,
        }),
        _ => false,
    }
}
/// The explicit assumptions of a library analysis; none by default.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Assumptions<'a> {
    /// With it, a call the analysis cannot follow keeps the registers this
    /// calling convention preserves; without it, every register is unknown
    /// after such a call.
    pub abi: Option<oer_analysis_riscv_model::CallAbi>,
    /// Compute the direct-call graph without summaries, then analyze every
    /// function in callee-first order with shared result/argument contracts.
    /// Constants propagate through any acyclic call depth in two analyses.
    pub call_summaries: bool,
    pub signatures: Option<&'a crate::signatures::Signatures>,
    /// Names whose call-result words become read-use sources. Empty keeps load-only output.
    pub call_results: &'a [String],
    /// Address ranges read-only at run time whatever a linked image's
    /// section flags say, such as flash mapped for reading: constants are
    /// read from the file-backed bytes inside them.
    pub read_only: &'a [ReadOnlyWindow],
}

impl Assumptions<'_> {
    fn validate(self) -> Result<()> {
        if self.signatures.is_some() && !self.call_summaries {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "signatures require call summaries",
            ));
        }
        Ok(())
    }
}

/// The sections and function symbols of one object.
#[derive(Default)]
struct Selection {
    /// Indices of nonempty executable sections, sorted.
    executable: Vec<u32>,
    functions: Vec<SymbolRecord>,
    diagnostics: Vec<Diagnostic>,
}
impl ElfSink for Selection {
    fn section(&mut self, r: &SectionRecord, c: &mut dyn RunControl) -> Result<()> {
        if elf::SectionFlags(r.flags).executable() && r.size != 0 {
            match self.executable.binary_search(&r.index) {
                Ok(_) => return Err(Error::new(ErrorCode::Integrity, "duplicate section index")),
                Err(i) => self.executable.insert(i, r.index),
            }
        }
        c.checkpoint(1)
    }
    fn symbol(&mut self, r: &SymbolRecord, _: &mut dyn RunControl) -> Result<()> {
        let section = elf::symbol_section(r.raw_section, r.extended_section).index();
        if SymbolKind::of_type(r.symbol_type) == SymbolKind::Text
            && section.is_some_and(|s| self.executable.binary_search(&s).is_ok())
        {
            self.functions.push(r.clone());
        }
        Ok(())
    }
    fn relocation(&mut self, _: &RelocationRecord, _: &mut dyn RunControl) -> Result<()> {
        Ok(())
    }
    fn diagnostic(&mut self, r: &Diagnostic, _: &mut dyn RunControl) -> Result<()> {
        self.diagnostics.push(r.clone());
        Ok(())
    }
}

/// Research the function `symbol` defines in the prepared `object` into
/// `sink`. `references` keeps each section's prepared relocations for the
/// object's other functions.
#[allow(clippy::too_many_arguments)]
fn research_function<'m>(
    object: &mut oer_analysis_blobray_artifacts::PreparedObject<'_, '_>,
    references: &mut AdmittedVec<'m, (u32, oer_analysis_riscv_analysis::PreparedReferences<'m>)>,
    symbol: &SymbolId,
    input: u64,
    payload: &ArtifactId,
    abi: Option<oer_analysis_riscv_model::CallAbi>,
    returns: &dyn CallSummaries,
    tables: &dyn TableEntries,
    decoder: &dyn FunctionSemantics,
    memory: &'m WorkingMemory,
    control: &mut dyn RunControl,
    sink: &mut dyn FunctionSink,
) -> Result<(
    oer_analysis_riscv_analysis::AnalysisSummary,
    oer_analysis_riscv_analysis::ProvenTables<'m, JumpDispatch>,
)> {
    let mut position = RunPosition {
        phase: RunPhase::AnalyzeFunction,
        input: Some(input),
        member: match symbol.object.location {
            ObjectLocation::Standalone => None,
            ObjectLocation::ArchiveMember { ordinal } => Some(ordinal),
        },
        ..Default::default()
    };
    position.artifact(payload);
    control.set_position(position);
    control.checkpoint(0)?;
    object.with_function(symbol, control, |view, control| {
        let index = if let Some(index) = references
            .iter()
            .position(|(section, _)| *section == view.section)
        {
            index
        } else {
            let prepared = oer_analysis_riscv_analysis::PreparedReferences::new(
                view.relocations,
                view.section,
                decoder,
                memory,
                control,
            )?;
            references.push((view.section, prepared), control.position())?;
            references.len() - 1
        };
        oer_analysis_riscv_analysis::research_following_tables(
            oer_analysis_riscv_analysis::FunctionInput {
                traps: oer_analysis_riscv_analysis::TrapSemantics::Unknown,
                image: view.image,
                section: view.section,
                extent: view.extent,
                bytes: view.code,
                relocations: &references[index].1,
                data_ranges: view.data_ranges,
                jumps: &[],
                returns,
                call_behavior: &UnknownCallBehavior,
            },
            &oer_analysis_riscv_analysis::RecognizedTables {
                entries: tables,
                section: view.section,
            },
            decoder,
            memory,
            control,
            sink,
            abi,
        )
    })
}

/// Collects one function's records into admitted memory.
struct Records<'a, 'm>(&'a mut RecordBuffer<'m>);
impl FunctionSink for Records<'_, '_> {
    fn record(&mut self, record: &FunctionRecord, control: &mut dyn RunControl) -> Result<()> {
        control.checkpoint(1)?;
        self.0.push(record.clone(), control.position())
    }
}

#[allow(clippy::too_many_arguments)]
fn analyze_object(
    input: u64,
    member: Member<'_>,
    abi: Option<oer_analysis_riscv_model::CallAbi>,
    read_only: &[ReadOnlyWindow],
    returns: &dyn CallSummaries,
    decoder: &dyn FunctionSemantics,
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
    visit: &mut Visit<'_>,
) -> Result<()> {
    let object = member.id;
    let mut selection = Selection::default();
    let (bytes, payload, elf) = match member.bytes {
        Ok(bytes) => {
            let (payload, elf) = inspect_source(bytes, &object, memory, control, &mut selection)?;
            (Some(bytes), Some(payload), elf)
        }
        Err(diagnostic) => {
            selection.diagnostics.push(diagnostic);
            (None, None, None)
        }
    };
    let mut gap = |reason: &str, c: &mut dyn RunControl| {
        visit(
            LibraryOutcome::Gap {
                input,
                object: Some(&object),
                reason,
            },
            c,
        )
    };
    if payload.is_none() {
        gap("object payload unavailable", control)?;
    }
    if !elf.as_ref().is_some_and(|e| {
        elf::is_riscv32_little_endian(e.bits, e.little_endian, e.machine)
            && matches!(
                elf::object_kind(e.object_type),
                elf::ObjectKind::Relocatable | elf::ObjectKind::Executable
            )
    }) {
        gap(
            "object is not a supported ELF32 little-endian RISC-V relocatable or executable",
            control,
        )?;
    }
    for d in &selection.diagnostics {
        gap(
            &format!("{:?}: {}: {}", d.code, d.context, d.message),
            control,
        )?;
    }
    if selection.functions.is_empty()
        && (!selection.executable.is_empty()
            || elf
                .as_ref()
                .is_some_and(|e| elf::object_kind(e.object_type) == elf::ObjectKind::Executable))
    {
        gap(
            "executable sections have no selected function symbols",
            control,
        )?;
    }
    let object_type = elf.as_ref().map(|e| elf::object_kind(e.object_type));
    let (Some(bytes), Some(payload)) = (bytes, payload) else {
        return Ok(());
    };
    // A relocatable object's relocations prove its jump tables; a static
    // image's facts (the words an `--emit-relocs` link kept, its `.LJTI*`
    // labels and sized read-only objects) prove its own.
    let functions: Vec<LibraryFunction> = selection
        .functions
        .into_iter()
        .map(|r| LibraryFunction {
            input,
            symbol: r.id,
            name: r.name,
        })
        .collect();
    if functions.is_empty() {
        return Ok(());
    }
    let mut entered = false;
    let result = oer_analysis_blobray_artifacts::with_prepared_object(
        bytes,
        &payload,
        read_only,
        memory,
        control,
        |prepared, c| {
            entered = true;
            let bytes = prepared.bytes();
            let mut unreadable = |error: Error, c: &mut dyn RunControl| {
                if error.code != ErrorCode::Integrity {
                    return Err(error);
                }
                visit(
                    LibraryOutcome::Gap {
                        input,
                        object: Some(&object),
                        reason: &format!("jump-table facts unreadable: {}", error.message),
                    },
                    c,
                )
            };
            let mut object_tables = None;
            let mut image_facts = None;
            match object_type {
                Some(elf::ObjectKind::Relocatable) => {
                    match oer_analysis_riscv_program::tables::ObjectTables::new(bytes, memory, c) {
                        Ok(tables) => object_tables = Some(tables),
                        Err(error) => unreadable(error, c)?,
                    }
                }
                Some(elf::ObjectKind::Executable) => {
                    match oer_analysis_riscv_program::facts::ImageFacts::new(
                        bytes, read_only, memory, c,
                    ) {
                        Ok(facts) => image_facts = Some(facts),
                        Err(error) => unreadable(error, c)?,
                    }
                }
                _ => {}
            }
            let image_tables = image_facts
                .as_ref()
                .map(|image| oer_analysis_riscv_program::tables::ImageTables { image });
            let tables: &dyn TableEntries = match (&object_tables, &image_tables) {
                (Some(tables), _) => tables,
                (None, Some(tables)) => tables,
                (None, None) => &NoTables,
            };
            let mut references = AdmittedVec::new(memory);
            for function in &functions {
                let mut records = RecordBuffer::new(memory);
                let mut sink = Records(&mut records);
                let researched = research_function(
                    prepared,
                    &mut references,
                    &function.symbol,
                    input,
                    &payload,
                    abi,
                    returns,
                    tables,
                    decoder,
                    memory,
                    c,
                    &mut sink,
                );
                match researched {
                    Ok((summary, found)) => visit(
                        LibraryOutcome::Analyzed(AnalyzedFunction {
                            limits: FunctionLimits::value_events(summary.limit_events),
                            function,
                            records: &records,
                            coverage: summary.coverage,
                            semantics: summary.semantics,
                            jump_tables: &found.tables,
                            summaries: returns,
                            conflicts: &[],
                            rom_signature_diagnostics: &[],
                            summary_outcome: None,
                            rom_signature_summary_outcome: None,
                            signature_audit_outcome: None,
                        }),
                        c,
                    )?,
                    Err(error) if blocks_function(&error) => visit(
                        LibraryOutcome::Blocked {
                            function,
                            error: &error,
                        },
                        c,
                    )?,
                    Err(error) => return Err(error),
                }
            }
            Ok(())
        },
    );
    match result {
        Err(error) if !entered && blocks_function(&error) => {
            for function in &functions {
                visit(
                    LibraryOutcome::Blocked {
                        function,
                        error: &error,
                    },
                    control,
                )?;
            }
            Ok(())
        }
        other => other,
    }
}

fn summary_failure(
    function: &LibraryFunction,
    outcome: SummaryOutcome<()>,
    unavailable: &str,
    audit: bool,
) -> Option<SignatureAuditIncomplete> {
    let reason = match outcome {
        SummaryOutcome::Available(()) => return None,
        SummaryOutcome::LocalBudgetExceeded(budget) if audit => Error::new(
            ErrorCode::LocalBudgetExceeded(budget),
            "read-use function work budget exhausted",
        ),
        _ => Error::new(ErrorCode::Unavailable, unavailable),
    };
    Some(SignatureAuditIncomplete {
        function: function.clone(),
        reason,
    })
}

pub fn register_limit_reason(record: &RegisterAccess) -> Option<LimitReason> {
    let reads = match record {
        RegisterAccess::Observation { reads, .. } => reads.as_ref(),
        RegisterAccess::CallResult { reads, .. } => Some(reads),
        _ => None,
    };
    if reads.is_some_and(|r| {
        r.unknown
            .iter()
            .any(|r| r.limit == ReadLimitKind::WorkLimit)
    }) {
        Some(LimitReason::LocalBudget(LocalBudgetScope::ReadFlow))
    } else {
        None
    }
}

/// Every candidate memory address the functions `inputs` define access,
/// inside `ranges` when it is nonempty, with blocked functions and gaps.
pub fn register_accesses(
    inputs: &[Executable],
    assumptions: Assumptions<'_>,
    ranges: &[ImageRegion],
    decoder: &dyn FunctionSemantics,
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
    emit: &mut dyn FnMut(&RegisterAccess, &mut dyn RunControl) -> Result<()>,
) -> Result<(RegisterAccessSummary, AnalysisLimited)> {
    crate::registers::validate_ranges(ranges)?;
    let callees = crate::registers::Callees::capture(inputs, memory, control)?;
    let mut limited = AnalysisLimited::default();
    let mut summary = RegisterAccessSummary {
        schema: 4,
        flow_local_budget_functions: 0,
        summary_outcomes: SummaryOutcomeCounts::default(),
        rom_signature_summary_unavailable_reasons: SummaryUnavailableCounts::default(),
        signature_audit_incomplete_reasons: SummaryUnavailableCounts::default(),
        contract_narrowed_bits: 0,
        contract_may_narrowed_bits: 0,
        signature_conflicts: Vec::new(),
        rom_signature_diagnostics: Vec::new(),
        rom_signature_diagnostic_count: 0,
        rom_signature_summary_unavailable: Vec::new(),
        signature_audit_incomplete: Vec::new(),
        ranges: ranges.to_vec(),
        functions: 0,
        partial_functions: 0,
        partial_causes: PartialCauses::default(),
        blocked_functions: 0,
        gaps: 0,
        observations: 0,
        unresolved_addresses: 0,
        indexed_addresses: 0,
    };
    let mut conflict_memory = Ledger::new(memory, control)?;
    let mut visit = |outcome: LibraryOutcome<'_>, c: &mut dyn RunControl| match outcome {
        LibraryOutcome::Analyzed(analyzed) => {
            let mut function_limits = analyzed.limits.clone();
            if let Some(outcome) = analyzed.summary_outcome {
                summary.summary_outcomes.record(outcome);
            }
            for (outcome, counts) in [
                (
                    analyzed.rom_signature_summary_outcome,
                    &mut summary.rom_signature_summary_unavailable_reasons,
                ),
                (
                    analyzed.signature_audit_outcome,
                    &mut summary.signature_audit_incomplete_reasons,
                ),
            ] {
                if let Some(outcome) = outcome {
                    counts.record(outcome);
                }
            }
            for (outcome, failures, reason, audit) in [
                (
                    analyzed.signature_audit_outcome,
                    &mut summary.signature_audit_incomplete,
                    "signature audit did not finish within its per-function budget",
                    true,
                ),
                (
                    analyzed.rom_signature_summary_outcome,
                    &mut summary.rom_signature_summary_unavailable,
                    "computed ROM summary unavailable within the original per-function budget",
                    false,
                ),
            ] {
                let Some(failure) = outcome
                    .and_then(|outcome| summary_failure(analyzed.function, outcome, reason, audit))
                else {
                    continue;
                };
                if let Some(SummaryOutcome::LocalBudgetExceeded(budget)) = outcome {
                    function_limits.result(&[LimitReason::LocalBudget(budget.scope)]);
                }
                conflict_memory.admit(
                    serde_json::to_vec(&failure)
                        .map_err(|e| Error::new(ErrorCode::Integrity, e.to_string()))?
                        .len(),
                    c,
                )?;
                conflict_memory.push(failures, failure, c)?;
            }
            for conflict in analyzed.conflicts {
                conflict_memory.admit(
                    serde_json::to_vec(conflict)
                        .map_err(|e| Error::new(ErrorCode::Integrity, e.to_string()))?
                        .len(),
                    c,
                )?;
                conflict_memory.push(&mut summary.signature_conflicts, conflict.clone(), c)?;
            }
            for diagnostic in analyzed.rom_signature_diagnostics {
                conflict_memory.admit(
                    serde_json::to_vec(diagnostic)
                        .map_err(|e| Error::new(ErrorCode::Integrity, e.to_string()))?
                        .len(),
                    c,
                )?;
                conflict_memory.push(
                    &mut summary.rom_signature_diagnostics,
                    diagnostic.clone(),
                    c,
                )?;
                summary.rom_signature_diagnostic_count += 1;
            }
            summary.functions += 1;
            summary.partial_functions += u64::from(!analyzed.complete());
            summary.partial_causes.add(analyzed.partial_causes(c)?);
            c.phase(RunPhase::AnalyzeValues)?;
            let facts = function_facts(analyzed.function, analyzed.records, memory, c)?;
            let mut reads = crate::registers::ReadObservations::new(
                &facts,
                memory,
                decoder,
                ranges,
                assumptions.abi,
                &callees,
                analyzed.function,
                analyzed.summaries,
                assumptions.call_results,
            );
            crate::registers::observe(analyzed.records, &facts, memory, ranges, c, &mut |o, c| {
                let stored = reads.stored(&o, c)?;
                let mask = o.mask.or_else(|| {
                    let FunctionRecord::MemoryAccess { width, .. } = o.fact else {
                        return None;
                    };
                    stored.as_deref().and_then(|stored| {
                        oer_analysis_blobray_analysis::registers::write_mask(*width, stored)
                    })
                });
                summary.observations += 1;
                summary.unresolved_addresses += u64::from(o.address.is_none());
                summary.indexed_addresses += u64::from(o.indexed.is_some());
                let observation = RegisterAccess::Observation {
                    function: analyzed.function.clone(),
                    record: o.record,
                    fact: Box::new(o.fact.clone()),
                    address: o.address,
                    alternative: o.alternative,
                    indexed: o.indexed,
                    mask,
                    stored,
                    reads: reads.reads(o.fact, c)?,
                };
                if let Some(reason) = register_limit_reason(&observation) {
                    function_limits.result(&[reason]);
                }
                emit(&observation, c)
            })?;
            reads.call_results(c, &mut |observation, c| {
                summary.observations += 1;
                if let Some(reason) = register_limit_reason(observation) {
                    function_limits.result(&[reason]);
                }
                emit(observation, c)
            })?;
            let [read_limit, stored_limit] = reads.local_budget_exhaustion();
            summary.flow_local_budget_functions += u64::from(read_limit || stored_limit);
            for exhausted in [read_limit, stored_limit] {
                if exhausted {
                    function_limits.event(LimitReason::LocalBudget(LocalBudgetScope::ReadFlow));
                }
            }
            limited.function(&function_limits);
            let [direct, may] = reads.contract_narrowed_bits();
            summary.contract_narrowed_bits += direct;
            summary.contract_may_narrowed_bits += may;
            Ok(())
        }
        LibraryOutcome::Blocked { function, error } => {
            summary.blocked_functions += 1;
            emit(
                &RegisterAccess::Blocked {
                    function: function.clone(),
                    error: error.clone(),
                },
                c,
            )
        }
        LibraryOutcome::Gap {
            input,
            object,
            reason,
        } => {
            summary.gaps += 1;
            emit(
                &RegisterAccess::Gap {
                    input,
                    object: object.cloned(),
                    reason: reason.to_owned(),
                },
                c,
            )
        }
    };
    assumptions.validate()?;
    if assumptions.call_summaries {
        summaries::analyze(
            inputs,
            assumptions,
            &callees,
            decoder,
            memory,
            control,
            &mut visit,
        )?;
    } else {
        analyze_without_summaries(inputs, assumptions, decoder, memory, control, &mut visit)?;
    }
    Ok((summary, limited))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn function() -> LibraryFunction {
        LibraryFunction {
            input: 0,
            symbol: SymbolId {
                object: ObjectId {
                    artifact: ArtifactId::of_bytes(b"object"),
                    location: ObjectLocation::Standalone,
                },
                table: SymbolTableKind::Static,
                table_section: 1,
                index: 1,
            },
            name: None,
        }
    }

    #[test]
    fn expression_limit_counts_round_trip_and_zero_keeps_existing_reports() {
        let mut causes = PartialCauses::default();
        let old = serde_json::to_value(causes).unwrap();
        assert!(old.get("expression_limits").is_none());
        assert_eq!(
            serde_json::from_value::<PartialCauses>(old).unwrap(),
            causes
        );
        causes.add(PartialCauses {
            expression_limits: 2,
            ..Default::default()
        });
        let encoded = serde_json::to_value(causes).unwrap();
        assert_eq!(encoded["expression_limits"], 2);
        assert_eq!(
            serde_json::from_value::<PartialCauses>(encoded).unwrap(),
            causes
        );
        assert_eq!(
            serde_json::to_value(SemanticGapReason::ExpressionLimit).unwrap(),
            "expression-limit"
        );
    }

    #[test]
    fn navigation_integrity_errors_name_the_physical_function_and_records() {
        let mut function = function();
        function.name = Some(b"mixed_targets".to_vec());
        let records = [1, 2].map(|value| FunctionRecord::CallInputs {
            offset: 0x40,
            registers: vec![AbstractValue::Constant { value }],
        });
        let memory = WorkingMemory::new(8192).expect("navigation fixture");
        let error = match function_facts(&function, &records, &memory, &mut || Ok(())) {
            Err(error) => error,
            Ok(_) => panic!("duplicate inputs must fail"),
        };
        assert_eq!(error.code, ErrorCode::Integrity);
        for expected in [
            format!("function {:?}", function.symbol),
            "mixed_targets".into(),
            "class=call-inputs offset=0x40".into(),
            format!("first record[0]={:?}", records[0]),
            format!("second record[1]={:?}", records[1]),
        ] {
            assert!(error.message.contains(&expected), "{error}");
        }
        assert_eq!(memory.used(), 0);
    }

    fn causes(
        coverage: FunctionCoverage,
        complete: bool,
        records: &[FunctionRecord],
        c: &mut dyn RunControl,
    ) -> Result<PartialCauses> {
        let function = function();
        AnalyzedFunction {
            limits: FunctionLimits::default(),
            function: &function,
            records,
            coverage,
            semantics: SemanticSummary {
                complete,
                ..SemanticSummary::default()
            },
            jump_tables: &[],
            summaries: &NoSummaries,
            conflicts: &[],
            rom_signature_diagnostics: &[],
            summary_outcome: None,
            rom_signature_summary_outcome: None,
            signature_audit_outcome: None,
        }
        .partial_causes(c)
    }

    #[test]
    fn incomplete_audit_record_preserves_the_local_budget_reason() {
        let function = function();
        let budget = LocalBudgetExceeded {
            limit: 123_456,
            scope: LocalBudgetScope::SignatureAudit,
        };
        let failure = summary_failure(
            &function,
            SummaryOutcome::LocalBudgetExceeded(budget),
            "audit unavailable",
            true,
        )
        .expect("incomplete audit record");
        let encoded = serde_json::to_vec(&failure).expect("serialize report record");
        let recorded: SignatureAuditIncomplete =
            serde_json::from_slice(&encoded).expect("deserialize report record");
        assert_eq!(recorded.function, function);
        assert_eq!(
            recorded.reason,
            Error::new(
                ErrorCode::LocalBudgetExceeded(budget),
                "read-use function work budget exhausted",
            )
        );
    }

    #[test]
    fn each_semantic_gap_reason_names_its_cause_and_none_is_other() {
        let gap = |reason| FunctionRecord::SemanticGap { offset: 0, reason };
        let full = FunctionCoverage::default();
        assert_eq!(
            causes(full, true, &[], &mut || Ok(())).unwrap(),
            PartialCauses::default(),
            "a complete function has no cause"
        );
        assert_eq!(
            causes(
                full,
                false,
                &[
                    gap(SemanticGapReason::AlternativeLimit),
                    gap(SemanticGapReason::AlternativeLimit),
                    gap(SemanticGapReason::ExpressionLimit),
                    gap(SemanticGapReason::ExpressionLimit),
                    gap(SemanticGapReason::UnsupportedInstruction),
                ],
                &mut || Ok(())
            )
            .unwrap(),
            PartialCauses {
                value_limits: 1,
                expression_limits: 1,
                decoding: 1,
                ..PartialCauses::default()
            },
            "two gaps of one reason count the function once"
        );
        assert_eq!(
            causes(full, false, &[], &mut || Ok(())).unwrap(),
            PartialCauses {
                other: 1,
                ..PartialCauses::default()
            },
            "a partial function no named cause explains is never uncounted"
        );
        let exhausted = causes(
            full,
            false,
            &[gap(SemanticGapReason::OpaqueCall)],
            &mut || Err(Error::new(ErrorCode::ResourceLimited, "work budget")),
        )
        .err()
        .unwrap();
        assert_eq!(exhausted.code, ErrorCode::ResourceLimited);
    }
}
