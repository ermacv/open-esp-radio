//! One research driver for indirect jumps through tables, shared by every
//! consumer of the value analysis.
//!
//! A first pass over a function locates its dispatches; a
//! [`JumpTableSource`] proves the entries of each table it can from the
//! build's facts, and the function is analyzed again following them, which
//! can reach a dispatch only a table's case leads to (a nested `switch`). A
//! pass is kept only while its own records prove again every table it
//! followed, unchanged, and more: then the records it emits are exactly
//! those the reported tables' jumps produced. A pass that proves the same
//! tables ends the search; one that loses or changes a table (a case label
//! jumping back into its dispatch block) means no table is proven, and the
//! function is reported from a pass that follows none, its dispatches
//! indirect gaps. Passes end within the function's dispatch count, every
//! pass charged to the same control.
use crate::{AnalysisSummary, FunctionInput, KnownJump, research};
use oer_analysis_riscv_model::*;

/// The tables one pass over a function proves, with the working memory
/// that admits them, their entries and the jumps the next pass follows.
pub struct ProvenTables<'m, T> {
    pub tables: Vec<T>,
    pub reserved: AdmittedVec<'m, MemoryReservation<'m>>,
}

impl<'m, T> ProvenTables<'m, T> {
    pub fn none(memory: &'m WorkingMemory) -> Self {
        Self {
            tables: Vec::new(),
            reserved: AdmittedVec::new(memory),
        }
    }
}

/// Where the entries of a function's jump tables come from: the facts of
/// one build, such as an object's relocations or an image's relocated
/// words and jump-table labels.
pub trait JumpTableSource {
    /// What the source proves of one table.
    type Table;

    /// Whether `earlier` and `later` prove the same table: the same jump
    /// through the same entries. What a table reports beyond them, such as
    /// a case value only a known index shows, may differ between passes.
    fn same(&self, earlier: &Self::Table, later: &Self::Table) -> bool;

    /// The tables the `records` of one pass over the function at `extent`
    /// prove: each a dispatch the records locate whose every followed entry
    /// the source proves and which lands inside `extent`. The result's
    /// reservations admit the tables and the jumps [`Self::jump`] makes of
    /// them.
    fn tables<'m>(
        &self,
        records: &[FunctionRecord],
        extent: CodeRange,
        memory: &'m WorkingMemory,
        control: &mut dyn RunControl,
    ) -> Result<ProvenTables<'m, Self::Table>>;

    /// The jump the control-flow graph follows for `table`: its site and
    /// its distinct targets in ascending order.
    fn jump(&self, table: &Self::Table) -> KnownJump;
}

/// Collects one pass's records into admitted memory.
pub(crate) struct Buffer<'a, 'm>(pub(crate) &'a mut RecordBuffer<'m>);

impl FunctionSink for Buffer<'_, '_> {
    fn record(&mut self, record: &FunctionRecord, control: &mut dyn RunControl) -> Result<()> {
        control.checkpoint(1)?;
        self.0.push(record.clone(), control.position())
    }
}

/// [`research`] of `input` following the jump tables `source` proves, as
/// the module describes. `input.jumps` are followed on every pass, such as
/// the dispatches a caller located otherwise; the source proves the rest.
/// Returns the summary of the reported pass and the tables it followed.
#[allow(clippy::too_many_arguments)]
pub fn research_following_tables<'m, S: JumpTableSource + ?Sized>(
    input: FunctionInput<'_>,
    source: &S,
    decoder: &dyn FunctionSemantics,
    memory: &'m WorkingMemory,
    control: &mut dyn RunControl,
    sink: &mut dyn FunctionSink,
    abi: Option<CallAbi>,
) -> Result<(AnalysisSummary, ProvenTables<'m, S::Table>)> {
    let known = input.jumps;
    let pass = |jumps: &[KnownJump], control: &mut dyn RunControl, sink: &mut dyn FunctionSink| {
        research(
            FunctionInput { jumps, ..input },
            decoder,
            memory,
            control,
            sink,
            abi,
        )
    };
    let mut followed = ProvenTables::none(memory);
    let mut records = RecordBuffer::new(memory);
    let mut summary = pass(known, control, &mut Buffer(&mut records))?;
    loop {
        let found = source.tables(&records, input.extent, memory, control)?;
        let keeps = followed
            .tables
            .iter()
            .all(|table| found.tables.iter().any(|other| source.same(table, other)));
        if keeps && found.tables.len() == followed.tables.len() {
            for record in records.iter() {
                sink.record(record, control)?;
            }
            return Ok((summary, followed));
        }
        if !keeps {
            drop(records);
            let summary = pass(known, control, sink)?;
            return Ok((summary, ProvenTables::none(memory)));
        }
        // The pass's jumps: the source's reservations admit its tables'
        // jumps; this admits the list and the caller's jumps copied into it.
        let known_targets: usize = known.iter().map(|jump| jump.targets.len()).sum();
        let bytes = (known.len() + found.tables.len()) * size_of::<KnownJump>()
            + known_targets * size_of::<u64>();
        let _jumps = memory.reserve(bytes as u64, control.position())?;
        let mut jumps = Vec::new();
        jumps
            .try_reserve_exact(known.len() + found.tables.len())
            .map_err(|_| Error::new(ErrorCode::ResourceLimited, "jump allocation refused"))?;
        jumps.extend_from_slice(known);
        jumps.extend(found.tables.iter().map(|table| source.jump(table)));
        followed = found;
        records = RecordBuffer::new(memory);
        summary = pass(&jumps, control, &mut Buffer(&mut records))?;
    }
}
