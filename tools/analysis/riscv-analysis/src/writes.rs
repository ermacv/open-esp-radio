//! Bounded write-event summaries over the common engine's canonical facts.
//!
//! Writes and barriers describe events, never memory postconditions. A field
//! denotes a value captured at its load PC, not the field's entry value.
//! The separate opt-in WordPostcondition proves one word at normal return,
//! using the common evaluator; it does not remove any generic write event.
use oer_analysis_riscv_model::*;

pub(crate) mod context;
mod tail;
mod word;
pub use context::{
    BranchProof, CallContext, ContextProof, call_context, contextual_function_writes,
};
pub use word::{
    CallWord, WordBarrier, WordContents, WordEvent, WordEventKind, WordInstance, WordPostcondition,
    WordQuery, call_word, contextual_word_function_writes, instantiate_word,
};

/// A machine-proved, state-preserving tail transfer. Composition is bounded
/// to this one hop; the target's local expression identities remain its own.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TailTransfer {
    pub entry: u64,
    pub site: u64,
    pub target: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WriteOrigin {
    /// Owner of the original instruction and its expression identities.
    pub function: u64,
    pub tail: Option<TailTransfer>,
}
impl WriteOrigin {
    fn local(function: u64) -> Self {
        Self {
            function,
            tail: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WriteBase {
    Unknown,
    Argument(u8),
    Address(u32),
    Stack {
        function: u64,
        offset: i64,
    },
    /// A function-local expression identity, not an exact object address.
    Expression {
        function: u64,
        id: u32,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WriteValue {
    Base {
        base: WriteBase,
        offset: u32,
    },
    /// Only one field load through a proven base and constant displacement.
    Field {
        function: u64,
        read: u64,
        base: WriteBase,
        displacement: u32,
        width: u8,
        signed: bool,
        offset: u32,
    },
}
impl WriteValue {
    pub const UNKNOWN: Self = Self::base(WriteBase::Unknown);
    pub const fn base(base: WriteBase) -> Self {
        Self::Base { base, offset: 0 }
    }
    fn add(self, delta: u32) -> Self {
        match self {
            Self::Base {
                base: WriteBase::Unknown,
                ..
            } => Self::UNKNOWN,
            Self::Base {
                base: WriteBase::Address(value),
                offset,
            } => Self::base(WriteBase::Address(
                value.wrapping_add(offset).wrapping_add(delta),
            )),
            Self::Base { base, offset } => Self::Base {
                base,
                offset: offset.wrapping_add(delta),
            },
            Self::Field {
                function,
                read,
                base,
                displacement,
                width,
                signed,
                offset,
            } => Self::Field {
                function,
                read,
                base,
                displacement,
                width,
                signed,
                offset: offset.wrapping_add(delta),
            },
        }
    }
    fn constant(self) -> Option<u32> {
        match self {
            Self::Base {
                base: WriteBase::Address(value),
                offset,
            } => Some(value.wrapping_add(offset)),
            _ => None,
        }
    }
    /// False includes a known load event through an unknown object pointer.
    pub fn known_base(self) -> bool {
        !matches!(
            self,
            Self::Base {
                base: WriteBase::Unknown,
                ..
            } | Self::Field {
                base: WriteBase::Unknown,
                ..
            }
        )
    }
    fn opaque(self) -> bool {
        !self.known_base()
            || matches!(
                self,
                Self::Base {
                    base: WriteBase::Expression { .. },
                    ..
                } | Self::Field {
                    base: WriteBase::Expression { .. },
                    ..
                }
            )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WriteOccurrence {
    /// The event executes on every modeled invocation. Currently proved only
    /// for a complete straight-line function ending in a return.
    Must,
    /// Includes a no-write path. Never kills a previous memory value.
    May,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WriteProof {
    ClosedGraph,
    /// Finite addresses from the common pass's all-entry induction proof.
    AlignmentLoop(AlignmentLoopProof),
    /// Values are unknown when a local incoming path is unaccounted for.
    OpenGraph,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FunctionWrite {
    pub origin: WriteOrigin,
    pub site: u64,
    pub address: WriteValue,
    pub value: WriteValue,
    pub width: u8,
    pub occurrence: WriteOccurrence,
    pub proof: WriteProof,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WriteBarrierKind {
    Call,
    Atomic,
    UnknownStore,
    /// Destination and width are known; the stored contents are opaque.
    UnknownValue,
    Unsupported,
    OpenGraph,
    ExternalTransfer,
    UnknownCallee,
    UnknownAbi,
    TailCycle,
    TailChain,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WriteBarrier {
    pub origin: WriteOrigin,
    pub site: u64,
    pub kind: WriteBarrierKind,
}

pub struct FunctionWrites<'m> {
    pub entry: u64,
    pub abi: Option<CallAbi>,
    pub closed_graph: bool,
    /// Proven transfer, also retained when its target summary is unavailable
    /// or composition is blocked. Barriers describe any remaining uncertainty.
    pub tail: Option<TailTransfer>,
    pub writes: AdmittedVec<'m, FunctionWrite>,
    pub barriers: AdmittedVec<'m, WriteBarrier>,
    pub reloads: AdmittedVec<'m, StackReload>,
    context: Option<ContextProof<'m>>,
    word: Option<WordPostcondition<'m>>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StackReload {
    pub origin: WriteOrigin,
    pub site: u64,
    /// Offset in the containing summary's entry-stack coordinates; `origin`
    /// retains the function that owns the original load instruction.
    pub slot: i64,
    pub width: u8,
    pub value: WriteValue,
}
impl FunctionWrites<'_> {
    pub fn word_postcondition(&self) -> Option<&WordPostcondition<'_>> {
        self.word.as_ref()
    }
    /// Conditional exclusions; ordinary `writes` and `barriers` stay complete.
    pub fn specialization(&self) -> Option<&ContextProof<'_>> {
        self.context.as_ref()
    }
    pub fn has_unknown_memory_effects(&self) -> bool {
        !self.barriers.is_empty()
    }
}

struct Values<'m> {
    expressions: AdmittedVec<'m, WriteValue>,
    entry: u64,
    abi: Option<CallAbi>,
    caller: bool,
}
impl<'m> Values<'m> {
    fn new(entry: u64, abi: Option<CallAbi>, caller: bool, memory: &'m WorkingMemory) -> Self {
        Self {
            expressions: AdmittedVec::new(memory),
            entry,
            abi,
            caller,
        }
    }
    fn value(&self, value: &AbstractValue) -> WriteValue {
        match value {
            AbstractValue::Constant { value } | AbstractValue::ImageAddress { address: value } => {
                WriteValue::base(WriteBase::Address(*value))
            }
            AbstractValue::EntryStack { offset } => WriteValue::base(WriteBase::Stack {
                function: self.entry,
                offset: *offset,
            }),
            AbstractValue::Expression { id } => self
                .expressions
                .get(*id as usize)
                .copied()
                .unwrap_or(WriteValue::UNKNOWN),
            _ => WriteValue::UNKNOWN,
        }
    }
    fn prepare(&mut self, facts: &FunctionIndex<'_, '_>, c: &mut dyn RunControl) -> Result<()> {
        for record in facts.records() {
            c.checkpoint(1)?;
            let FunctionRecord::Expression {
                id,
                offset,
                expression,
            } = record
            else {
                continue;
            };
            if *id as usize != self.expressions.len() {
                return Err(Error::new(
                    ErrorCode::Integrity,
                    "write summaries require canonical expression IDs",
                ));
            }
            let value = match expression {
                Expression::EntryRegister { register: 0 } => {
                    WriteValue::base(WriteBase::Address(0))
                }
                Expression::EntryRegister { register } => self
                    .abi
                    .and_then(|abi| abi.argument_word(*register))
                    .map_or(WriteValue::UNKNOWN, |word| {
                        WriteValue::base(WriteBase::Argument(word as u8))
                    }),
                Expression::Integer { op, left, right } => {
                    let (a, b) = (self.value(left), self.value(right));
                    match (op, a.constant(), b.constant()) {
                        (_, Some(a), Some(b)) => {
                            WriteValue::base(WriteBase::Address(op.evaluate(a, b)))
                        }
                        (IntegerOp::Add, _, Some(b)) => a.add(b),
                        (IntegerOp::Add, Some(a), _) => b.add(a),
                        (IntegerOp::Sub, _, Some(b)) => a.add(b.wrapping_neg()),
                        (IntegerOp::And, _, Some(u32::MAX)) => a,
                        (IntegerOp::And, Some(u32::MAX), _) => b,
                        _ => WriteValue::UNKNOWN,
                    }
                }
                Expression::Load {
                    address,
                    width,
                    signed,
                } => match self.value(address) {
                    WriteValue::Base {
                        base: base @ (WriteBase::Argument(_) | WriteBase::Address(_)),
                        offset: displacement,
                    } => WriteValue::Field {
                        function: self.entry,
                        read: *offset,
                        base,
                        displacement,
                        width: *width,
                        signed: *signed,
                        offset: 0,
                    },
                    WriteValue::Field { .. }
                    | WriteValue::Base {
                        base: WriteBase::Expression { .. },
                        ..
                    } => WriteValue::base(WriteBase::Expression {
                        function: self.entry,
                        id: *id,
                    }),
                    _ if self.caller || !matches!(address, AbstractValue::EntryStack { .. }) => {
                        WriteValue::base(WriteBase::Expression {
                            function: self.entry,
                            id: *id,
                        })
                    }
                    _ => WriteValue::UNKNOWN,
                },
                _ => WriteValue::UNKNOWN,
            };
            self.expressions.push(value, c.position())?;
        }
        Ok(())
    }
}

/// The same closure contract used by task publication: unknown calls retain
/// their continuation, unknown non-calls can enter unobserved local blocks.
pub fn closes_local_graph(record: &FunctionRecord) -> bool {
    match record {
        FunctionRecord::Gap { reason, .. } => *reason == GapReason::Unvisited,
        FunctionRecord::Edge {
            target, relation, ..
        } => closes_local_edge(*target, *relation),
        _ => true,
    }
}

pub(crate) fn closes_local_edge(target: Option<u64>, relation: EdgeKind) -> bool {
    !matches!(relation, EdgeKind::Conflict | EdgeKind::Stop)
        && (target.is_some() || matches!(relation, EdgeKind::Call | EdgeKind::Return))
}

/// No new CFG or instruction interpreter: consumes canonical expressions,
/// memory observations, decoded instruction flows and existing graph edges.
/// Supplied summaries may be composed through one proven transparent tail
/// instruction. Missing summaries, cycles and longer chains retain barriers;
/// target effects retain their original PCs, owners and unknown reasons.
pub fn function_writes<'m>(
    facts: &FunctionIndex<'_, '_>,
    entry: u64,
    abi: Option<CallAbi>,
    summaries: &[&FunctionWrites<'_>],
    memory: &'m WorkingMemory,
    c: &mut dyn RunControl,
) -> Result<FunctionWrites<'m>> {
    let mut values = Values::new(entry, abi, false, memory);
    values.prepare(facts, c)?;
    let mut out = FunctionWrites {
        entry,
        abi,
        closed_graph: true,
        tail: None,
        writes: AdmittedVec::new(memory),
        barriers: AdmittedVec::new(memory),
        reloads: AdmittedVec::new(memory),
        context: None,
        word: None,
    };
    let mut next = entry;
    let mut straight = true;
    let mut returned = false;
    let mut register_value = None;
    let mut address_range = None;
    for record in facts.records() {
        c.checkpoint(1)?;
        out.closed_graph &= closes_local_graph(record);
        let barrier = match record {
            FunctionRecord::StoreAddressRange {
                offset,
                initial,
                first_offset,
                count,
                proof,
            } => {
                address_range = Some((
                    *offset,
                    values.value(initial),
                    *first_offset,
                    *count,
                    *proof,
                ));
                None
            }
            FunctionRecord::Value { offset, value, .. } => {
                register_value = Some((*offset, values.value(value)));
                None
            }
            FunctionRecord::MemoryAccess {
                offset,
                access: MemoryKind::Load,
                width: 4,
                address: AbstractValue::EntryStack { offset: slot },
                ..
            } => {
                let value = register_value
                    .filter(|(site, _)| site == offset)
                    .map_or(WriteValue::UNKNOWN, |(_, value)| value);
                out.reloads.push(
                    StackReload {
                        origin: WriteOrigin::local(entry),
                        site: *offset,
                        slot: *slot,
                        width: 4,
                        value,
                    },
                    c.position(),
                )?;
                None
            }
            FunctionRecord::Instruction {
                offset, decoded, ..
            } => {
                straight &= !returned && *offset == next;
                next = offset.saturating_add(u64::from(decoded.length));
                match decoded.flow {
                    InstructionFlow::Next => (),
                    InstructionFlow::Return { .. } => returned = true,
                    _ => straight = false,
                }
                None
            }
            FunctionRecord::MemoryAccess {
                offset,
                access: MemoryKind::Store,
                width,
                address,
                value,
                ..
            } => {
                let range = address_range
                    .take()
                    .filter(|(site, initial, _, count, proof)| {
                        site == offset
                            && *width == 1
                            && !initial.opaque()
                            && *count == proof.mask
                            && matches!(count, 1 | 3 | 7)
                    });
                let address = range.map_or_else(
                    || values.value(address),
                    |(_, initial, first, _, _)| initial.add(first),
                );
                let value = value
                    .as_ref()
                    .map_or(WriteValue::UNKNOWN, |v| values.value(v));
                let value = if matches!(width, 1 | 2 | 4) {
                    value
                } else {
                    WriteValue::UNKNOWN
                };
                for delta in 0..range.map_or(1, |(_, _, _, count, _)| count) {
                    c.checkpoint(1)?;
                    out.writes.push(
                        FunctionWrite {
                            origin: WriteOrigin::local(entry),
                            site: *offset,
                            address: address.add(u32::from(delta)),
                            value,
                            width: *width,
                            occurrence: WriteOccurrence::May,
                            proof: range.map_or(WriteProof::OpenGraph, |(_, _, _, _, proof)| {
                                WriteProof::AlignmentLoop(proof)
                            }),
                        },
                        c.position(),
                    )?;
                }
                if address.opaque() {
                    Some((*offset, WriteBarrierKind::UnknownStore))
                } else if value.opaque() {
                    Some((*offset, WriteBarrierKind::UnknownValue))
                } else {
                    None
                }
            }
            FunctionRecord::MemoryAccess { offset, access, .. }
                if !matches!(access, MemoryKind::Load | MemoryKind::FloatLoad) =>
            {
                Some((*offset, WriteBarrierKind::Atomic))
            }
            FunctionRecord::SemanticGap { offset, reason } => {
                straight = false;
                Some((
                    *offset,
                    if *reason == SemanticGapReason::OpaqueCall {
                        WriteBarrierKind::Call
                    } else {
                        WriteBarrierKind::Unsupported
                    },
                ))
            }
            FunctionRecord::Transfer { offset, call, .. } => {
                straight = false;
                Some((
                    *offset,
                    if *call {
                        WriteBarrierKind::Call
                    } else {
                        WriteBarrierKind::ExternalTransfer
                    },
                ))
            }
            _ => None,
        };
        if let Some((site, kind)) = barrier {
            out.barriers.push(
                WriteBarrier {
                    origin: WriteOrigin::local(entry),
                    site,
                    kind,
                },
                c.position(),
            )?;
        }
    }
    if !out.closed_graph {
        out.barriers.push(
            WriteBarrier {
                origin: WriteOrigin::local(entry),
                site: entry,
                kind: WriteBarrierKind::OpenGraph,
            },
            c.position(),
        )?;
    }
    for write in out.writes.iter_mut() {
        c.checkpoint(1)?;
        if out.closed_graph {
            if write.proof == WriteProof::OpenGraph {
                write.proof = WriteProof::ClosedGraph;
            }
            if straight && returned {
                write.occurrence = WriteOccurrence::Must;
            }
        } else {
            write.proof = WriteProof::OpenGraph;
            write.address = WriteValue::UNKNOWN;
            write.value = WriteValue::UNKNOWN;
        }
    }
    if !out.closed_graph {
        for reload in out.reloads.iter_mut() {
            c.checkpoint(1)?;
            reload.value = WriteValue::UNKNOWN;
        }
    }
    tail::compose(facts, &mut out, summaries, memory, c)?;
    Ok(out)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CallWrite {
    pub callee: u64,
    pub source: FunctionWrite,
    pub write: FunctionWrite,
}
pub struct CallWrites<'m> {
    pub writes: AdmittedVec<'m, CallWrite>,
    pub barriers: AdmittedVec<'m, WriteBarrier>,
}

fn substitute(
    value: WriteValue,
    arguments: &[WriteValue; 8],
    callee: u64,
    stack: WriteValue,
) -> WriteValue {
    let argument = |word: u8| {
        arguments
            .get(usize::from(word))
            .copied()
            .unwrap_or(WriteValue::UNKNOWN)
    };
    match value {
        WriteValue::Base {
            base:
                WriteBase::Stack {
                    function,
                    offset: at,
                },
            offset,
        } if function == callee => {
            // Stack coordinates are scoped to the callee. Only its actual
            // entry sp translates them into the caller's coordinates.
            match stack {
                WriteValue::Base {
                    base:
                        WriteBase::Stack {
                            function,
                            offset: sp,
                        },
                    offset: 0,
                } => sp
                    .checked_add(at)
                    .map_or(WriteValue::UNKNOWN, |at| WriteValue::Base {
                        base: WriteBase::Stack {
                            function,
                            offset: at,
                        },
                        offset,
                    }),
                _ => WriteValue::UNKNOWN,
            }
        }
        WriteValue::Base {
            base: WriteBase::Argument(word),
            offset,
        } => argument(word).add(offset),
        WriteValue::Field {
            function,
            read,
            base: WriteBase::Argument(word),
            displacement,
            width,
            signed,
            offset,
        } => {
            let (base, displacement) = match argument(word) {
                WriteValue::Base { base, offset } => (base, offset.wrapping_add(displacement)),
                _ => (WriteBase::Unknown, displacement),
            };
            WriteValue::Field {
                function,
                read,
                base,
                displacement,
                width,
                signed,
                offset,
            }
        }
        WriteValue::Field {
            function,
            read,
            base: base @ WriteBase::Stack { .. },
            displacement,
            width,
            signed,
            offset,
        } => {
            let base = match substitute(WriteValue::base(base), arguments, callee, stack) {
                WriteValue::Base { base, offset: 0 } => base,
                _ => WriteBase::Unknown,
            };
            WriteValue::Field {
                function,
                read,
                base,
                displacement,
                width,
                signed,
                offset,
            }
        }
        value => value,
    }
}

fn find_summary<'a, 'm>(
    target: Option<u64>,
    summaries: &[&'a FunctionWrites<'m>],
    c: &mut dyn RunControl,
) -> Result<Option<&'a FunctionWrites<'m>>> {
    let mut found = None;
    for candidate in summaries {
        c.checkpoint(1)?;
        if target == Some(candidate.entry) {
            if found.is_some() {
                return Err(Error::new(
                    ErrorCode::Integrity,
                    "duplicate callee write summary",
                ));
            }
            found = Some(*candidate);
        }
    }
    Ok(found)
}

/// Apply only the targets in the caller's Transfer fact. Missing targets,
/// missing summaries and missing ABI/arguments retain unknown effects. A union
/// of multiple targets deliberately downgrades every event to May.
#[allow(clippy::too_many_arguments)]
pub fn call_writes<'m>(
    caller: &FunctionIndex<'_, '_>,
    entry: u64,
    site: u64,
    abi: Option<CallAbi>,
    summaries: &[&FunctionWrites<'_>],
    memory: &'m WorkingMemory,
    c: &mut dyn RunControl,
) -> Result<CallWrites<'m>> {
    let mut out = CallWrites {
        writes: AdmittedVec::new(memory),
        barriers: AdmittedVec::new(memory),
    };
    for record in caller.records() {
        c.checkpoint(1)?;
        if !closes_local_graph(record) {
            out.barriers.push(
                WriteBarrier {
                    origin: WriteOrigin::local(entry),
                    site,
                    kind: WriteBarrierKind::OpenGraph,
                },
                c.position(),
            )?;
            return Ok(out);
        }
    }
    let mut targets = AdmittedVec::new(memory);
    if let Some(transfer) = caller.transfer(site, c)?
        && transfer.call
    {
        match transfer.target {
            AbstractValue::ImageAddress { address } => {
                targets.push(Some(*address), c.position())?
            }
            AbstractValue::Alternatives { values } => {
                for value in values.values() {
                    c.checkpoint(1)?;
                    targets.push(
                        if let ValueAlternative::ImageAddress { address } = value {
                            Some(*address)
                        } else {
                            None
                        },
                        c.position(),
                    )?;
                }
            }
            _ => targets.push(None, c.position())?,
        }
    }
    if targets.is_empty() {
        targets.push(None, c.position())?;
    }
    let mut values = Values::new(entry, abi, true, memory);
    values.prepare(caller, c)?;
    let inputs = caller.call_inputs(site, c)?;
    let mut arguments = [WriteValue::UNKNOWN; 8];
    if let (Some(abi), Some(inputs)) = (abi, inputs.as_ref()) {
        for (argument, register) in arguments.iter_mut().zip(abi.arguments()) {
            c.checkpoint(1)?;
            if let Some(value) = inputs.registers.get(usize::from(register)) {
                *argument = values.value(value);
            }
        }
    }
    let stack = inputs
        .as_ref()
        .and_then(|i| i.registers.get(2))
        .map_or(WriteValue::UNKNOWN, |v| values.value(v));
    for target in targets.iter() {
        c.checkpoint(1)?;
        let Some(summary) = find_summary(target.map(u64::from), summaries, c)? else {
            out.barriers.push(
                WriteBarrier {
                    origin: WriteOrigin::local(entry),
                    site,
                    kind: WriteBarrierKind::UnknownCallee,
                },
                c.position(),
            )?;
            continue;
        };
        if abi.is_none() || abi != summary.abi {
            out.barriers.push(
                WriteBarrier {
                    origin: WriteOrigin::local(entry),
                    site,
                    kind: WriteBarrierKind::UnknownAbi,
                },
                c.position(),
            )?;
            continue;
        }
        if !summary.closed_graph {
            out.barriers.push(
                WriteBarrier {
                    origin: WriteOrigin::local(summary.entry),
                    site,
                    kind: WriteBarrierKind::OpenGraph,
                },
                c.position(),
            )?;
        }
        let context = summary
            .context
            .as_ref()
            .filter(|proof| proof.context.applies(summary.entry, abi, &arguments));
        for write in summary.writes.iter() {
            c.checkpoint(1)?;
            if context.is_some_and(|proof| proof.excludes(write.origin.function, write.site)) {
                continue;
            }
            let source = *write;
            let write = FunctionWrite {
                address: substitute(write.address, &arguments, summary.entry, stack),
                value: substitute(write.value, &arguments, summary.entry, stack),
                occurrence: if targets.len() == 1 {
                    write.occurrence
                } else {
                    WriteOccurrence::May
                },
                ..*write
            };
            if write.address.opaque() || write.value.opaque() {
                out.barriers.push(
                    WriteBarrier {
                        origin: write.origin,
                        site: write.site,
                        kind: if write.address.opaque() {
                            WriteBarrierKind::UnknownStore
                        } else {
                            WriteBarrierKind::UnknownValue
                        },
                    },
                    c.position(),
                )?;
            }
            out.writes.push(
                CallWrite {
                    callee: summary.entry,
                    source,
                    write,
                },
                c.position(),
            )?;
        }
        for barrier in summary.barriers.iter() {
            c.checkpoint(1)?;
            if context.is_some_and(|proof| proof.excludes(barrier.origin.function, barrier.site)) {
                continue;
            }
            out.barriers.push(*barrier, c.position())?;
        }
    }
    Ok(out)
}

/// A half-open range in one RV32 entry-stack coordinate system. Restricting
/// coordinates to the signed RV32 domain also excludes modular aliases and
/// arithmetic overflow; adding the same actual sp preserves disjointness.
fn stack_range(value: WriteValue, function: u64, width: u8) -> Option<(i64, i64)> {
    let WriteValue::Base {
        base: WriteBase::Stack {
            function: owner,
            offset,
        },
        offset: delta,
    } = value
    else {
        return None;
    };
    if owner != function || width == 0 {
        return None;
    }
    let start = offset.checked_add(i64::from(delta as i32))?;
    let end = start.checked_add(i64::from(width))?;
    (start >= i64::from(i32::MIN) && end <= i64::from(i32::MAX)).then_some((start, end))
}

impl CallWrites<'_> {
    fn unknown_addresses(&self, c: &mut dyn RunControl) -> Result<bool> {
        for barrier in self.barriers.iter() {
            c.checkpoint(1)?;
            if barrier.kind != WriteBarrierKind::UnknownValue {
                return Ok(true);
            }
        }
        Ok(false)
    }
    /// All effects, including callee stack stores, must be known. Absolute,
    /// symbolic, foreign-stack and wrapping addresses cannot prove exclusion
    /// from a caller stack whose absolute address is unknown. Store values
    /// and Must/May never establish exclusion.
    pub fn preserves_stack_word(
        &self,
        function: u64,
        slot: i64,
        c: &mut dyn RunControl,
    ) -> Result<bool> {
        if self.unknown_addresses(c)? {
            return Ok(false);
        }
        let Some((lo, hi)) = stack_range(
            WriteValue::base(WriteBase::Stack {
                function,
                offset: slot,
            }),
            function,
            4,
        ) else {
            return Ok(false);
        };
        for write in self.writes.iter() {
            c.checkpoint(1)?;
            if write.write.proof == WriteProof::OpenGraph {
                return Ok(false);
            }
            let Some((start, end)) = stack_range(write.write.address, function, write.write.width)
            else {
                return Ok(false);
            };
            if start < hi && lo < end {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

/// Prepared applications of existing write summaries, scoped to one caller.
/// Private entries prevent callers from asserting preservation by PC alone.
/// Re-analysis must match the proved target and exact argument/sp inputs before
/// using an application. The value evaluator admits these applications only
/// when the current graph's shared closure check succeeds, independently of
/// the graph used to prepare them. No field contents are cached.
pub struct StackCalls<'m> {
    entry: u64,
    calls: AdmittedVec<'m, StackCall<'m>>,
    words: AdmittedVec<'m, (u64, CallWord)>,
}
impl std::fmt::Debug for StackCalls<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StackCalls")
            .field("entry", &self.entry)
            .field("calls", &self.calls.len())
            .finish()
    }
}
struct StackCall<'m> {
    site: u64,
    target: AbstractValue,
    _target_memory: MemoryReservation<'m>,
    inputs: [AbstractValue; 9],
    writes: CallWrites<'m>,
}
impl StackCalls<'_> {
    pub(crate) fn word(
        &self,
        entry: u64,
        site: u64,
        target: &AbstractValue,
        arguments: &[WriteValue; 8],
        c: &mut dyn RunControl,
    ) -> Result<Option<CallWord>> {
        if entry != self.entry {
            return Ok(None);
        }
        for (at, word) in self.words.iter() {
            c.checkpoint(1)?;
            if *at == site
                && matches!(target, AbstractValue::ImageAddress { address } if u64::from(*address) == word.callee)
                && word
                    .arguments
                    .iter()
                    .zip(arguments)
                    .all(|(expected, actual)| expected.is_none_or(|value| value == *actual))
            {
                return Ok(Some(*word));
            }
        }
        Ok(None)
    }
    pub(crate) fn preserves(
        &self,
        entry: u64,
        site: u64,
        target: &AbstractValue,
        input: impl Fn(u8) -> AbstractValue,
        slot: i64,
        c: &mut dyn RunControl,
    ) -> Result<bool> {
        if entry != self.entry {
            return Ok(false);
        }
        for call in self.calls.iter() {
            c.checkpoint(1)?;
            if call.site != site || &call.target != target {
                continue;
            }
            for (expected, register) in call.inputs.iter().zip([2, 10, 11, 12, 13, 14, 15, 16, 17])
            {
                c.checkpoint(1)?;
                if *expected != AbstractValue::Unknown && *expected != input(register) {
                    return Ok(false);
                }
            }
            return call.writes.preserves_stack_word(entry, slot, c);
        }
        Ok(false)
    }
}

/// Opt-in preparation from a closed caller's canonical FunctionIndex. Every
/// target needs a complete summary; calls inside a summary remain barriers.
/// Opaque stored contents do not prevent exclusion of a known byte range.
/// All other barriers retain their existing conservative meaning.
pub fn stack_calls<'m>(
    caller: &FunctionIndex<'_, '_>,
    entry: u64,
    abi: Option<CallAbi>,
    summaries: &[&FunctionWrites<'_>],
    memory: &'m WorkingMemory,
    c: &mut dyn RunControl,
) -> Result<StackCalls<'m>> {
    let mut out = StackCalls {
        entry,
        calls: AdmittedVec::new(memory),
        words: AdmittedVec::new(memory),
    };
    for record in caller.records() {
        c.checkpoint(1)?;
        let FunctionRecord::Transfer {
            offset: site,
            target,
            call: true,
        } = record
        else {
            continue;
        };
        if let AbstractValue::ImageAddress { address } = target
            && let Some(summary) = find_summary(Some(u64::from(*address)), summaries, c)?
            && let Ok(word) = call_word(caller, entry, *site, abi, summary, memory, c)?
        {
            out.words.push((*site, word), c.position())?;
        }
        let writes = call_writes(caller, entry, *site, abi, summaries, memory, c)?;
        if writes.unknown_addresses(c)? {
            continue;
        }
        let inputs = caller.call_inputs(*site, c)?;
        // Values can normalize an expression (e.g. a full-word identity mask)
        // into a Stack base. This profile revalidates exact leaves, not DAG
        // identities: never let such an unrecorded expression dependency lend
        // a reusable exclusion proof to different live arguments.
        let mut exact_addresses = true;
        for write in writes.writes.iter() {
            c.checkpoint(1)?;
            let register = match write.source.address {
                WriteValue::Base {
                    base: WriteBase::Argument(word),
                    ..
                } if word < 8 => Some(usize::from(word) + 10),
                WriteValue::Base {
                    base: WriteBase::Stack { .. },
                    ..
                } => Some(2),
                _ => None,
            };
            if let Some(register) = register {
                exact_addresses &= matches!(
                    inputs.as_ref().and_then(|i| i.registers.get(register)),
                    Some(AbstractValue::EntryStack { .. })
                );
            }
        }
        // A conditional exclusion can depend on a length even when it removes
        // EVERY store. Cache it only if the required arguments are exact live
        // leaves, so StackCall::preserves rechecks them on each re-analysis.
        for summary in summaries {
            c.checkpoint(1)?;
            let Some(proof) = summary.context.as_ref() else {
                continue;
            };
            let targeted = match target {
                AbstractValue::ImageAddress { address } => u64::from(*address) == summary.entry,
                AbstractValue::Alternatives { values } => values.values().iter().any(|value| matches!(value, ValueAlternative::ImageAddress { address } if u64::from(*address) == summary.entry)),
                _ => false,
            };
            if !targeted {
                continue;
            }
            for (required, register) in proof
                .context
                .arguments
                .iter()
                .zip(proof.context.abi.arguments())
            {
                c.checkpoint(1)?;
                if required.is_some() {
                    exact_addresses &= matches!(
                        inputs
                            .as_ref()
                            .and_then(|i| i.registers.get(usize::from(register))),
                        Some(AbstractValue::Constant { .. } | AbstractValue::ImageAddress { .. })
                    );
                }
            }
        }
        if !exact_addresses {
            continue;
        }
        let inputs = [2usize, 10, 11, 12, 13, 14, 15, 16, 17].map(|register| {
            match inputs.as_ref().and_then(|i| i.registers.get(register)) {
                Some(
                    v @ (AbstractValue::Constant { .. }
                    | AbstractValue::ImageAddress { .. }
                    | AbstractValue::EntryStack { .. }),
                ) => v.clone(),
                _ => AbstractValue::Unknown,
            }
        });
        let reservation = memory.reserve(target.allocated_bytes(), c.position())?;
        out.calls.push(
            StackCall {
                site: *site,
                target: target.clone(),
                _target_memory: reservation,
                inputs,
                writes,
            },
            c.position(),
        )?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
