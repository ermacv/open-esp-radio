//! Every function captured libraries define, analyzed in process.
use crate::captured::{Member, visit_members};
use crate::in_process::Executable;
use crate::*;
use oer_analysis_blobray_analysis::navigation::Facts;
use std::collections::{BTreeMap, BTreeSet};

/// One analyzed function with its records, borrowed for a visit.
pub struct AnalyzedFunction<'a> {
    pub function: &'a LibraryFunction,
    pub records: &'a [FunctionRecord],
    pub coverage: FunctionCoverage,
    pub semantics: SemanticSummary,
    /// Ordered tables or complete target sets the analysis followed.
    pub jump_tables: &'a [JumpDispatch],
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
            };
            *cause = 1;
        }
        if causes == PartialCauses::default() {
            causes.other = 1;
        }
        Ok(causes)
    }
}

/// One outcome of a library analysis, in input, object and symbol order.
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

/// Analyze every function symbol of an executable section of every object
/// `inputs` contain under `assumptions`, presenting each outcome to
/// `visit`. With [`Assumptions::call_returns`], a first pass collects the
/// constants each function returns and the analysis takes them at calls one
/// level deep.
pub fn analyze_library(
    inputs: &[Executable],
    assumptions: Assumptions<'_>,
    decoder: &dyn FunctionSemantics,
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
    visit: &mut Visit<'_>,
) -> Result<()> {
    let Assumptions {
        abi,
        call_returns,
        read_only,
    } = assumptions;
    let mut ledger = Ledger::new(memory, control)?;
    let mut table = ReturnTable::default();
    for (index, executable) in inputs.iter().enumerate().filter(|_| call_returns) {
        let input = index as u64;
        let mut position = RunPosition {
            input: Some(input),
            ..RunPosition::default()
        };
        position.artifact(executable.id());
        control.set_position(position);
        let mut collect = Collect {
            table: &mut table,
            ledger: &mut ledger,
        };
        visit_members(executable, memory, control, &mut |member, c| {
            analyze_object(
                input,
                member,
                abi,
                read_only,
                &NoReturns,
                Some(&mut collect),
                decoder,
                memory,
                c,
                &mut |_, _| Ok(()),
            )
        })?;
    }
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
                input, member, abi, read_only, &table, None, decoder, memory, c, visit,
            )
        })?;
        let gap = |reason: &str, visit: &mut Visit<'_>, c: &mut dyn RunControl| {
            visit(
                LibraryOutcome::Gap {
                    input,
                    object: None,
                    reason,
                },
                c,
            )
        };
        if container.framing.is_some() {
            gap("archive member enumeration incomplete", visit, control)?;
        }
        if container.kind == ContainerKind::Unsupported {
            gap("unsupported input format", visit, control)?;
        }
    }
    Ok(())
}

/// How a call names a function: its address in a linked image, or its
/// global symbol in a relocatable object.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
enum CalleeKey {
    Address(u32),
    Symbol(Vec<u8>),
}

/// The constants each function of the inputs leaves in `a0` and `a1` at
/// every return, where its analysis is complete, no exit leaves it but a
/// return, and every return's value is a known constant; by how a call names
/// the function. A name defined with different results answers nothing.
#[derive(Default)]
pub struct ReturnTable {
    addresses: BTreeMap<u32, [Option<Vec<u32>>; 2]>,
    symbols: BTreeMap<Vec<u8>, [Option<Vec<u32>>; 2]>,
}

impl CallReturns for ReturnTable {
    fn returns(&self, callee: Callee<'_>, register: u8) -> Option<&[u32]> {
        let results = match callee {
            Callee::Address(address) => self.addresses.get(&address)?,
            Callee::Symbol(name) => self.symbols.get(name)?,
        };
        let slot = CallAbi::RiscvInteger
            .results()
            .iter()
            .position(|&r| r == register)?;
        results[slot].as_deref()
    }
}

/// The first pass's table and the ledger that admits it.
pub(crate) struct Collect<'t, 'm> {
    table: &'t mut ReturnTable,
    ledger: &'t mut Ledger<'m>,
}

