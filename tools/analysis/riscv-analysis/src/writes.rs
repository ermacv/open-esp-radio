//! Bounded write-event summaries over the common engine's canonical facts.
//!
//! These are observations of stores, never postconditions of memory. A field
//! denotes a value captured at its load PC, not the field's entry value. Stack
//! forwarding is exclusively the value engine's entry-stack-word proof. No
//! alias, publication, return-order or later-load proof is inferred here.
use oer_analysis_riscv_model::*;

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
    /// Values are unknown when a local incoming path is unaccounted for.
    OpenGraph,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FunctionWrite {
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
    Unsupported,
    OpenGraph,
    ExternalTransfer,
    UnknownCallee,
    UnknownAbi,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WriteBarrier {
    pub site: u64,
    pub kind: WriteBarrierKind,
}

pub struct FunctionWrites<'m> {
    pub entry: u64,
    pub abi: Option<CallAbi>,
    pub closed_graph: bool,
    pub writes: AdmittedVec<'m, FunctionWrite>,
    pub barriers: AdmittedVec<'m, WriteBarrier>,
    pub reloads: AdmittedVec<'m, StackReload>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StackReload {
    pub site: u64,
    pub slot: i64,
    pub width: u8,
    pub value: WriteValue,
}
impl FunctionWrites<'_> {
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
        } => {
            !matches!(relation, EdgeKind::Conflict | EdgeKind::Stop)
                && (target.is_some() || matches!(relation, EdgeKind::Call | EdgeKind::Return))
        }
        _ => true,
    }
}

/// No new CFG or instruction interpreter: consumes canonical expressions,
/// memory observations, decoded instruction flows and existing graph edges.
pub fn function_writes<'m>(
    facts: &FunctionIndex<'_, '_>,
    entry: u64,
    abi: Option<CallAbi>,
    memory: &'m WorkingMemory,
    c: &mut dyn RunControl,
) -> Result<FunctionWrites<'m>> {
    let mut values = Values::new(entry, abi, false, memory);
    values.prepare(facts, c)?;
    let mut out = FunctionWrites {
        entry,
        abi,
        closed_graph: true,
        writes: AdmittedVec::new(memory),
        barriers: AdmittedVec::new(memory),
        reloads: AdmittedVec::new(memory),
    };
    let mut next = entry;
    let mut straight = true;
    let mut returned = false;
    let mut register_value = None;
    for record in facts.records() {
        c.checkpoint(1)?;
        out.closed_graph &= closes_local_graph(record);
        let barrier = match record {
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
                let address = values.value(address);
                let value = value
                    .as_ref()
                    .map_or(WriteValue::UNKNOWN, |v| values.value(v));
                let value = if matches!(width, 1 | 2 | 4) {
                    value
                } else {
                    WriteValue::UNKNOWN
                };
                out.writes.push(
                    FunctionWrite {
                        site: *offset,
                        address,
                        value,
                        width: *width,
                        occurrence: WriteOccurrence::May,
                        proof: WriteProof::OpenGraph,
                    },
                    c.position(),
                )?;
                (address.opaque() || value.opaque())
                    .then_some((*offset, WriteBarrierKind::UnknownStore))
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
            out.barriers
                .push(WriteBarrier { site, kind }, c.position())?;
        }
    }
    if !out.closed_graph {
        out.barriers.push(
            WriteBarrier {
                site: entry,
                kind: WriteBarrierKind::OpenGraph,
            },
            c.position(),
        )?;
    }
    for write in out.writes.iter_mut() {
        c.checkpoint(1)?;
        if out.closed_graph {
            write.proof = WriteProof::ClosedGraph;
            if straight && returned {
                write.occurrence = WriteOccurrence::Must;
            }
        } else {
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

fn substitute(value: WriteValue, arguments: &[WriteValue; 8]) -> WriteValue {
    let argument = |word: u8| {
        arguments
            .get(usize::from(word))
            .copied()
            .unwrap_or(WriteValue::UNKNOWN)
    };
    match value {
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
        value => value,
    }
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
    if let (Some(abi), Some(inputs)) = (abi, inputs) {
        for (argument, register) in arguments.iter_mut().zip(abi.arguments()) {
            c.checkpoint(1)?;
            if let Some(value) = inputs.registers.get(usize::from(register)) {
                *argument = values.value(value);
            }
        }
    }
    for target in targets.iter() {
        c.checkpoint(1)?;
        let mut summary = None;
        for candidate in summaries {
            c.checkpoint(1)?;
            if target.map(u64::from) == Some(candidate.entry) {
                if summary.is_some() {
                    return Err(Error::new(
                        ErrorCode::Integrity,
                        "duplicate callee write summary",
                    ));
                }
                summary = Some(*candidate);
            }
        }
        let Some(summary) = summary else {
            out.barriers.push(
                WriteBarrier {
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
                    site,
                    kind: WriteBarrierKind::UnknownAbi,
                },
                c.position(),
            )?;
            continue;
        }
        for write in summary.writes.iter() {
            c.checkpoint(1)?;
            let source = *write;
            let write = FunctionWrite {
                address: substitute(write.address, &arguments),
                value: substitute(write.value, &arguments),
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
                        site: write.site,
                        kind: WriteBarrierKind::UnknownStore,
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
            out.barriers.push(*barrier, c.position())?;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
