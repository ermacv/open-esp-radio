//! Expression observations and bounded CFG bit dataflow, never physical field inference.
use crate::navigation::Facts;
use oer_analysis_blobray_domain::*;
use oer_analysis_riscv_model::*;
mod arguments;
mod domain;
mod provenance;
use arguments::ReturnedDependency;
pub use arguments::{ArgumentProgram, CallAt};
use domain::{Domain, FlowValue, Reads};
pub use provenance::StoredProvenanceEvaluator;

fn constant(v: &AbstractValue) -> Option<u32> {
    if let AbstractValue::Constant { value } = v {
        Some(*value)
    } else {
        None
    }
}
fn masked(expression: &Expression) -> Option<(&AbstractValue, u32)> {
    if let Expression::Integer {
        op: IntegerOp::And,
        left,
        right,
    } = expression
    {
        constant(right)
            .map(|mask| (left, mask))
            .or_else(|| constant(left).map(|mask| (right, mask)))
    } else {
        None
    }
}
fn width_mask(width: u8) -> Option<u32> {
    match width {
        1 => Some(0xff),
        2 => Some(0xffff),
        4 => Some(u32::MAX),
        _ => None,
    }
}
/// Recognize `(load >> shift) & mask` or `load & mask` and retain source bit positions.
/// Signed extension bits beyond the load width are excluded.
pub fn read_mask<'a>(
    facts: &Facts<'a, '_>,
    expression: &'a Expression,
) -> Option<(&'a AbstractValue, u8, RegisterMask)> {
    let (mut value, bits) = masked(expression)?;
    let mut shift = 0;
    if let Some(Expression::Integer {
        op: IntegerOp::Shr | IntegerOp::Sar,
        left,
        right,
    }) = facts.value_expression(value)
    {
        shift = constant(right)? & 31;
        value = left;
    }
    if let Some(Expression::Load { address, width, .. }) = facts.value_expression(value) {
        let bits = bits.wrapping_shl(shift) & width_mask(*width)?;
        (bits != 0).then_some((
            address,
            *width,
            RegisterMask {
                kind: RegisterMaskKind::ReadSelection,
                bits,
            },
        ))
    } else {
        None
    }
}

/// A read-modify-write replaces every stored bit except those copied
/// identically from the same word. A write without any preserved bit has no
/// replacement mask; its stored provenance still describes the whole write.
pub fn write_mask(width: u8, stored: &[StoredBits]) -> Option<RegisterMask> {
    let mask = width_mask(width)?;
    let preserved = stored.iter().fold(0, |bits, run| {
        if matches!(run.source, StoredBitsSource::Load { low, same_word: true, .. } if low == run.low)
        {
            bits | (u32::MAX >> (32 - u32::from(run.width))) << run.low
        } else {
            bits
        }
    }) & mask;
    (preserved != 0).then_some(RegisterMask {
        kind: RegisterMaskKind::WriteReplacement,
        bits: mask & !preserved,
    })
}

/// One load or entry-register source's bit dependencies in a register or stack byte.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
struct ReadDependency {
    source: usize,
    exact: [u32; 32],
    may: [u32; 32],
    inverted: u32,
}

impl ReadDependency {
    fn empty(source: usize) -> Self {
        Self {
            source,
            exact: [0; 32],
            may: [0; 32],
            inverted: 0,
        }
    }
    fn selected(&self, outputs: u32) -> (u32, u32) {
        let (mut exact, mut may) = (0, 0);
        for (i, (&e, &m)) in self.exact.iter().zip(&self.may).enumerate() {
            if outputs & (1 << i) != 0 {
                exact |= e;
                may |= m;
            }
        }
        (exact & !may, may)
    }
    fn active(&self) -> bool {
        self.exact.iter().chain(&self.may).any(|bits| *bits != 0)
    }
    fn wire(&self, mapping: impl Fn(usize) -> Option<usize>) -> Self {
        let mut result = Self::empty(self.source);
        for (i, (exact, may)) in result.exact.iter_mut().zip(&mut result.may).enumerate() {
            if let Some(from) = mapping(i) {
                *exact = self.exact.get(from).copied().unwrap_or(0);
                *may = self.may.get(from).copied().unwrap_or(0);
                if self.inverted & (1u32.wrapping_shl(from as u32)) != 0 {
                    result.inverted |= 1 << i;
                }
            }
        }
        result
    }
    fn join(self, other: Self) -> Self {
        if self == other {
            return self;
        }
        let mut result = Self::empty(self.source);
        for (i, ((exact, may), ((a, b), (x, y)))) in result
            .exact
            .iter_mut()
            .zip(&mut result.may)
            .zip(
                self.exact
                    .iter()
                    .zip(&self.may)
                    .zip(other.exact.iter().zip(&other.may)),
            )
            .enumerate()
        {
            let bit = 1 << i;
            if a == x && b == y && (self.inverted ^ other.inverted) & bit == 0 {
                *exact = *a;
                *may = *b;
                result.inverted |= self.inverted & bit;
            } else {
                *may = a | b | x | y;
            }
        }
        result
    }
}

#[derive(Clone, Copy)]
struct ReadSet {
    start: usize,
    end: usize,
    hash: u64,
    next: usize,
}

/// Immutable, interned sets. Every backing allocation is admitted before
/// growth; ids, not cloned maps, are kept in the CFG states.
struct ReadSets<'m, T> {
    entries: AdmittedVec<'m, T>,
    sets: AdmittedVec<'m, ReadSet>,
    buckets: AdmittedVec<'m, usize>,
    scratch: AdmittedVec<'m, T>,
}

fn read_integrity() -> Error {
    Error::new(ErrorCode::Integrity, "invalid read-flow graph or set id")
}