impl Collect<'_, '_> {
    /// Add the results of the function `key` names from its complete
    /// analysis's `records`, or, without them (the function is blocked or
    /// its analysis incomplete), record that it gives no results: another
    /// definition of the same name then answers nothing either.
    fn add(
        &mut self,
        key: &CalleeKey,
        records: Option<&[FunctionRecord]>,
        c: &mut dyn RunControl,
    ) -> Result<()> {
        let results = match records {
            Some(records) => returned_constants(records, c)?,
            None => [None, None],
        };
        let mut admitted: [Option<Vec<u32>>; 2] = [None, None];
        for (slot, set) in admitted.iter_mut().zip(results) {
            if let Some(set) =
                set.filter(|set| !set.is_empty() && set.len() <= MAX_VALUE_ALTERNATIVES)
            {
                let mut values = Vec::new();
                for value in set {
                    self.ledger.push(&mut values, value, c)?;
                }
                *slot = Some(values);
            }
        }
        match key {
            CalleeKey::Address(address) => insert(
                &mut self.table.addresses,
                *address,
                admitted,
                self.ledger,
                c,
            ),
            CalleeKey::Symbol(name) => {
                self.ledger.admit(name.len(), c)?;
                insert(
                    &mut self.table.symbols,
                    name.clone(),
                    admitted,
                    self.ledger,
                    c,
                )
            }
        }
    }
}

/// The constants a complete function's `records` leave in `a0` and `a1` at
/// every return; none for a register any return leaves something else in,
/// and none at all when the function has no return or leaves other than by
/// a return.
fn returned_constants(
    records: &[FunctionRecord],
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
            FunctionRecord::Transfer { call: false, .. } => return Ok([None, None]),
            FunctionRecord::Edge {
                relation,
                external: true,
                ..
            } if !matches!(
                relation,
                EdgeKind::Call | EdgeKind::PossibleContinuation | EdgeKind::Return
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
            true
        }
        AbstractValue::Alternatives { values } => values.values().iter().all(|value| match value {
            ValueAlternative::Constant { value } => {
                set.insert(*value);
                true
            }
            _ => false,
        }),
        _ => false,
    }
}

/// Record `results` under `key`; a key seen with other results keeps none.
fn insert<K: Ord>(
    map: &mut BTreeMap<K, [Option<Vec<u32>>; 2]>,
    key: K,
    results: [Option<Vec<u32>>; 2],
    ledger: &mut Ledger<'_>,
    c: &mut dyn RunControl,
) -> Result<()> {
    match map.get_mut(&key) {
        Some(known) => {
            for (known, new) in known.iter_mut().zip(results) {
                if *known != new {
                    *known = None;
                }
            }
        }
        None => {
            ledger.insert(map, key, results, c)?;
        }
    }
    Ok(())
}

