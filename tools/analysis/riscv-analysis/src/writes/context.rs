//! Conditional exclusions. The complete generic effects remain in the summary;
//! every application rechecks the actual Transfer, ABI and required constants.
use super::*;
use crate::{FunctionInput, FunctionSemantics};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CallContext {
    pub(crate) entry: u64,
    pub(crate) abi: CallAbi,
    pub(crate) arguments: [Option<u32>; 8],
}
impl CallContext {
    pub fn entry(&self) -> u64 {
        self.entry
    }
    pub fn arguments(&self) -> &[Option<u32>; 8] {
        &self.arguments
    }
    pub(super) fn applies(
        &self,
        entry: u64,
        abi: Option<CallAbi>,
        arguments: &[WriteValue; 8],
    ) -> bool {
        self.entry == entry
            && abi == Some(self.abi)
            && self
                .arguments
                .iter()
                .zip(arguments)
                .all(|(required, actual)| required.is_none_or(|v| actual.constant() == Some(v)))
    }
}

/// Produce assumptions only from the canonical caller facts and ABI mapping.
/// A single transparent tail may transport them without changing registers.
#[allow(clippy::too_many_arguments)]
pub fn call_context(
    caller: &FunctionIndex<'_, '_>,
    entry: u64,
    site: u64,
    abi: Option<CallAbi>,
    summary: &FunctionWrites<'_>,
    memory: &WorkingMemory,
    c: &mut dyn RunControl,
) -> Result<Option<CallContext>> {
    let Some(abi) = abi.filter(|abi| Some(*abi) == summary.abi) else {
        return Ok(None);
    };
    for record in caller.records() {
        c.checkpoint(1)?;
        if !closes_local_graph(record) {
            return Ok(None);
        }
    }
    if !summary.closed_graph {
        return Ok(None);
    }
    let Some(transfer) = caller.transfer(site, c)? else {
        return Ok(None);
    };
    if !transfer.call
        || *transfer.target
            != (AbstractValue::ImageAddress {
                address: summary.entry as u32,
            })
    {
        return Ok(None);
    }
    let Some(inputs) = caller.call_inputs(site, c)? else {
        return Ok(None);
    };
    let mut values = Values::new(entry, Some(abi), true, memory);
    values.prepare(caller, c)?;
    let mut arguments = [None; 8];
    for (argument, register) in arguments.iter_mut().zip(abi.arguments()) {
        c.checkpoint(1)?;
        *argument = inputs
            .registers
            .get(usize::from(register))
            .and_then(|v| values.value(v).constant());
    }
    if arguments.iter().all(Option::is_none) {
        return Ok(None);
    }
    Ok(Some(CallContext {
        entry: summary.tail.map_or(summary.entry, |tail| tail.target),
        abi,
        arguments,
    }))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BranchProof {
    pub site: u64,
    pub test: BranchTest,
    pub left: [u32; 8],
    pub left_count: u8,
    pub right: [u32; 8],
    pub right_count: u8,
    pub taken: bool,
}

pub struct ContextProof<'m> {
    pub(crate) context: CallContext,
    pub(crate) owner: u64,
    pub(crate) branches: AdmittedVec<'m, BranchProof>,
    pub(crate) unreachable: AdmittedVec<'m, u64>,
}
impl ContextProof<'_> {
    pub fn context(&self) -> &CallContext {
        &self.context
    }
    pub fn branches(&self) -> &[BranchProof] {
        &self.branches
    }
    pub fn unreachable(&self) -> &[u64] {
        &self.unreachable
    }
    pub(super) fn excludes(&self, owner: u64, site: u64) -> bool {
        owner == self.owner && self.unreachable.binary_search(&site).is_ok()
    }
}

pub(crate) struct ContextRun<'a, 'm> {
    pub context: &'a CallContext,
    pub proof: Option<ContextProof<'m>>,
}
struct Saved<'m>(RecordBuffer<'m>);
impl FunctionSink for Saved<'_> {
    fn record(&mut self, record: &FunctionRecord, c: &mut dyn RunControl) -> Result<()> {
        self.0.push(record.clone(), c.position())
    }
}

/// Opt-in specialization through the very same CFG and value engine. Its
/// canonical records and generic write list remain conservative; exclusions
/// are carried separately and cannot be used without checking applicability.
#[allow(clippy::too_many_arguments)]
pub fn contextual_function_writes<'m>(
    input: FunctionInput<'_>,
    isa: &dyn FunctionSemantics,
    context: &CallContext,
    summaries: &[&FunctionWrites<'_>],
    memory: &'m WorkingMemory,
    c: &mut dyn RunControl,
) -> Result<FunctionWrites<'m>> {
    if input.extent.start != context.entry {
        return Err(Error::new(
            ErrorCode::InvalidRequest,
            "call context belongs to a different target",
        ));
    }
    let mut saved = Saved(RecordBuffer::new(memory));
    let mut run = ContextRun {
        context,
        proof: None,
    };
    crate::analyze_with(
        input,
        isa,
        memory,
        c,
        &mut saved,
        true,
        Some(context.abi),
        true,
        None,
        Some(&mut run),
    )?;
    let facts = FunctionIndex::new(&saved.0, RecordPolicy::Canonical, memory, c)?;
    let mut summary = function_writes(
        &facts,
        context.entry,
        Some(context.abi),
        summaries,
        memory,
        c,
    )?;
    summary.context = run.proof;
    Ok(summary)
}