impl<'m, T: Copy + Eq + std::hash::Hash> ReadSets<'m, T> {
    fn new(memory: &'m WorkingMemory) -> Self {
        Self {
            entries: AdmittedVec::new(memory),
            sets: AdmittedVec::new(memory),
            buckets: AdmittedVec::new(memory),
            scratch: AdmittedVec::new(memory),
        }
    }
    fn values(&self, id: usize) -> Result<&[T]> {
        if id == 0 {
            return Ok(&[]);
        }
        let set = self.sets.get(id - 1).ok_or_else(read_integrity)?;
        self.entries
            .get(set.start..set.end)
            .ok_or_else(read_integrity)
    }
    fn intern(&mut self, c: &mut dyn RunControl) -> Result<usize> {
        use std::hash::{Hash, Hasher};
        if self.scratch.is_empty() {
            return Ok(0);
        }
        c.checkpoint(self.scratch.len() as u64)?;
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.scratch.hash(&mut hasher);
        let hash = hasher.finish();
        if self.buckets.is_empty() {
            for _ in 0..64 {
                self.buckets.push(0, c.position())?;
            }
        }
        let bucket = hash as usize % self.buckets.len();
        let mut id = self
            .buckets
            .get(bucket)
            .copied()
            .ok_or_else(read_integrity)?;
        while id != 0 {
            let set = self.sets.get(id - 1).copied().ok_or_else(read_integrity)?;
            c.checkpoint((set.end - set.start) as u64 + 1)?;
            if set.hash == hash && self.values(id)? == &self.scratch[..] {
                self.scratch.truncate(0);
                return Ok(id);
            }
            id = set.next;
        }
        let start = self.entries.len();
        for value in self.scratch.iter().copied() {
            self.entries.push(value, c.position())?;
        }
        let next = self
            .buckets
            .get(bucket)
            .copied()
            .ok_or_else(read_integrity)?;
        self.sets.push(
            ReadSet {
                start,
                end: self.entries.len(),
                hash,
                next,
            },
            c.position(),
        )?;
        id = self.sets.len();
        *self.buckets.get_mut(bucket).ok_or_else(read_integrity)? = id;
        self.scratch.truncate(0);
        if self.sets.len() > self.buckets.len() * 2 {
            let old = self.buckets.len();
            c.checkpoint((old * 2 + self.sets.len()) as u64)?;
            for _ in old..old * 2 {
                self.buckets.push(0, c.position())?;
            }
            self.buckets.fill(0);
            for (i, set) in self.sets.iter_mut().enumerate() {
                let slot = self
                    .buckets
                    .get_mut(set.hash as usize % (old * 2))
                    .ok_or_else(read_integrity)?;
                set.next = *slot;
                *slot = i + 1;
            }
        }
        Ok(id)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
enum ReadAtom {
    #[default]
    Unknown,
    Constant(u32),
    Stack(i64),
}
impl ReadAtom {
    fn constant(self) -> Option<u32> {
        if let Self::Constant(value) = self {
            Some(value)
        } else {
            None
        }
    }
    fn from_fact(value: &AbstractValue) -> Self {
        match value {
            AbstractValue::Constant { value } | AbstractValue::ImageAddress { address: value } => {
                Self::Constant(*value)
            }
            AbstractValue::EntryStack { offset } => Self::Stack(*offset),
            _ => Self::Unknown,
        }
    }
}

/// Known value bits travel with dependencies, including values with no
/// selected load source (arguments, constants and their masked results).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
struct ReadKnown {
    zero: u32,
    one: u32,
}
impl ReadKnown {
    fn from_fact(value: &AbstractValue) -> Self {
        match value {
            AbstractValue::Constant { value } | AbstractValue::ImageAddress { address: value } => {
                Self::constant(*value)
            }
            AbstractValue::Alternatives { values } => values
                .values()
                .iter()
                .try_fold(
                    Self {
                        zero: u32::MAX,
                        one: u32::MAX,
                    },
                    |shared, value| {
                        let value = match value {
                            ValueAlternative::Constant { value }
                            | ValueAlternative::ImageAddress { address: value } => *value,
                            _ => return None,
                        };
                        Some(Self {
                            zero: shared.zero & !value,
                            one: shared.one & value,
                        })
                    },
                )
                .unwrap_or_default(),
            _ => Self::default(),
        }
    }

    fn constant(value: u32) -> Self {
        Self {
            zero: !value,
            one: value,
        }
    }
    fn not(self) -> Self {
        Self {
            zero: self.one,
            one: self.zero,
        }
    }
    fn constant_value(self) -> Option<u32> {
        (self.zero | self.one == u32::MAX).then_some(self.one)
    }
    fn wire(self, mapping: impl Fn(usize) -> Option<usize>) -> Self {
        let mut result = Self::default();
        for i in 0..32 {
            let bit = 1 << i;
            match mapping(i) {
                Some(from) if from < 32 => {
                    result.zero |= ((self.zero >> from) & 1) << i;
                    result.one |= ((self.one >> from) & 1) << i;
                }
                _ => result.zero |= bit,
            }
        }
        result
    }
    fn integer(op: IntegerOp, a: Self, b: Self) -> Self {
        use IntegerOp::*;
        match op {
            AndNot => Self::integer(And, a, b.not()),
            OrNot => Self::integer(Or, a, b.not()),
            XorNot => Self::integer(Xor, a, b.not()),
            And => Self {
                zero: a.zero | b.zero,
                one: a.one & b.one,
            },
            Or => Self {
                zero: a.zero & b.zero,
                one: a.one | b.one,
            },
            Xor => Self {
                zero: (a.zero & b.zero) | (a.one & b.one),
                one: (a.zero & b.one) | (a.one & b.zero),
            },
            Add | Sub if b.constant_value() == Some(0) => a,
            Add if a.constant_value() == Some(0) => b,
            Shl | Shr | Sar | RotateLeft | RotateRight | BitExtract
                if b.constant_value().is_some() =>
            {
                let by = b.one as usize & 31;
                a.wire(|i| match op {
                    Shl => i.checked_sub(by),
                    Shr => (i + by < 32).then_some(i + by),
                    Sar => Some((i + by).min(31)),
                    RotateLeft => Some((i + 32 - by) & 31),
                    RotateRight => Some((i + by) & 31),
                    BitExtract => (i == 0).then_some(by),
                    _ => None,
                })
            }
            SignExtendByte | SignExtendHalf | ByteReverse => a.wire(|i| {
                Some(match op {
                    SignExtendByte => i.min(7),
                    SignExtendHalf => i.min(15),
                    _ => (3 - i / 8) * 8 + i % 8,
                })
            }),
            _ => Self::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
struct ReadCell<F: FlowValue> {
    flow: F,
    atom: ReadAtom,
    known: ReadKnown,
}
impl<F: FlowValue> ReadCell<F> {
    fn constant(value: u32) -> Self {
        Self {
            flow: F::constant(value),
            atom: ReadAtom::Constant(value),
            known: ReadKnown::constant(value),
        }
    }
}
#[derive(Clone, Copy, Eq, PartialEq, Hash)]
struct ReadSlot<B> {
    offset: i64,
    cell: B,
}
#[derive(Clone, Copy, Eq, PartialEq)]
struct ReadState<F: FlowValue> {
    registers: [ReadCell<F>; 32],
    slots: usize,
}
impl<F: FlowValue> ReadState<F> {
    fn entry() -> Self {
        let mut state = Self {
            registers: [ReadCell::default(); 32],
            slots: 0,
        };
        state.set(0, ReadCell::constant(0));
        state.set(
            2,
            ReadCell {
                flow: F::default(),
                atom: ReadAtom::Stack(0),
                known: ReadKnown::default(),
            },
        );
        state
    }
    fn register(&self, register: u8) -> ReadCell<F> {
        self.registers
            .get(usize::from(register))
            .copied()
            .unwrap_or_default()
    }
    fn operand(&self, operand: Operand) -> ReadCell<F> {
        match operand {
            Operand::Register(register) => self.register(register),
            Operand::Immediate(value) => ReadCell::constant(value),
        }
    }
    fn set(&mut self, register: u8, value: ReadCell<F>) {
        if let Some(slot) = self.registers.get_mut(usize::from(register)) {
            *slot = value;
        }
    }
}

#[derive(Clone, Copy)]
struct ReadInstruction<'a> {
    site: u64,
    opaque: u32,
    operation: SemanticOp,
    branch: Option<(BranchTest, Operand, Operand)>,
    flow: InstructionFlow,
    address: Option<&'a AbstractValue>,
    value: Option<(u8, ReadAtom, ReadKnown)>,
    address_atom: ReadAtom,
    source: Option<usize>,
    arguments: bool,
    call: bool,
    dispatch: bool,
    edge_start: usize,
    edge_end: usize,
}
struct ReadNode<'a, F: FlowValue> {
    instruction: ReadInstruction<'a>,
    input: Option<ReadState<F>>,
    queued: bool,
    unsettled: bool,
}
#[derive(Clone, Copy)]
struct ReadEdge {
    from: usize,
    target: Option<usize>,
    relation: EdgeKind,
}
enum ReadOrigin<'a> {
    Load {
        site: u64,
        address: &'a AbstractValue,
        width: u8,
    },
    Entry(u8),
}
struct ReadSource<'a> {
    origin: ReadOrigin<'a>,
    preserved: u32,
    limits: [u32; 4],
    event: usize,
}
struct ReadEvent {
    usage: ReadUse,
    next: usize,
}
#[derive(Clone, Copy)]
struct ReadDispatch {
    producer: usize,
    register: u8,
    site: u64,
}

#[derive(Clone, Copy)]
enum ReadFlowOp {
    Join,
    Assemble,
    Integer(IntegerOp, ReadKnown, ReadKnown),
}

/// Bound pathological CFGs without spending the whole operation's budget on
/// one function. Every admitted step is still charged to the caller.
const READ_MIN_FUNCTION_WORK: u64 = 100_000;
const READ_INSTRUCTION_WORK: u64 = 512;
fn read_work_limit(instructions: usize) -> u64 {
    READ_MIN_FUNCTION_WORK.max((instructions as u64).saturating_mul(READ_INSTRUCTION_WORK))
}
struct ReadWork<'c> {
    caller: &'c mut dyn RunControl,
    remaining: u64,
}
impl RunControl for ReadWork<'_> {
    fn checkpoint(&mut self, units: u64) -> Result<()> {
        self.remaining = self.remaining.checked_sub(units).ok_or_else(|| {
            Error::new(
                ErrorCode::ResourceLimited,
                "read-use function work budget exhausted",
            )
        })?;
        self.caller.checkpoint(units)
    }
    fn position(&self) -> RunPosition {
        self.caller.position()
    }
    fn set_position(&mut self, position: RunPosition) {
        self.caller.set_position(position);
    }
}

type SelectRead<'s> = dyn FnMut(&AbstractValue, u8, &mut dyn RunControl) -> Result<bool> + 's;