/// The explicit assumptions of a library analysis; none by default.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Assumptions<'a> {
    /// With it, a call the analysis cannot follow keeps the registers this
    /// calling convention preserves; without it, every register is unknown
    /// after such a call.
    pub abi: Option<oer_analysis_riscv_model::CallAbi>,
    /// A call to a function whose every return leaves only known constants
    /// in a result register takes them for that register. A first pass
    /// analyzes every function to collect those constants
    /// ([`ReturnTable`]), so the analysis takes about twice as long.
    pub call_returns: bool,
    /// Address ranges read-only at run time whatever a linked image's
    /// section flags say, such as flash mapped for reading: constants are
    /// read from the file-backed bytes inside them.
    pub read_only: &'a [ReadOnlyWindow],
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
        if r.flags & 4 != 0 && r.size != 0 {
            match self.executable.binary_search(&r.index) {
                Ok(_) => return Err(Error::new(ErrorCode::Integrity, "duplicate section index")),
                Err(i) => self.executable.insert(i, r.index),
            }
        }
        c.checkpoint(1)
    }
    fn symbol(&mut self, r: &SymbolRecord, _: &mut dyn RunControl) -> Result<()> {
        // STT_FUNC in a section already known to be executable.
        let section = if r.raw_section == 0xffff {
            r.extended_section
        } else if r.raw_section > 0 && r.raw_section < 0xff00 {
            Some(u32::from(r.raw_section))
        } else {
            None
        };
        if r.symbol_type == 2 && section.is_some_and(|s| self.executable.binary_search(&s).is_ok())
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
    returns: &dyn CallReturns,
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
    returns: &dyn CallReturns,
    mut collect: Option<&mut Collect<'_, '_>>,
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
        e.bits == 32 && e.little_endian && e.machine == 243 && matches!(e.object_type, 1 | 2)
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
        && (!selection.executable.is_empty() || elf.as_ref().is_some_and(|e| e.object_type == 2))
    {
        gap(
            "executable sections have no selected function symbols",
            control,
        )?;
    }
    let object_type = elf.as_ref().map(|e| e.object_type);
    // How a call names each function: its address in an image, or its
    // global symbol in a relocatable object.
    let keys: Vec<Option<CalleeKey>> = selection
        .functions
        .iter()
        .map(|r| match object_type {
            Some(2) => u32::try_from(r.value)
                .ok()
                .map(|at| CalleeKey::Address(at & !1)),
            Some(1) => r
                .name
                .clone()
                .filter(|name| r.binding == 1 && !name.is_empty())
                .map(CalleeKey::Symbol),
            _ => None,
        })
        .collect();
    // The first pass records that a function no analysis reads gives no
    // results.
    let reject = |collect: &mut Option<&mut Collect<'_, '_>>, c: &mut dyn RunControl| {
        if let Some(collect) = collect.as_deref_mut() {
            for key in keys.iter().flatten() {
                collect.add(key, None, c)?;
            }
        }
        Ok::<_, Error>(())
    };
    let (Some(bytes), Some(payload)) = (bytes, payload) else {
        return reject(&mut collect, control);
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
                Some(1) => {
                    match oer_analysis_riscv_program::tables::ObjectTables::new(bytes, memory, c) {
                        Ok(tables) => object_tables = Some(tables),
                        Err(error) => unreadable(error, c)?,
                    }
                }
                Some(2) => {
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
            for (function, key) in functions.iter().zip(&keys) {
                let mut records = RecordBuffer::new(memory);
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
                    &mut Records(&mut records),
                );
                match researched {
                    Ok((summary, _)) if collect.is_some() => {
                        if let (Some(collect), Some(key)) = (collect.as_deref_mut(), key) {
                            let complete =
                                summary.coverage.complete() && summary.semantics.complete;
                            collect.add(key, complete.then_some(&records[..]), c)?;
                        }
                    }
                    Ok((summary, found)) => visit(
                        LibraryOutcome::Analyzed(AnalyzedFunction {
                            function,
                            records: &records,
                            coverage: summary.coverage,
                            semantics: summary.semantics,
                            jump_tables: &found.tables,
                        }),
                        c,
                    )?,
                    Err(error) if blocks_function(&error) => {
                        if let (Some(collect), Some(key)) = (collect.as_deref_mut(), key) {
                            collect.add(key, None, c)?;
                        }
                        visit(
                            LibraryOutcome::Blocked {
                                function,
                                error: &error,
                            },
                            c,
                        )?
                    }
                    Err(error) => return Err(error),
                }
            }
            Ok(())
        },
    );
    match result {
        Err(error) if !entered && blocks_function(&error) => {
            reject(&mut collect, control)?;
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
) -> Result<RegisterAccessSummary> {
    crate::registers::validate_ranges(ranges)?;
    let mut summary = RegisterAccessSummary {
        schema: 3,
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
    analyze_library(
        inputs,
        assumptions,
        decoder,
        memory,
        control,
        &mut |outcome, c| match outcome {
            LibraryOutcome::Analyzed(analyzed) => {
                summary.functions += 1;
                summary.partial_functions += u64::from(!analyzed.complete());
                summary.partial_causes.add(analyzed.partial_causes(c)?);
                c.phase(RunPhase::AnalyzeValues)?;
                let facts = Facts::new(analyzed.records, memory, c)?;
                let mut reads = crate::registers::ReadObservations::new(
                    &facts,
                    memory,
                    decoder,
                    ranges,
                    assumptions.abi,
                );
                crate::registers::observe(
                    analyzed.records,
                    &facts,
                    memory,
                    ranges,
                    c,
                    &mut |o, c| {
                        summary.observations += 1;
                        summary.unresolved_addresses += u64::from(o.address.is_none());
                        summary.indexed_addresses += u64::from(o.indexed.is_some());
                        emit(
                            &RegisterAccess::Observation {
                                function: analyzed.function.clone(),
                                record: o.record,
                                fact: Box::new(o.fact.clone()),
                                address: o.address,
                                alternative: o.alternative,
                                indexed: o.indexed,
                                mask: o.mask,
                                stored: o.stored,
                                reads: reads.reads(o.fact, c)?,
                            },
                            c,
                        )
                    },
                )
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
        },
    )?;
    Ok(summary)
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

    fn causes(
        coverage: FunctionCoverage,
        complete: bool,
        records: &[FunctionRecord],
        c: &mut dyn RunControl,
    ) -> Result<PartialCauses> {
        let function = function();
        AnalyzedFunction {
            function: &function,
            records,
            coverage,
            semantics: SemanticSummary {
                complete,
                ..SemanticSummary::default()
            },
            jump_tables: &[],
        }
        .partial_causes(c)
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
                    gap(SemanticGapReason::UnsupportedInstruction),
                ],
                &mut || Ok(())
            )
            .unwrap(),
            PartialCauses {
                value_limits: 1,
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
