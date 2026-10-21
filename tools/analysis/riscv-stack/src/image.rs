//! Functions, compiler frame records and value-analysis observations of one
//! static RV32 image.
use crate::TransferKind;
use oer_analysis_riscv_analysis::{
    FunctionInput, JumpTableSource, KnownJump, PreparedReferences, research_following_tables,
};
use oer_analysis_riscv_lift::RiscvDecoder;
use oer_analysis_riscv_model::Ledger;
use oer_analysis_riscv_model::*;
use oer_analysis_riscv_program::ProgramView;
use oer_analysis_riscv_program::facts::ImageFacts;
use std::collections::BTreeMap;

const SP: u8 = 2;

#[cfg(test)]
#[path = "image_tests.rs"]
mod tests;

/// A defined code symbol and every alias at its address.
pub use oer_analysis_elf::Function;

fn invalid(message: impl Into<String>) -> Error {
    Error::new(ErrorCode::Integrity, message)
}

/// The words of the data object that starts at `address` in an unwritable
/// section: a table whose every entry an index the language bounds-checks
/// against the object's length may load.
pub(crate) fn table(
    image: &ImageFacts<'_>,
    address: u32,
    ledger: &mut Ledger<'_>,
    control: &mut dyn RunControl,
) -> Result<Option<Vec<u32>>> {
    let Some(size) = image.data_object(address) else {
        return Ok(None);
    };
    words(image, address, size / 4, ledger, control)
}

/// The `count` read-only words from `base`, each charged to `control`
/// before it is read; `None` at the first word outside read-only data.
pub(crate) fn words(
    image: &ImageFacts<'_>,
    base: u32,
    count: u64,
    ledger: &mut Ledger<'_>,
    control: &mut dyn RunControl,
) -> Result<Option<Vec<u32>>> {
    let mut words = Vec::new();
    for i in 0..count {
        control.checkpoint(1)?;
        let Some(word) = image.read_only_word(base.wrapping_add(4 * i as u32)) else {
            return Ok(None);
        };
        ledger.push(&mut words, word, control)?;
    }
    Ok(Some(words))
}

/// The entries of the jump table the compiler labelled at `base`, each
/// charged to `control` as it is read.
pub(crate) fn jump_table(
    image: &ImageFacts<'_>,
    base: u32,
    ledger: &mut Ledger<'_>,
    control: &mut dyn RunControl,
) -> Result<Option<Vec<u32>>> {
    let Some(entries) = image.jump_table(base) else {
        return Ok(None);
    };
    let mut words = Vec::new();
    for entry in entries {
        control.checkpoint(1)?;
        ledger.push(&mut words, entry, control)?;
    }
    Ok(Some(words))
}

fn uleb128(bytes: &[u8], at: &mut usize) -> Result<u64> {
    let mut value = 0u64;
    for shift in (0..64).step_by(7) {
        let byte = *bytes
            .get(*at)
            .ok_or_else(|| invalid("truncated .stack_sizes entry"))?;
        *at += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err(invalid("oversized .stack_sizes value"))
}

/// The compiler's frame record of each function: `.stack_sizes` entries of a
/// 32-bit address followed by a ULEB128 size. An image without the section has
/// no records; one function recorded twice with different sizes is an error.
pub fn stack_sizes(
    image: &ImageFacts<'_>,
    keep: &mut Ledger<'_>,
    control: &mut dyn RunControl,
) -> Result<BTreeMap<u32, u64>> {
    let file = image.file();
    let mut sizes = BTreeMap::new();
    let Some(section) = file.section_by_name(".stack_sizes") else {
        return Ok(sizes);
    };
    let bytes = section.data;
    // At least five bytes per record.
    control.checkpoint(bytes.len() as u64 / 5)?;
    let mut at = 0;
    while at < bytes.len() {
        let address = bytes
            .get(at..at + 4)
            .ok_or_else(|| invalid("truncated .stack_sizes address"))?;
        let address = u32::from_le_bytes(address.try_into().unwrap());
        at += 4;
        let size = uleb128(bytes, &mut at)?;
        if let Some(previous) = keep.insert(&mut sizes, address, size, control)?
            && previous != size
        {
            return Err(invalid(format!(
                "{address:#010x} has .stack_sizes {previous} and {size}"
            )));
        }
    }
    Ok(sizes)
}

/// The bytes of `function` in its section.
pub(crate) fn function_bytes<'a>(
    file: &oer_analysis_elf::Elf<'a>,
    function: &Function,
) -> Result<&'a [u8]> {
    let section = file
        .section(function.section)
        .map_err(|_| invalid("function in a missing section"))?;
    let start = u64::from(function.address)
        .checked_sub(section.address)
        .ok_or_else(|| invalid("function before its section"))? as usize;
    section
        .data
        .get(start..start + function.size as usize)
        .ok_or_else(|| invalid(format!("{} exceeds its section", function.label())))
}