/// One instruction CFG interpreter, parameterized by register and stack-byte
/// contents. Both passes share preparation, traversal, transfers, call-summary
/// application, work budgets and admitted state/queue storage.
struct FlowEngine<'f, 'a, 'm, D: Domain<'m>> {
    facts: &'f Facts<'a, 'm>,
    nodes: AdmittedVec<'m, ReadNode<'a, D::Flow>>,
    edges: AdmittedVec<'m, ReadEdge>,
    domain: D,
    stacks: ReadSets<'m, ReadSlot<D::Byte>>,
    sources: AdmittedVec<'m, ReadSource<'a>>,
    dispatches: AdmittedVec<'m, ReadDispatch>,
    queue: AdmittedVec<'m, usize>,
    ready: bool,
    work_limit: bool,
    summary_mode: bool,
    prepared: bool,
    converged: bool,
    pending: Option<usize>,
}
impl<'f, 'a, 'm, D: Domain<'m>> FlowEngine<'f, 'a, 'm, D> {
    pub fn new(facts: &'f Facts<'a, 'm>, memory: &'m WorkingMemory) -> Self {
        Self {
            facts,
            nodes: AdmittedVec::new(memory),
            edges: AdmittedVec::new(memory),
            domain: D::new(memory),
            stacks: ReadSets::new(memory),
            sources: AdmittedVec::new(memory),
            dispatches: AdmittedVec::new(memory),
            queue: AdmittedVec::new(memory),
            ready: false,
            work_limit: false,
            summary_mode: false,
            prepared: false,
            converged: false,
            pending: None,
        }
    }
    pub fn analyze(
        &mut self,
        abi: Option<CallAbi>,
        decoder: &dyn FunctionSemantics,
        select: &mut SelectRead<'_>,
        calls: &CallAt<'_>,
        c: &mut dyn RunControl,
    ) -> Result<()> {
        if self.ready {
            return Ok(());
        }
        self.ready = true;
        let result = (|| {
            let records = self.facts.records();
            c.checkpoint(records.len() as u64)?;
            let instructions = records
                .iter()
                .filter(|record| matches!(record, FunctionRecord::Instruction { .. }))
                .count();
            let mut work = ReadWork {
                caller: c,
                remaining: read_work_limit(instructions),
            };
            self.prepare(decoder, select, &mut work)?;
            self.solve(abi, calls, &mut work)
        })();
        match result {
            Err(error)
                if matches!(error.code, ErrorCode::ResourceLimited | ErrorCode::TimedOut) =>
            {
                self.work_limit = true;
                if D::RECOVER
                    && !self.converged
                    && let Err(error) = self.invalidate_pending(c)
                {
                    if !matches!(error.code, ErrorCode::ResourceLimited | ErrorCode::TimedOut) {
                        return Err(error);
                    }
                    for node in self.nodes.iter_mut() {
                        node.unsettled = true;
                    }
                }
                Ok(())
            }
            result => result,
        }
    }
    fn invalidate_pending(&mut self, c: &mut dyn RunControl) -> Result<()> {
        if !self.prepared {
            for node in self.nodes.iter_mut() {
                node.unsettled = true;
            }
            return Ok(());
        }
        self.queue.truncate(0);
        for (id, node) in self.nodes.iter_mut().enumerate() {
            if node.queued || self.pending == Some(id) {
                node.unsettled = true;
                self.queue.push(id, c.position())?;
            }
        }
        let mut cursor = 0;
        while let Some(id) = self.queue.get(cursor).copied() {
            c.checkpoint(1)?;
            cursor += 1;
            let instruction = self.node(id)?.instruction;
            for index in instruction.edge_start..instruction.edge_end {
                let target = self.edges.get(index).ok_or_else(read_integrity)?.target;
                if let Some(target) = target
                    && !self.node(target)?.unsettled
                {
                    self.node_mut(target)?.unsettled = true;
                    self.queue.push(target, c.position())?;
                }
            }
        }
        Ok(())
    }
    fn node(&self, id: usize) -> Result<&ReadNode<'a, D::Flow>> {
        self.nodes.get(id).ok_or_else(read_integrity)
    }
    fn node_mut(&mut self, id: usize) -> Result<&mut ReadNode<'a, D::Flow>> {
        self.nodes.get_mut(id).ok_or_else(read_integrity)
    }
    fn at(&self, site: u64) -> Option<usize> {
        self.nodes
            .binary_search_by_key(&site, |node| node.instruction.site)
            .ok()
    }
    fn source_at(&self, site: u64) -> Option<usize> {
        self.sources
            .binary_search_by_key(&site, |source| match source.origin {
                ReadOrigin::Load { site, .. } => site,
                ReadOrigin::Entry(_) => u64::MAX,
            })
            .ok()
    }
    fn prepare(
        &mut self,
        decoder: &dyn FunctionSemantics,
        select: &mut SelectRead<'_>,
        c: &mut dyn RunControl,
    ) -> Result<()> {
        let records = self.facts.records();
        for record in records {
            c.checkpoint(1)?;
            match record {
                FunctionRecord::Instruction {
                    offset,
                    bytes,
                    decoded,
                } => self.nodes.push(
                    ReadNode {
                        instruction: ReadInstruction {
                            site: *offset,
                            opaque: opaque_read_registers(bytes, decoder.lift(bytes)),
                            operation: decoder.lift(bytes),
                            branch: decoder.branch(bytes),
                            flow: decoded.flow,
                            address: None,
                            value: None,
                            address_atom: ReadAtom::Unknown,
                            source: None,
                            arguments: false,
                            call: false,
                            dispatch: false,
                            edge_start: 0,
                            edge_end: 0,
                        },
                        input: None,
                        queued: false,
                        unsettled: false,
                    },
                    c.position(),
                )?,
                FunctionRecord::MemoryAccess {
                    offset,
                    access:
                        MemoryKind::Load
                        | MemoryKind::LoadReserved
                        | MemoryKind::Atomic
                        | MemoryKind::FloatLoad,
                    address,
                    width,
                    ..
                } if D::ALL_LOADS || select(address, *width, c)? => self.sources.push(
                    ReadSource {
                        origin: ReadOrigin::Load {
                            site: *offset,
                            address,
                            width: *width,
                        },
                        preserved: 0,
                        limits: [0; 4],
                        event: 0,
                    },
                    c.position(),
                )?,
                _ => {}
            }
        }
        c.checkpoint(
            (records.len() as u64).saturating_mul(u64::from(records.len().max(1).ilog2() + 1)),
        )?;
        self.nodes
            .sort_unstable_by_key(|node| node.instruction.site);
        self.sources
            .sort_unstable_by_key(|source| match source.origin {
                ReadOrigin::Load { site, .. } => site,
                ReadOrigin::Entry(_) => u64::MAX,
            });
        for record in records {
            c.checkpoint(1)?;
            match record {
                FunctionRecord::MemoryAccess {
                    offset, address, ..
                } => {
                    if let Some(id) = self.at(*offset) {
                        let source = self.source_at(*offset);
                        let instruction = &mut self.node_mut(id)?.instruction;
                        instruction.address = Some(address);
                        instruction.address_atom = ReadAtom::from_fact(address);
                        instruction.source = source;
                    }
                }
                FunctionRecord::Value {
                    offset,
                    register,
                    value,
                    ..
                } => {
                    if let Some(id) = self.at(*offset) {
                        self.node_mut(id)?.instruction.value =
                            Some((*register, ReadAtom::from_fact(value), D::fact_known(value)));
                    }
                }
                FunctionRecord::CallInputs { offset, .. } => {
                    if let Some(id) = self.at(*offset) {
                        self.node_mut(id)?.instruction.arguments = true;
                    }
                }
                FunctionRecord::Edge {
                    from,
                    target,
                    relation,
                    external,
                } => {
                    if let Some(from) = self.at(*from) {
                        if *relation == EdgeKind::Call {
                            self.node_mut(from)?.instruction.call = true;
                        }
                        let target = target
                            .filter(|_| !external && *relation != EdgeKind::Call)
                            .and_then(|site| self.at(site));
                        self.edges.push(
                            ReadEdge {
                                from,
                                target,
                                relation: *relation,
                            },
                            c.position(),
                        )?;
                    }
                }
                FunctionRecord::Dispatch { offset, index, .. } => {
                    if let Some(id) = self.at(*offset) {
                        self.node_mut(id)?.instruction.dispatch = true;
                    }
                    // The value fact locates the physical producer of the
                    // dispatch index; its dependencies come from the CFG state.
                    for producer in records {
                        c.checkpoint(1)?;
                        if let FunctionRecord::Value {
                            offset: producer,
                            register,
                            value,
                            ..
                        } = producer
                            && value == index
                            && !matches!(value, AbstractValue::Unknown)
                            && let Some(producer) = self.at(*producer)
                        {
                            self.dispatches.push(
                                ReadDispatch {
                                    producer,
                                    register: *register,
                                    site: *offset,
                                },
                                c.position(),
                            )?;
                        }
                    }
                }
                _ => {}
            }
        }
        c.checkpoint(
            (self.edges.len() as u64)
                .saturating_mul(u64::from(self.edges.len().max(1).ilog2() + 1)),
        )?;
        self.edges.sort_unstable_by_key(|edge| edge.from);
        for (id, node) in self.nodes.iter_mut().enumerate() {
            node.instruction.edge_start = self.edges.partition_point(|edge| edge.from < id);
            node.instruction.edge_end = self.edges.partition_point(|edge| edge.from <= id);
        }
        self.prepared = true;
        Ok(())
    }
    fn integer(
        &mut self,
        op: IntegerOp,
        left: ReadCell<D::Flow>,
        right: ReadCell<D::Flow>,
        c: &mut dyn RunControl,
    ) -> Result<ReadCell<D::Flow>> {
        let atom = match (op, left.atom, right.atom) {
            (_, ReadAtom::Constant(a), ReadAtom::Constant(b)) => {
                ReadAtom::Constant(op.evaluate(a, b))
            }
            (IntegerOp::Add, ReadAtom::Stack(a), ReadAtom::Constant(b))
            | (IntegerOp::Add, ReadAtom::Constant(b), ReadAtom::Stack(a)) => {
                ReadAtom::Stack(a.wrapping_add(i64::from(b as i32)))
            }
            (IntegerOp::Sub, ReadAtom::Stack(a), ReadAtom::Constant(b)) => {
                ReadAtom::Stack(a.wrapping_sub(i64::from(b as i32)))
            }
            _ => ReadAtom::Unknown,
        };
        let flow = D::integer(self, op, left, right, c)?;
        let known = atom
            .constant()
            .map(ReadKnown::constant)
            .unwrap_or_else(|| ReadKnown::integer(op, left.known, right.known));
        let flow = D::refine(flow, known);
        let known = D::known(flow, known);
        let atom = known
            .constant_value()
            .map(ReadAtom::Constant)
            .unwrap_or(atom);
        Ok(ReadCell { flow, atom, known })
    }
    fn join_cell(
        &mut self,
        a: ReadCell<D::Flow>,
        b: ReadCell<D::Flow>,
        c: &mut dyn RunControl,
    ) -> Result<ReadCell<D::Flow>> {
        Ok(ReadCell {
            flow: D::join(self, a.flow, b.flow, c)?,
            atom: if a.atom == b.atom {
                a.atom
            } else {
                ReadAtom::Unknown
            },
            known: ReadKnown {
                zero: a.known.zero & b.known.zero,
                one: a.known.one & b.known.one,
            },
        })
    }
    fn join_stacks(&mut self, a: usize, b: usize, c: &mut dyn RunControl) -> Result<usize> {
        if a == b {
            return Ok(a);
        }
        let (mut l, mut r) = (0, 0);
        loop {
            let x = self.stacks.values(a)?.get(l).copied();
            let y = self.stacks.values(b)?.get(r).copied();
            let offset = match (x, y) {
                (Some(x), Some(y)) => x.offset.min(y.offset),
                (Some(x), _) => x.offset,
                (_, Some(y)) => y.offset,
                _ => break,
            };
            c.checkpoint(1)?;
            let x = x.filter(|x| x.offset == offset);
            let y = y.filter(|y| y.offset == offset);
            if x.is_some() {
                l += 1;
            }
            if y.is_some() {
                r += 1;
            }
            let cell = self.join_cell(
                D::unpack(x.map(|x| x.cell).unwrap_or_default()),
                D::unpack(y.map(|y| y.cell).unwrap_or_default()),
                c,
            )?;
            self.stacks.scratch.push(
                ReadSlot {
                    offset,
                    cell: D::pack(cell),
                },
                c.position(),
            )?;
        }
        self.stacks.intern(c)
    }
    fn store_byte(
        &mut self,
        stack: usize,
        offset: i64,
        cell: D::Byte,
        c: &mut dyn RunControl,
    ) -> Result<usize> {
        let len = self.stacks.values(stack)?.len();
        let mut inserted = false;
        for index in 0..len {
            c.checkpoint(1)?;
            let slot = self
                .stacks
                .values(stack)?
                .get(index)
                .copied()
                .ok_or_else(read_integrity)?;
            if !inserted && slot.offset >= offset {
                self.stacks
                    .scratch
                    .push(ReadSlot { offset, cell }, c.position())?;
                inserted = true;
            }
            if slot.offset != offset {
                self.stacks.scratch.push(slot, c.position())?;
            }
        }
        if !inserted {
            self.stacks
                .scratch
                .push(ReadSlot { offset, cell }, c.position())?;
        }
        self.stacks.intern(c)
    }
    fn merge(
        &mut self,
        id: usize,
        state: ReadState<D::Flow>,
        c: &mut dyn RunControl,
    ) -> Result<()> {
        let old = self.node(id)?.input;
        let joined = if let Some(old) = old {
            if old == state {
                return Ok(());
            }
            let mut joined = old;
            for ((cell, a), b) in joined
                .registers
                .iter_mut()
                .zip(old.registers)
                .zip(state.registers)
            {
                *cell = self.join_cell(a, b, c)?;
            }
            joined.slots = self.join_stacks(old.slots, state.slots, c)?;
            if old == joined {
                return Ok(());
            }
            joined
        } else {
            state
        };
        let enqueue = !self.node(id)?.queued;
        if enqueue {
            self.queue.push(id, c.position())?;
        }
        let node = self.node_mut(id)?;
        node.input = Some(joined);
        node.queued = true;
        Ok(())
    }
    fn solve(
        &mut self,
        abi: Option<CallAbi>,
        calls: &CallAt<'_>,
        c: &mut dyn RunControl,
    ) -> Result<()> {
        if self.nodes.is_empty() {
            return Ok(());
        }
        let mut entry = ReadState::entry();
        {
            for register in 1..32 {
                if !D::entry_source(register, self.summary_mode) {
                    continue;
                }
                let source = self.sources.len();
                self.sources.push(
                    ReadSource {
                        origin: ReadOrigin::Entry(register),
                        preserved: 0,
                        limits: [0; 4],
                        event: 0,
                    },
                    c.position(),
                )?;
                entry.set(
                    register,
                    ReadCell {
                        flow: D::seed(self, source, 4, false, u32::MAX, c)?,
                        ..ReadCell::default()
                    },
                );
            }
        }
        self.merge(0, entry, c)?;
        let mut cursor = 0;
        while let Some(id) = self.queue.get(cursor).copied() {
            c.checkpoint(1)?;
            cursor += 1;
            self.pending = Some(id);
            self.node_mut(id)?.queued = false;
            let input = self.node(id)?.input.ok_or_else(read_integrity)?;
            let output = self.execute(id, input, abi, calls, false, c)?;
            let instruction = self.node(id)?.instruction;
            for index in instruction.edge_start..instruction.edge_end {
                let edge = self.edges.get(index).copied().ok_or_else(read_integrity)?;
                if let Some(target) = edge.target {
                    self.merge(target, output, c)?;
                }
            }
            self.pending = None;
            if cursor == self.queue.len() {
                self.queue.truncate(0);
                cursor = 0;
            }
        }
        self.converged = true;
        if !D::EMIT {
            return Ok(());
        }
        // Emit only at the fixed point: an earlier Exact must not survive a
        // later join which demotes the same path to May.
        for id in 0..self.nodes.len() {
            c.checkpoint(1)?;
            if let Some(input) = self.node(id)?.input {
                self.execute(id, input, abi, calls, true, c)?;
            }
        }
        Ok(())
    }
    fn execute(
        &mut self,
        id: usize,
        input: ReadState<D::Flow>,
        abi: Option<CallAbi>,
        calls: &CallAt<'_>,
        emit: bool,
        c: &mut dyn RunControl,
    ) -> Result<ReadState<D::Flow>> {
        let instruction = self.node(id)?.instruction;
        let mut output = input;
        if emit {
            if let Some((_, left, right)) = instruction.branch {
                for value in [left, right] {
                    D::sink(
                        self,
                        input.operand(value).flow,
                        u32::MAX,
                        ReadSink::Branch {
                            site: instruction.site,
                        },
                        c,
                    )?;
                }
            }
            if let InstructionFlow::Indirect { base, offset, .. } = instruction.flow {
                let value = self.integer(
                    IntegerOp::Add,
                    input.register(base),
                    ReadCell::constant(offset as u32),
                    c,
                )?;
                if !instruction.dispatch {
                    D::sink(
                        self,
                        value.flow,
                        !1,
                        ReadSink::IndirectTarget {
                            site: instruction.site,
                        },
                        c,
                    )?;
                }
            }
            if instruction.arguments {
                self.limit_slots(input.slots, ReadLimitKind::StackSpill, c)?;
                if let Some(abi) = abi {
                    self.consume_arguments(input, instruction.site, abi, calls, c)?;
                } else {
                    self.limit_state(input, ReadLimitKind::UnsupportedOperation, c)?;
                }
            }
            if matches!(instruction.flow, InstructionFlow::Return { .. }) {
                for register in CallAbi::RiscvInteger.results() {
                    D::return_value(self, input.register(register).flow, register, c)?;
                }
                self.limit_slots(input.slots, ReadLimitKind::StackSpill, c)?;
            }
        }
        // The integer model omits CSR inputs and moves into the FP file.
        // Their structural operands identify exactly which live dependencies
        // stop there; they never poison other loads in the function.
        let opaque = instruction.opaque;
        if emit {
            for register in 1..32 {
                if opaque & (1 << register) != 0 {
                    D::limit(
                        self,
                        input.register(register).flow,
                        ReadLimitKind::UnsupportedOperation,
                        c,
                    )?;
                }
            }
        }
        match instruction.operation {
            SemanticOp::Integer {
                op,
                dest,
                left,
                right,
            } => {
                let value = self.integer(op, input.operand(left), input.operand(right), c)?;
                if dest != 0 {
                    output.set(dest, value);
                }
            }
            SemanticOp::Upper {
                dest,
                value,
                pc_relative,
            } => {
                if dest != 0 {
                    output.set(
                        dest,
                        if pc_relative {
                            ReadCell::default()
                        } else {
                            ReadCell::constant(value)
                        },
                    );
                }
            }
            SemanticOp::Link { dest } | SemanticOp::Opaque { dest } => {
                if dest != 0 {
                    output.set(dest, ReadCell::default());
                }
            }
            SemanticOp::Memory {
                kind,
                base,
                displacement,
                width,
                dest,
                source,
                signed,
                ..
            } => {
                let address = self.integer(
                    IntegerOp::Add,
                    input.register(base),
                    ReadCell::constant(displacement as u32),
                    c,
                )?;
                if emit {
                    D::sink(
                        self,
                        address.flow,
                        u32::MAX,
                        ReadSink::Address {
                            site: instruction.site,
                        },
                        c,
                    )?;
                }
                let stack = match instruction.address_atom {
                    ReadAtom::Stack(offset) => Some(offset),
                    _ => {
                        if let ReadAtom::Stack(offset) = address.atom {
                            Some(offset)
                        } else {
                            None
                        }
                    }
                };
                let mask = width_mask(width).unwrap_or(u32::MAX);
                if let Some(source) = source {
                    let value = input.register(source);
                    if let Some(offset) = stack {
                        for byte in 0..usize::from(width) {
                            let flow =
                                D::map(self, value.flow, |i| (i < 8).then_some(i + byte * 8), c)?;
                            let atom = value
                                .atom
                                .constant()
                                .map(|value| ReadAtom::Constant((value >> (byte * 8)) & 255))
                                .unwrap_or_default();
                            output.slots = self.store_byte(
                                output.slots,
                                offset.wrapping_add(byte as i64),
                                D::pack(ReadCell {
                                    flow,
                                    atom,
                                    known: value.known.wire(|i| (i < 8).then_some(i + byte * 8)),
                                }),
                                c,
                            )?;
                        }
                    } else if emit && base == 2 {
                        let selected =
                            D::map(self, value.flow, |i| (mask & (1 << i) != 0).then_some(i), c)?;
                        D::limit(self, selected, ReadLimitKind::StackSpill, c)?;
                    } else if emit {
                        D::store(self, value.flow, mask, instruction, kind, width, c)?;
                    }
                }
                if matches!(kind, MemoryKind::Load | MemoryKind::LoadReserved) {
                    let mut value = ReadCell {
                        known: if signed {
                            ReadKnown::default()
                        } else {
                            ReadKnown {
                                zero: !mask,
                                one: 0,
                            }
                        },
                        ..ReadCell::default()
                    };
                    let mut forwarded = 0;
                    if let Some(offset) = stack {
                        let mut constant = Some(0u32);
                        for byte in 0..usize::from(width) {
                            let key = offset.wrapping_add(byte as i64);
                            let slots = self.stacks.values(input.slots)?;
                            let slot = slots
                                .binary_search_by_key(&key, |slot| slot.offset)
                                .ok()
                                .and_then(|i| slots.get(i));
                            if slot.is_some() {
                                forwarded |= 255 << (byte * 8);
                            }
                            let cell = D::unpack(slot.map(|slot| slot.cell).unwrap_or_default());
                            let flow = D::map(
                                self,
                                cell.flow,
                                |i| i.checked_sub(byte * 8).filter(|i| *i < 8),
                                c,
                            )?;
                            value.flow =
                                D::binary(self, value.flow, flow, ReadFlowOp::Assemble, c)?;
                            value.known.zero |= (cell.known.zero & 255) << (byte * 8);
                            value.known.one |= (cell.known.one & 255) << (byte * 8);
                            constant = constant
                                .zip(cell.atom.constant())
                                .map(|(a, b)| a | ((b & 255) << (byte * 8)));
                        }
                        if signed && width < 4 {
                            if forwarded & (1 << (width * 8 - 1)) != 0 {
                                forwarded |= !mask;
                            }
                            value.flow = D::map(
                                self,
                                value.flow,
                                |i| Some(i.min(usize::from(width) * 8 - 1)),
                                c,
                            )?;
                            value.known = value
                                .known
                                .wire(|i| Some(i.min(usize::from(width) * 8 - 1)));
                        }
                        value.atom = constant
                            .map(|v| {
                                ReadAtom::Constant(match (width, signed) {
                                    (1, true) => v as i8 as i32 as u32,
                                    (2, true) => v as i16 as i32 as u32,
                                    _ => v,
                                })
                            })
                            .unwrap_or_default();
                    }
                    if let Some(source) = instruction.source {
                        value.flow =
                            D::load(self, value.flow, source, width, signed, forwarded, c)?;
                    }
                    value.flow = D::refine(value.flow, value.known);
                    if let Some(dest) = dest
                        && dest != 0
                    {
                        output.set(dest, value);
                    }
                } else if let Some(dest) = dest
                    && dest != 0
                {
                    output.set(dest, ReadCell::default());
                }
                if emit
                    && matches!(kind, MemoryKind::Atomic | MemoryKind::FloatLoad)
                    && let Some(source) = instruction.source
                {
                    D::source_limit(self, source, mask, ReadLimitKind::UnsupportedOperation)?;
                }
            }
            SemanticOp::Unsupported => {
                output.registers = ReadState::entry().registers;
                output.set(2, ReadCell::default());
                output.slots = 0;
            }
            SemanticOp::None | SemanticOp::Fence { .. } => {}
        }
        if let Some((register, atom, known)) = instruction.value
            && register != 0
        {
            let mut value = output.register(register);
            if atom != ReadAtom::Unknown {
                value.atom = atom;
            }
            if let ReadAtom::Constant(v) = atom {
                value.known = ReadKnown::constant(v);
            }
            value.known.zero |= known.zero;
            value.known.one |= known.one;
            value.flow = D::refine(value.flow, value.known);
            output.set(register, value);
        }
        if emit {
            for index in 0..self.dispatches.len() {
                let dispatch = self
                    .dispatches
                    .get(index)
                    .copied()
                    .ok_or_else(read_integrity)?;
                if dispatch.producer == id {
                    D::sink(
                        self,
                        output.register(dispatch.register).flow,
                        u32::MAX,
                        ReadSink::Dispatch {
                            site: dispatch.site,
                        },
                        c,
                    )?;
                }
            }
            for index in instruction.edge_start..instruction.edge_end {
                let edge = self.edges.get(index).copied().ok_or_else(read_integrity)?;
                if edge.target.is_none()
                    && matches!(
                        edge.relation,
                        EdgeKind::Indirect
                            | EdgeKind::Conflict
                            | EdgeKind::Taken
                            | EdgeKind::Fallthrough
                            | EdgeKind::PossibleContinuation
                            | EdgeKind::Trap
                            | EdgeKind::Stop
                    )
                    || (edge.target.is_none()
                        && edge.relation == EdgeKind::Jump
                        && !instruction.arguments)
                {
                    self.limit_state(output, ReadLimitKind::PartialFunction, c)?;
                }
            }
        }
        if instruction.arguments && !instruction.call && emit {
            // A tail transfer returns the target's result directly to our caller.
            if let Some(summary) = calls(instruction.site) {
                for register in CallAbi::RiscvInteger.results() {
                    let flow = self.call_result(input, &summary.arguments, register, c)?;
                    D::return_value(self, flow, register, c)?;
                }
            }
        }
        if instruction.call {
            if let Some(abi) = abi {
                let mut results = [ReadCell::default(); 2];
                if let Some(summary) = calls(instruction.site) {
                    for (cell, register) in results.iter_mut().zip(abi.results()) {
                        cell.flow = self.call_result(input, &summary.arguments, register, c)?;
                        D::call_constants(cell, summary, register);
                    }
                }
                for register in abi.clobbered() {
                    output.set(register, ReadCell::default());
                }
                for (register, cell) in abi.results().into_iter().zip(results) {
                    output.set(register, cell);
                }
            } else {
                if emit {
                    self.limit_state(output, ReadLimitKind::UnsupportedOperation, c)?;
                }
                output.registers = ReadState::entry().registers;
                output.set(2, ReadCell::default());
            }
        }
        Ok(output)
    }
    fn limit_slots(
        &mut self,
        slots: usize,
        reason: ReadLimitKind,
        c: &mut dyn RunControl,
    ) -> Result<()> {
        let len = self.stacks.values(slots)?.len();
        for index in 0..len {
            let slot = self
                .stacks
                .values(slots)?
                .get(index)
                .copied()
                .ok_or_else(read_integrity)?;
            D::limit(self, D::unpack(slot.cell).flow, reason, c)?;
        }
        Ok(())
    }
    fn limit_state(
        &mut self,
        state: ReadState<D::Flow>,
        reason: ReadLimitKind,
        c: &mut dyn RunControl,
    ) -> Result<()> {
        for register in state.registers {
            D::limit(self, register.flow, reason, c)?;
        }
        self.limit_slots(state.slots, reason, c)
    }
    pub(super) fn consume_arguments(
        &mut self,
        input: ReadState<D::Flow>,
        site: u64,
        abi: CallAbi,
        calls: &CallAt<'_>,
        c: &mut dyn RunControl,
    ) -> Result<()> {
        for register in abi.arguments() {
            let consumed = calls(site)
                .map(|s| &s.arguments)
                .and_then(|s| s.consumed.get(usize::from(register - 10)).copied())
                .unwrap_or(ArgumentConsumption {
                    exact: 0,
                    may: u32::MAX,
                });
            D::consume_argument(
                self,
                input.register(register).flow,
                consumed,
                site,
                register,
                c,
            )?;
        }
        Ok(())
    }
    pub(super) fn call_result(
        &mut self,
        input: ReadState<D::Flow>,
        summary: &ArgumentSummary,
        register: u8,
        c: &mut dyn RunControl,
    ) -> Result<D::Flow> {
        let mut result = D::Flow::default();
        let mut covered = 0;
        for returned in summary.returned.iter().filter(|r| r.register == register) {
            result = D::call_mapping(
                self,
                result,
                input.register(returned.argument).flow,
                returned,
                &mut covered,
                c,
            )?;
        }
        Ok(result)
    }
}

/// Selected load uses, using the original dependency domain and work budget.
pub struct ReadUsesEvaluator<'f, 'a, 'm> {
    engine: FlowEngine<'f, 'a, 'm, Reads<'m>>,
}
impl<'f, 'a, 'm> ReadUsesEvaluator<'f, 'a, 'm> {
    pub fn new(facts: &'f Facts<'a, 'm>, memory: &'m WorkingMemory) -> Self {
        Self {
            engine: FlowEngine::new(facts, memory),
        }
    }
    pub fn analyze(
        &mut self,
        abi: Option<CallAbi>,
        decoder: &dyn FunctionSemantics,
        select: &mut SelectRead<'_>,
        calls: &CallAt<'_>,
        c: &mut dyn RunControl,
    ) -> Result<()> {
        self.engine.analyze(abi, decoder, select, calls, c)
    }
    pub fn reads(
        &self,
        offset: u64,
        address: &AbstractValue,
        width: u8,
        access: MemoryKind,
        c: &mut dyn RunControl,
    ) -> Result<ReadUses> {
        self.engine.reads(offset, address, width, access, c)
    }
}

fn opaque_read_registers(bytes: &[u8], operation: SemanticOp) -> u32 {
    use oer_analysis_riscv_decode::{
        Extension, Extensions, Float, Instruction, Operand as IsaOperand, Register,
    };
    match oer_analysis_riscv_decode::decode(bytes, Extensions::ALL)
        .map(|(instruction, _)| instruction)
    {
        Some(Instruction::Extension(Extension::Csr {
            source: IsaOperand::Register(register),
            ..
        })) => 1u32.wrapping_shl(u32::from(register)) & !1,
        Some(Instruction::Float(Float::Operation { operands, .. })) => operands
            .iter()
            .skip(1)
            .filter_map(|operand| match operand {
                Some(Register::Integer(register)) => Some(1u32.wrapping_shl(u32::from(*register))),
                _ => None,
            })
            .fold(0, |a, b| a | b),
        _ if operation == SemanticOp::Unsupported => !1,
        _ => 0,
    }
}