/// What the value analysis observed in one function.
pub(crate) struct Observation {
    /// The deepest entry-relative `sp`, unless some `sp` value is not entry-relative.
    pub depth: Option<u64>,
    pub complete: bool,
    /// Transfers whose target the value analysis resolved.
    pub resolved: Vec<(u32, u32, TransferKind)>,
    /// Sites of indirect jumps whose targets the analysis found inside the
    /// function, and those it followed through a proven jump table, whose
    /// entries all lie inside it: code the linear sweep already covers.
    pub local_jumps: Vec<u32>,
    /// Bytes below the entry `sp` at each transfer the analysis reached.
    pub site_depths: BTreeMap<u32, u64>,
    /// The exact integer registers at each transfer the analysis reached.
    pub site_registers: BTreeMap<u32, [Option<u32>; 32]>,
    pub predecessors: BTreeMap<u32, Vec<u32>>,
    open_graph: OpenGraph,
}

impl Observation {
    pub fn closed_graph(&self) -> bool {
        self.open_graph.closed()
    }

    /// Only this unresolved external indirect exit is missing. Its eventual
    /// targets must all be outside the function before using this proof.
    pub fn closed_except(&self, site: u32) -> bool {
        !self.open_graph.whole
            && self.open_graph.sites.len() == 1
            && self.open_graph.sites.get(&u64::from(site)) == Some(&true)
            && !self
                .predecessors
                .values()
                .any(|incoming| incoming.contains(&site))
    }
}

#[derive(Default)]
struct OpenGraph {
    // A site is eligible only if every opening record there is an
    // unresolved external indirect edge, rather than a gap or conflict.
    sites: BTreeMap<u64, bool>,
    whole: bool,
}

impl OpenGraph {
    fn closed(&self) -> bool {
        !self.whole && self.sites.is_empty()
    }

    fn record(
        &mut self,
        record: &FunctionRecord,
        ledger: &mut Ledger<'_>,
        control: &mut dyn RunControl,
    ) -> Result<()> {
        if oer_analysis_riscv_analysis::writes::closes_local_graph(record) {
            return Ok(());
        }
        let (site, eligible) = match record {
            FunctionRecord::Edge {
                from,
                target,
                relation,
                external,
            } => (
                *from,
                target.is_none() && *relation == EdgeKind::Indirect && *external,
            ),
            FunctionRecord::Gap { start, .. } => (*start, false),
            _ => {
                self.whole = true;
                return Ok(());
            }
        };
        *ledger.entry(&mut self.sites, site, || true, control)? &= eligible;
        Ok(())
    }
}

/// The value analysis's records of one function, read into what the stack
/// analysis keeps; their growth is admitted in the function's ledger.
struct Observer<'l, 'm> {
    ledger: &'l mut Ledger<'m>,
    deepest: i64,
    dynamic: bool,
    unexpanded: Vec<u32>,
    site_depths: BTreeMap<u32, u64>,
    site_registers: BTreeMap<u32, [Option<u32>; 32]>,
    transfers: Vec<(u32, u32, TransferKind)>,
    predecessors: BTreeMap<u32, Vec<u32>>,
    open_graph: OpenGraph,
}

impl FunctionSink for Observer<'_, '_> {
    fn record(&mut self, record: &FunctionRecord, control: &mut dyn RunControl) -> Result<()> {
        self.open_graph.record(record, self.ledger, control)?;
        match record {
            FunctionRecord::Edge {
                from,
                target,
                external,
                ..
            } => {
                if let Some(target) = target
                    && !external
                {
                    let incoming = self.ledger.entry(
                        &mut self.predecessors,
                        *target as u32,
                        Vec::new,
                        control,
                    )?;
                    if !incoming.contains(&(*from as u32)) {
                        self.ledger.push(incoming, *from as u32, control)?;
                    }
                }
            }
            FunctionRecord::Value {
                register: SP,
                value,
                ..
            } => match value {
                AbstractValue::EntryStack { offset } => self.deepest = self.deepest.min(*offset),
                _ => self.dynamic = true,
            },
            FunctionRecord::Transfer {
                offset,
                target,
                call,
            } => {
                let kind = if *call {
                    TransferKind::Call
                } else {
                    TransferKind::Tail
                };
                // Every alternative must be an address for the site to count
                // as resolved.
                let addresses: Option<Vec<u32>> = match target {
                    AbstractValue::ImageAddress { address } => Some(vec![*address]),
                    AbstractValue::Alternatives { values } => values
                        .values()
                        .iter()
                        .map(|value| match value {
                            ValueAlternative::ImageAddress { address } => Some(*address),
                            _ => None,
                        })
                        .collect(),
                    _ => None,
                };
                for address in addresses.into_iter().flatten() {
                    self.ledger.push(
                        &mut self.transfers,
                        (*offset as u32, address, kind),
                        control,
                    )?;
                }
            }
            FunctionRecord::SemanticGap {
                offset,
                reason: SemanticGapReason::UnexpandedControlFlow,
            } => self
                .ledger
                .push(&mut self.unexpanded, *offset as u32, control)?,
            FunctionRecord::CallInputs { offset, registers } => {
                if let Some(AbstractValue::EntryStack { offset: sp }) = registers.get(SP as usize) {
                    self.ledger.insert(
                        &mut self.site_depths,
                        *offset as u32,
                        sp.unsigned_abs(),
                        control,
                    )?;
                }
                self.ledger.insert(
                    &mut self.site_registers,
                    *offset as u32,
                    exact_registers(registers),
                    control,
                )?;
            }
            _ => {}
        }
        Ok(())
    }
}