fn read_operation(
    op: IntegerOp,
    a: ReadKnown,
    b: ReadKnown,
    l: ReadDependency,
    r: ReadDependency,
) -> ReadDependency {
    let mut out = ReadDependency::empty(l.source);
    let right = b.constant_value();
    use IntegerOp::*;
    match op {
        And | Or | Xor | AndNot | OrNot | XorNot => {
            let (b, r) = if matches!(op, AndNot | OrNot | XorNot) {
                let mut inverted = r;
                inverted.inverted ^= u32::MAX;
                (b.not(), inverted)
            } else {
                (b, r)
            };
            for (i, ((exact, may), ((le, lm), (re, rm)))) in out
                .exact
                .iter_mut()
                .zip(&mut out.may)
                .zip(l.exact.iter().zip(&l.may).zip(r.exact.iter().zip(&r.may)))
                .enumerate()
            {
                let bit = 1 << i;
                let a_zero = a.zero & bit != 0;
                let a_one = a.one & bit != 0;
                let b_zero = b.zero & bit != 0;
                let b_one = b.one & bit != 0;
                let source = match op {
                    And | AndNot if a_zero || b_zero => None,
                    Or | OrNot if a_one || b_one => None,
                    And | AndNot if a_one => Some((re, rm, r.inverted, false)),
                    And | AndNot if b_one => Some((le, lm, l.inverted, false)),
                    Or | OrNot if a_zero => Some((re, rm, r.inverted, false)),
                    Or | OrNot if b_zero => Some((le, lm, l.inverted, false)),
                    Xor | XorNot if a_zero || a_one => Some((re, rm, r.inverted, a_one)),
                    Xor | XorNot if b_zero || b_one => Some((le, lm, l.inverted, b_one)),
                    _ => {
                        *may = le | lm | re | rm;
                        None
                    }
                };
                if let Some((e, m, inverted, invert)) = source {
                    *exact = *e;
                    *may = *m;
                    out.inverted |= (inverted ^ if invert { bit } else { 0 }) & bit;
                }
            }
        }
        Shl | Shr | Sar | RotateLeft | RotateRight | BitExtract if right.is_some() => {
            let by = right.unwrap_or(0) as usize & 31;
            out = l.wire(|i| match op {
                Shl => i.checked_sub(by),
                Shr => (i + by < 32).then_some(i + by),
                Sar => Some((i + by).min(31)),
                RotateLeft => Some((i + 32 - by) & 31),
                RotateRight => Some((i + by) & 31),
                BitExtract => (i == 0).then_some(by),
                _ => None,
            });
        }
        SignExtendByte | SignExtendHalf | ByteReverse => {
            out = l.wire(|i| {
                Some(match op {
                    SignExtendByte => i.min(7),
                    SignExtendHalf => i.min(15),
                    _ => (3 - i / 8) * 8 + i % 8,
                })
            });
        }
        Add | Sub => {
            let mut bits = 0;
            for ((out, (le, lm)), (re, rm)) in out
                .may
                .iter_mut()
                .zip(l.exact.iter().zip(&l.may))
                .zip(r.exact.iter().zip(&r.may))
            {
                bits |= le | lm | re | rm;
                *out = bits;
            }
        }
        ShiftAdd1 | ShiftAdd2 | ShiftAdd3 => {
            let by = match op {
                ShiftAdd1 => 1,
                ShiftAdd2 => 2,
                _ => 3,
            };
            out = read_operation(
                Add,
                ReadKnown::default(),
                b,
                l.wire(|i| i.checked_sub(by)),
                r,
            );
        }
        _ => {
            let (a, b) = l.selected(u32::MAX);
            let (x, y) = r.selected(u32::MAX);
            out.may.fill(a | b | x | y);
        }
    }
    out
}

/// Operator nodes an indexed address is followed through; deeper sums are
/// left unresolved, never truncated into a different progression.
const INDEXED_DEPTH: u8 = 16;

/// `constant + coefficient * term`, every operation modulo 2^32, with at most
/// one term: a value the linear walk cannot split further. Terms are equal
/// when they are the same leaf or the same expression node.
#[derive(Clone, Copy)]
struct Linear<'a> {
    constant: u32,
    term: Option<(&'a AbstractValue, u32)>,
}

/// What one expression node decomposes to, memoized by its id: a
/// decomposition (or `None` when two different terms meet) and the levels it
/// needs, or the depth it could not be decomposed within.
#[derive(Clone, Copy)]
enum Decomposed<'a> {
    Known(Option<Linear<'a>>, u8),
    Deeper(u8),
}

impl<'a> Linear<'a> {
    fn scale(self, by: u32) -> Self {
        Self {
            constant: self.constant.wrapping_mul(by),
            term: self
                .term
                .map(|(term, coefficient)| (term, coefficient.wrapping_mul(by))),
        }
    }

    /// The sum, or `None` when the operands carry two different terms.
    fn add(self, other: Self) -> Option<Self> {
        let term = match (self.term, other.term) {
            (None, term) | (term, None) => term,
            (Some((a, x)), Some((b, y))) if a == b => Some((a, x.wrapping_add(y))),
            _ => return None,
        };
        Some(Self {
            constant: self.constant.wrapping_add(other.constant),
            term,
        })
    }
}