/// The exact integer registers among a call site's inputs.
pub(crate) fn exact_registers(registers: &[AbstractValue]) -> [Option<u32>; 32] {
    let mut exact = [None; 32];
    for (slot, value) in exact.iter_mut().zip(registers) {
        *slot = match value {
            AbstractValue::Constant { value: v } | AbstractValue::ImageAddress { address: v } => {
                Some(*v)
            }
            _ => None,
        };
    }
    exact
}

/// The jump tables of a function's dispatches, from the facts of its image.
pub(crate) type Tables<'a> = dyn JumpTableSource<Table = JumpDispatch> + 'a;

/// The bounded value analysis of one function, its control-flow graph
/// following the indirect jumps the given jumps name and the jump tables
/// the given source proves ([`research_following_tables`]).
pub(crate) type Observe<'a> = dyn FnMut(
        &Function,
        &[KnownJump],
        &Tables<'_>,
        &mut Ledger<'_>,
        &mut dyn RunControl,
    ) -> Result<Observation>
    + 'a;

/// Run `body` with an observer of `image`'s functions: `observe(function,
/// jumps, tables, ledger)` runs the bounded value analysis over one
/// function, its control-flow graph following the indirect jumps `jumps`
/// names and the tables `tables` proves, admitting the observation in
/// `ledger`.
pub(crate) fn observing<R>(
    image: &ImageFacts<'_>,
    call_behavior: &dyn CallBehavior,
    traps: oer_analysis_riscv_analysis::TrapSemantics,
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
    body: impl FnOnce(&mut Observe<'_>, &mut dyn RunControl) -> Result<R>,
) -> Result<R> {
    let file = image.file();
    let view = ProgramView::new(image.bytes(), file, &[], memory, control)?;
    let mut observe = |function: &Function,
                       jumps: &[KnownJump],
                       tables: &Tables<'_>,
                       ledger: &mut Ledger<'_>,
                       control: &mut dyn RunControl|
     -> Result<Observation> {
        let code = function_bytes(file, function)?;
        let references =
            PreparedReferences::new(&[], function.section as u32, &RiscvDecoder, memory, control)?;
        let mut observer = Observer {
            ledger,
            deepest: 0,
            dynamic: false,
            unexpanded: Vec::new(),
            site_depths: BTreeMap::new(),
            site_registers: BTreeMap::new(),
            transfers: Vec::new(),
            predecessors: BTreeMap::new(),
            open_graph: OpenGraph::default(),
        };
        let (summary, followed) = research_following_tables(
            FunctionInput {
                traps,
                image: Some(&view),
                section: function.section as u32,
                extent: CodeRange {
                    start: u64::from(function.address),
                    length: u64::from(function.size),
                },
                bytes: code,
                relocations: &references,
                data_ranges: &[],
                jumps,
                returns: &NoSummaries,
                call_behavior,
            },
            tables,
            &RiscvDecoder,
            memory,
            control,
            &mut observer,
            // The psABI preserves sp and the saved registers across calls.
            Some(CallAbi::RiscvInteger),
        )?;
        let complete = summary.coverage.decoding
            && summary.coverage.control_flow
            && observer.unexpanded.is_empty()
            && !observer.dynamic;
        let depth = (!observer.dynamic).then_some(observer.deepest.unsigned_abs());
        let mut local_jumps = observer.unexpanded;
        for table in &followed.tables {
            observer
                .ledger
                .push(&mut local_jumps, table.site() as u32, control)?;
        }
        Ok(Observation {
            depth,
            complete,
            resolved: observer.transfers,
            local_jumps,
            site_depths: observer.site_depths,
            site_registers: observer.site_registers,
            predecessors: observer.predecessors,
            open_graph: observer.open_graph,
        })
    };
    body(&mut observe, control)
}