/// The decomposition of `value` within `depth` levels, memoized in the
/// caller's `memo` (one per function) by each expression node's dense id in
/// admitted working memory: an expression is a DAG whose nodes several sums
/// share, so a node reached again within no more levels costs nothing. A node
/// that did not decompose within its levels is decomposed again only when it
/// is reached with more, so at most `INDEXED_DEPTH + 1` times.
fn linear<'a, 'm>(
    facts: &Facts<'a, '_>,
    value: &'a AbstractValue,
    depth: u8,
    memo: &mut AdmittedVec<'m, Option<Decomposed<'a>>>,
    c: &mut dyn RunControl,
) -> Result<Decomposed<'a>> {
    if let Some(constant) = exact_address(value) {
        return Ok(Decomposed::Known(
            Some(Linear {
                constant,
                term: None,
            }),
            0,
        ));
    }
    let term = Linear {
        constant: 0,
        term: Some((value, 1)),
    };
    let (AbstractValue::Expression { id }, Some(Expression::Integer { op, left, right })) =
        (value, facts.value_expression(value))
    else {
        return Ok(Decomposed::Known(Some(term), 0));
    };
    let slot = *id as usize;
    match memo.get(slot).copied().flatten() {
        Some(known @ Decomposed::Known(..)) => return Ok(known),
        Some(Decomposed::Deeper(within)) if depth <= within => {
            return Ok(Decomposed::Deeper(depth));
        }
        _ => {}
    }
    let outcome = if depth == 0 {
        Decomposed::Deeper(0)
    } else {
        c.checkpoint(1)?;
        // The operands' decompositions within one level less, with the
        // levels they need; `None` when one is deeper.
        let mut height = 0u8;
        let mut operand = |v: &'a AbstractValue,
                           memo: &mut AdmittedVec<'m, Option<Decomposed<'a>>>,
                           c: &mut dyn RunControl|
         -> Result<Option<Option<Linear<'a>>>> {
            Ok(match linear(facts, v, depth - 1, memo, c)? {
                Decomposed::Known(result, needs) if needs < depth => {
                    height = height.max(needs);
                    Some(result)
                }
                _ => None,
            })
        };
        let result = match op {
            IntegerOp::Add | IntegerOp::Sub => {
                match (operand(left, memo, c)?, operand(right, memo, c)?) {
                    (Some(l), Some(r)) => Some(match (l, r) {
                        (Some(l), Some(r)) if *op == IntegerOp::Add => l.add(r),
                        (Some(l), Some(r)) => l.add(r.scale(u32::MAX)),
                        _ => None,
                    }),
                    _ => None,
                }
            }
            IntegerOp::Shl => match constant(right) {
                Some(by) => operand(left, memo, c)?.map(|l| l.map(|l| l.scale(1 << (by & 31)))),
                None => Some(Some(term)),
            },
            IntegerOp::Mul => match (constant(left), constant(right)) {
                (_, Some(by)) => operand(left, memo, c)?.map(|l| l.map(|l| l.scale(by))),
                (Some(by), _) => operand(right, memo, c)?.map(|r| r.map(|r| r.scale(by))),
                _ => Some(Some(term)),
            },
            IntegerOp::ShiftAdd1 | IntegerOp::ShiftAdd2 | IntegerOp::ShiftAdd3 => {
                let by = match op {
                    IntegerOp::ShiftAdd1 => 2,
                    IntegerOp::ShiftAdd2 => 4,
                    _ => 8,
                };
                match (operand(left, memo, c)?, operand(right, memo, c)?) {
                    (Some(l), Some(r)) => Some(match (l, r) {
                        (Some(l), Some(r)) => l.scale(by).add(r),
                        _ => None,
                    }),
                    _ => None,
                }
            }
            _ => Some(Some(term)),
        };
        match result {
            Some(result) => Decomposed::Known(result, height + 1),
            None => Decomposed::Deeper(depth),
        }
    };
    if memo.len() <= slot {
        c.checkpoint((slot + 1 - memo.len()) as u64)?;
        while memo.len() <= slot {
            memo.push(None, c.position())?;
        }
    }
    memo[slot] = Some(outcome);
    Ok(outcome)
}

/// The values an index expression is proven to stay below: `x & mask`, a
/// zero-extending byte or halfword load, `x %u n` and `x >> k`; `None` when
/// any 32-bit value is possible.
fn index_count(facts: &Facts<'_, '_>, index: &AbstractValue) -> Option<u32> {
    match facts.value_expression(index)? {
        Expression::Integer { op, left, right } => match op {
            IntegerOp::And => constant(right)
                .or_else(|| constant(left))
                .and_then(|mask| mask.checked_add(1)),
            IntegerOp::Remu => constant(right).filter(|n| *n != 0),
            IntegerOp::Shr => constant(right)
                .map(|by| by & 31)
                .filter(|by| *by != 0)
                .map(|by| 1 << (32 - by)),
            IntegerOp::Lt | IntegerOp::Ltu => Some(2),
            _ => None,
        },
        Expression::Load {
            width: 1,
            signed: false,
            ..
        } => Some(1 << 8),
        Expression::Load {
            width: 2,
            signed: false,
            ..
        } => Some(1 << 16),
        _ => None,
    }
}

/// The progressions of one function's unresolved addresses, over one memo
/// for the whole function: an expression node is decomposed once, whichever
/// address reaches it, and again only when reached with more levels than it
/// failed within (at most `INDEXED_DEPTH + 1` times). The memo lives in
/// admitted working memory, and filling it up to a new id is charged too.
pub struct IndexedAddresses<'f, 'a, 'm> {
    facts: &'f Facts<'a, 'm>,
    memo: AdmittedVec<'m, Option<Decomposed<'a>>>,
}

impl<'f, 'a, 'm> IndexedAddresses<'f, 'a, 'm> {
    pub fn new(facts: &'f Facts<'a, 'm>, memory: &'m WorkingMemory) -> Self {
        Self {
            facts,
            memo: AdmittedVec::new(memory),
        }
    }

    /// The progression an unresolved `address` selects from when the
    /// analysis computes it as a constant plus a nonzero multiple of one
    /// unknown index, through additions, subtractions, constant shifts and
    /// multiplications and `shNadd`. Any other shape, two different unknowns
    /// or a sum deeper than [`INDEXED_DEPTH`] is `None`: the address stays
    /// unresolved.
    pub fn address(
        &mut self,
        address: &'a AbstractValue,
        c: &mut dyn RunControl,
    ) -> Result<Option<IndexedAddress>> {
        let facts = self.facts;
        if facts.value_expression(address).is_none() {
            return Ok(None);
        }
        let Decomposed::Known(
            Some(Linear {
                constant,
                term: Some((index, coefficient)),
            }),
            _,
        ) = linear(facts, address, INDEXED_DEPTH, &mut self.memo, c)?
        else {
            return Ok(None);
        };
        let count = index_count(facts, index);
        // An unbounded index with a unit step reaches every address: that is a
        // pointer plus an offset, not an array, and says nothing.
        if coefficient == 0 || (count.is_none() && matches!(coefficient, 1 | u32::MAX)) {
            return Ok(None);
        }
        Ok(Some(IndexedAddress {
            base: constant,
            stride: coefficient as i32,
            count,
        }))
    }
}

fn exact_address(value: &AbstractValue) -> Option<u32> {
    match value {
        AbstractValue::Constant { value } | AbstractValue::ImageAddress { address: value } => {
            Some(*value)
        }
        _ => None,
    }
}

fn same_word(loaded: &AbstractValue, loaded_width: u8, address: &AbstractValue, width: u8) -> bool {
    loaded == address && loaded_width == width
}

#[cfg(test)]
mod tests {
    use super::*;
    fn value(id: u32) -> AbstractValue {
        AbstractValue::Expression { id }
    }
    fn number(value: u32) -> AbstractValue {
        AbstractValue::Constant { value }
    }
    fn records(expressions: Vec<Expression>) -> Vec<FunctionRecord> {
        expressions
            .into_iter()
            .enumerate()
            .map(|(id, expression)| FunctionRecord::Expression {
                id: id as u32,
                offset: id as u64 * 4,
                expression,
            })
            .collect()
    }
    #[test]
    fn numeric_alternatives_keep_only_bits_shared_by_every_value() {
        for (values, varying) in [
            (
                vec![
                    ValueAlternative::Constant { value: 0x80 },
                    ValueAlternative::Constant { value: 0xa0 },
                ],
                0x20,
            ),
            (
                vec![
                    ValueAlternative::ImageAddress { address: 0x80 },
                    ValueAlternative::ImageAddress { address: 0xa0 },
                ],
                0x20,
            ),
            (
                vec![
                    ValueAlternative::Constant { value: 0x80 },
                    ValueAlternative::ImageAddress { address: 0xa0 },
                ],
                0x20,
            ),
            (
                vec![
                    ValueAlternative::Constant { value: 0x80 },
                    ValueAlternative::Constant { value: 0xa0 },
                    ValueAlternative::Constant { value: 0xc0 },
                ],
                0x60,
            ),
            (
                (0..8)
                    .map(|n| ValueAlternative::Constant {
                        value: 0x80 | (n << 4),
                    })
                    .collect(),
                0x70,
            ),
        ] {
            let value = AbstractValue::Alternatives {
                values: ValueAlternatives::new(values).expect("numeric alternatives"),
            };
            let bits = ReadKnown::from_fact(&value);
            assert_eq!(bits.one, 0x80);
            assert_eq!(bits.zero, !(0x80 | varying));
        }
        for address in [
            ValueAlternative::Section {
                section: 1,
                offset: 0x80,
            },
            ValueAlternative::EntryStack { offset: 0x80 },
            ValueAlternative::Symbol {
                symbol: SymbolId {
                    object: ObjectId {
                        artifact: ArtifactId::of_bytes(b"alternatives fixture"),
                        location: ObjectLocation::Standalone,
                    },
                    table: SymbolTableKind::Static,
                    table_section: 1,
                    index: 2,
                },
                addend: 0x80,
            },
        ] {
            for values in [
                vec![address.clone(), ValueAlternative::Constant { value: 0xa0 }],
                vec![
                    address.clone(),
                    ValueAlternative::EntryStack { offset: 0xa0 },
                ],
            ] {
                let value = AbstractValue::Alternatives {
                    values: ValueAlternatives::new(values).expect("relocatable alternatives"),
                };
                assert_eq!(ReadKnown::from_fact(&value), ReadKnown::default());
            }
        }
    }
    /// The indexed progression of `expressions`' last node.
    fn indexed(expressions: Vec<Expression>) -> Option<IndexedAddress> {
        let memory = WorkingMemory::new(1 << 16).unwrap();
        let last = value(expressions.len() as u32 - 1);
        let records = records(expressions);
        let facts = Facts::new(&records, &memory, &mut || Ok(())).unwrap();
        IndexedAddresses::new(&facts, &memory)
            .address(&last, &mut || Ok(()))
            .unwrap()
    }
    fn integer(op: IntegerOp, left: AbstractValue, right: AbstractValue) -> Expression {
        Expression::Integer { op, left, right }
    }
    fn argument(register: u8) -> Expression {
        Expression::EntryRegister { register }
    }

    #[test]
    fn an_address_of_one_scaled_index_is_an_indexed_progression() {
        // coex_hw_timer_set: `(index + 0x2010f40) << 4`.
        assert_eq!(
            indexed(vec![
                argument(10),
                integer(IntegerOp::Add, value(0), number(0x0201_0f40)),
                integer(IntegerOp::Shl, value(1), number(4)),
                integer(IntegerOp::Add, value(2), number(4)),
            ]),
            Some(IndexedAddress {
                base: 0x2010_f404,
                stride: 0x10,
                count: None,
            })
        );
        // mac_tx_set_mplen: a descending queue vector, `index * -0x7c + base`.
        assert_eq!(
            indexed(vec![
                argument(11),
                integer(IntegerOp::Mul, value(0), number(0xffff_ff84)),
                integer(IntegerOp::Add, value(1), number(0x2010_54fc)),
            ]),
            Some(IndexedAddress {
                base: 0x2010_54fc,
                stride: -0x7c,
                count: None,
            })
        );
        // A masked index bounds the progression; `sh2add` scales it.
        assert_eq!(
            indexed(vec![
                argument(10),
                integer(IntegerOp::And, value(0), number(7)),
                integer(IntegerOp::ShiftAdd2, value(1), number(0x2010_4000)),
                integer(IntegerOp::Sub, value(2), number(8)),
            ]),
            Some(IndexedAddress {
                base: 0x2010_3ff8,
                stride: 4,
                count: Some(8),
            })
        );
        // The same index twice is still one index.
        assert_eq!(
            indexed(vec![
                argument(10),
                integer(IntegerOp::Shl, value(0), number(3)),
                integer(IntegerOp::Add, value(1), value(0)),
                integer(IntegerOp::Add, value(2), number(0x2010_0000)),
            ])
            .map(|indexed| indexed.stride),
            Some(9)
        );
    }

    #[test]
    fn a_pointer_offset_or_two_indices_stay_unresolved() {
        // A loaded pointer plus a field offset: any address, not an array.
        assert_eq!(
            indexed(vec![
                Expression::Load {
                    address: number(0x3fc0_0000),
                    width: 4,
                    signed: false,
                },
                integer(IntegerOp::Add, value(0), number(8)),
            ]),
            None
        );
        // Two different unknowns.
        assert_eq!(
            indexed(vec![
                argument(10),
                argument(11),
                integer(IntegerOp::Shl, value(0), number(4)),
                integer(IntegerOp::Add, value(2), value(1)),
            ]),
            None
        );
        // An index that cancels out leaves no progression.
        assert_eq!(
            indexed(vec![
                argument(10),
                integer(IntegerOp::Shl, value(0), number(2)),
                integer(IntegerOp::Sub, value(1), value(1)),
                integer(IntegerOp::Add, value(2), number(0x2010_0000)),
            ]),
            None
        );
        // A byte index bounds a unit stride, which is then an array again.
        assert_eq!(
            indexed(vec![
                Expression::Load {
                    address: number(0x3fc0_0000),
                    width: 1,
                    signed: false,
                },
                integer(IntegerOp::Add, value(0), number(0x2010_0000)),
            ]),
            Some(IndexedAddress {
                base: 0x2010_0000,
                stride: 1,
                count: Some(256),
            })
        );
    }

    #[test]
    fn a_sum_deeper_than_the_bound_stays_unresolved_and_is_charged() {
        let mut expressions = vec![argument(10), integer(IntegerOp::Shl, value(0), number(2))];
        for id in 1..=u32::from(INDEXED_DEPTH) + 1 {
            expressions.push(integer(IntegerOp::Add, value(id), number(4)));
        }
        assert_eq!(indexed(expressions), None);
        let memory = WorkingMemory::new(1 << 16).unwrap();
        let records = records(vec![
            argument(10),
            integer(IntegerOp::Shl, value(0), number(2)),
            integer(IntegerOp::Add, value(1), number(0x2010_0000)),
        ]);
        let facts = Facts::new(&records, &memory, &mut || Ok(())).unwrap();
        let refused = IndexedAddresses::new(&facts, &memory).address(&value(2), &mut || {
            Err(Error::new(ErrorCode::ResourceLimited, "work budget"))
        });
        assert_eq!(refused.unwrap_err().code, ErrorCode::ResourceLimited);
    }

    #[test]
    fn a_shared_node_is_decomposed_once_however_many_paths_reach_it() {
        // x0 = a0 & 7; x(i+1) = x(i) + x(i); the address is x14 + base: 2^14
        // paths reach x0, and each node is still charged once.
        let mut expressions = vec![argument(10), integer(IntegerOp::And, value(0), number(7))];
        for id in 1..15 {
            expressions.push(integer(IntegerOp::Add, value(id), value(id)));
        }
        expressions.push(integer(
            IntegerOp::Add,
            value(expressions.len() as u32 - 1),
            number(0x2010_0000),
        ));
        let nodes = expressions.len() as u64;
        let memory = WorkingMemory::new(1 << 16).unwrap();
        let last = value(expressions.len() as u32 - 1);
        let records = records(expressions);
        let facts = Facts::new(&records, &memory, &mut || Ok(())).unwrap();
        let mut checkpoints = 0u64;
        let mut count = || {
            checkpoints += 1;
            Ok(())
        };
        let mut addresses = IndexedAddresses::new(&facts, &memory);
        assert_eq!(
            addresses.address(&last, &mut count).unwrap(),
            Some(IndexedAddress {
                base: 0x2010_0000,
                stride: 1 << 14,
                count: Some(8),
            })
        );
        assert!(
            checkpoints <= 2 * nodes,
            "{checkpoints} checkpoints for {nodes} nodes"
        );
        // A second address of the function reuses the memo: no node is
        // decomposed again.
        let before = checkpoints;
        let mut count = || {
            checkpoints += 1;
            Ok(())
        };
        addresses.address(&last, &mut count).unwrap();
        assert_eq!(checkpoints, before);
    }
}

#[cfg(test)]
#[path = "registers/read_tests.rs"]
mod read_tests;
