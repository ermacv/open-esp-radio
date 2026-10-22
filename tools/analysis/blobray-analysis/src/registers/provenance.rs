//! Fixed-size bit provenance. Joining different tags always yields Unknown.
use super::*;

/// Compact encoding of Unknown, zero, one, or a word/entry source and bit.
/// No sets or expression nodes grow behind a cell.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub(super) struct Tag(u32);
impl Tag {
    const UNKNOWN: Self = Self(0);
    const ZERO: Self = Self(1);
    const ONE: Self = Self(2);
    fn source(source: usize, bit: u8) -> Result<Self> {
        u32::try_from(source)
            .ok()
            .and_then(|s| s.checked_mul(32))
            .and_then(|s| s.checked_add(u32::from(bit) + 3))
            .map(Self)
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::ResourceLimited,
                    "provenance source count exhausted",
                )
            })
    }
    fn origin(self) -> Option<(usize, u8)> {
        self.0
            .checked_sub(3)
            .map(|v| ((v / 32) as usize, (v % 32) as u8))
    }
    fn not(self) -> Self {
        match self {
            Self::ZERO => Self::ONE,
            Self::ONE => Self::ZERO,
            _ => Self::UNKNOWN,
        }
    }
    fn and(self, other: Self) -> Self {
        match (self, other) {
            (Self::ZERO, _) | (_, Self::ZERO) => Self::ZERO,
            (Self::ONE, x) | (x, Self::ONE) => x,
            (a, b) if a == b => a,
            _ => Self::UNKNOWN,
        }
    }
    fn or(self, other: Self) -> Self {
        match (self, other) {
            (Self::ONE, _) | (_, Self::ONE) => Self::ONE,
            (Self::ZERO, x) | (x, Self::ZERO) => x,
            (a, b) if a == b => a,
            _ => Self::UNKNOWN,
        }
    }
    fn xor(self, other: Self) -> Self {
        match (self, other) {
            (Self::ZERO, x) | (x, Self::ZERO) => x,
            (Self::ONE, x) | (x, Self::ONE) => x.not(),
            _ => Self::UNKNOWN,
        }
    }
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub(super) struct ProvenanceValue([Tag; 32]);
impl FlowValue for ProvenanceValue {
    fn constant(value: u32) -> Self {
        Self(std::array::from_fn(|i| {
            if value & (1 << i) == 0 {
                Tag::ZERO
            } else {
                Tag::ONE
            }
        }))
    }
}
impl ProvenanceValue {
    fn bit(self, index: usize) -> Tag {
        self.0.get(index).copied().unwrap_or(Tag::UNKNOWN)
    }
    fn wire(self, mapping: impl Fn(usize) -> Option<usize>, padding: Tag) -> Self {
        Self(std::array::from_fn(|i| {
            mapping(i)
                .and_then(|from| self.0.get(from).copied())
                .unwrap_or(padding)
        }))
    }
    fn integer(op: IntegerOp, a: Self, b: Self, amount: Option<u32>) -> Self {
        use IntegerOp::*;
        match op {
            AndNot => Self::integer(And, a, Self(b.0.map(Tag::not)), amount),
            OrNot => Self::integer(Or, a, Self(b.0.map(Tag::not)), amount),
            XorNot => Self::integer(Xor, a, Self(b.0.map(Tag::not)), amount),
            And | Or | Xor => Self(std::array::from_fn(|i| match op {
                And => a.bit(i).and(b.bit(i)),
                Or => a.bit(i).or(b.bit(i)),
                _ => a.bit(i).xor(b.bit(i)),
            })),
            Add => {
                let mut carry = false;
                Self(std::array::from_fn(|i| {
                    if carry {
                        return Tag::UNKNOWN;
                    }
                    match (a.bit(i), b.bit(i)) {
                        (Tag::ZERO, x) | (x, Tag::ZERO) => x,
                        _ => {
                            carry = true;
                            Tag::UNKNOWN
                        }
                    }
                }))
            }
            Sub if amount.is_some() => {
                let low = amount.unwrap_or(0).trailing_zeros() as usize;
                a.wire(|i| (i < low).then_some(i), Tag::UNKNOWN)
            }
            ShiftAdd1 | ShiftAdd2 | ShiftAdd3 => {
                let by = match op {
                    ShiftAdd1 => 1,
                    ShiftAdd2 => 2,
                    _ => 3,
                };
                Self::integer(Add, a.wire(|i| i.checked_sub(by), Tag::ZERO), b, amount)
            }
            Shl | Shr | Sar | RotateLeft | RotateRight | BitExtract if amount.is_some() => {
                let by = amount.unwrap_or(0) as usize & 31;
                a.wire(
                    |i| match op {
                        Shl => i.checked_sub(by),
                        Shr => (i + by < 32).then_some(i + by),
                        Sar => Some((i + by).min(31)),
                        RotateLeft => Some((i + 32 - by) & 31),
                        RotateRight => Some((i + by) & 31),
                        BitExtract => (i == 0).then_some(by),
                        _ => None,
                    },
                    Tag::ZERO,
                )
            }
            SignExtendByte | SignExtendHalf | ByteReverse => a.wire(
                |i| {
                    Some(match op {
                        SignExtendByte => i.min(7),
                        SignExtendHalf => i.min(15),
                        _ => (3 - i / 8) * 8 + i % 8,
                    })
                },
                Tag::ZERO,
            ),
            _ => Self::default(),
        }
    }
    fn known(self) -> ReadKnown {
        self.0
            .iter()
            .enumerate()
            .fold(ReadKnown::default(), |mut known, (i, tag)| {
                if *tag == Tag::ZERO {
                    known.zero |= 1 << i;
                }
                if *tag == Tag::ONE {
                    known.one |= 1 << i;
                }
                known
            })
    }
    fn value_bits(mut self, known: ReadKnown) -> Self {
        for (i, tag) in self.0.iter_mut().enumerate() {
            if known.zero & (1 << i) != 0 {
                *tag = Tag::ZERO;
            }
            if known.one & (1 << i) != 0 {
                *tag = Tag::ONE;
            }
        }
        self
    }
}

pub(super) struct Provenance<'m> {
    memory: &'m WorkingMemory,
    words: AdmittedVec<'m, usize>,
}
impl<'m> Domain<'m> for Provenance<'m> {
    type Flow = ProvenanceValue;
    type Byte = [Tag; 8];
    const ALL_LOADS: bool = true;
    const RECOVER: bool = true;
    const EMIT: bool = false;
    const INSTRUCTION_WORK: bool = true;
    fn new(memory: &'m WorkingMemory) -> Self {
        Self {
            memory,
            words: AdmittedVec::new(memory),
        }
    }
    fn prepare_sources(e: &mut FlowEngine<'_, '_, 'm, Self>, c: &mut dyn RunControl) -> Result<()> {
        use std::hash::{Hash, Hasher};
        let count = e.sources.len();
        let size = count
            .max(1)
            .checked_next_power_of_two()
            .ok_or_else(read_integrity)?;
        let mut buckets = AdmittedVec::new(e.domain.memory);
        let mut next = AdmittedVec::new(e.domain.memory);
        for _ in 0..size {
            buckets.push(0usize, c.position())?;
        }
        for _ in 0..count {
            next.push(0usize, c.position())?;
        }
        for (id, source) in e.sources.iter().enumerate() {
            c.checkpoint(0)?;
            let ReadOrigin::Load { address, width, .. } = source.origin else {
                return Err(read_integrity());
            };
            let mut hash = std::collections::hash_map::DefaultHasher::new();
            width.hash(&mut hash);
            if let Some(address) = exact_address(address) {
                (0u8, address).hash(&mut hash);
            } else {
                1u8.hash(&mut hash);
                address.hash(&mut hash);
            }
            let bucket = (hash.finish() as usize) % size;
            let head = *buckets.get(bucket).ok_or_else(read_integrity)?;
            let mut candidate = head;
            let mut word = id;
            while candidate != 0 {
                let other = candidate - 1;
                let ReadOrigin::Load {
                    address: a,
                    width: w,
                    ..
                } = e.sources.get(other).ok_or_else(read_integrity)?.origin
                else {
                    return Err(read_integrity());
                };
                if same_provenance_word(a, w, address, width) {
                    word = other;
                    break;
                }
                candidate = *next.get(other).ok_or_else(read_integrity)?;
            }
            if word == id {
                *next.get_mut(id).ok_or_else(read_integrity)? = head;
                *buckets.get_mut(bucket).ok_or_else(read_integrity)? = id + 1;
            }
            e.domain.words.push(word, c.position())?;
        }
        Ok(())
    }
    fn entry_source(register: u8, _: bool) -> bool {
        register != 0 && register != 2
    }
    fn fact_known(value: &AbstractValue) -> ReadKnown {
        ReadKnown::from_fact(value)
    }
    fn refine(mut flow: Self::Flow, known: ReadKnown) -> Self::Flow {
        for (i, tag) in flow.0.iter_mut().enumerate() {
            // Knowing a loaded bit's value does not erase its word identity.
            // The shared known masks retain that independent value proof.
            if tag.origin().is_some() {
                continue;
            }
            if known.zero & (1 << i) != 0 {
                *tag = Tag::ZERO;
            }
            if known.one & (1 << i) != 0 {
                *tag = Tag::ONE;
            }
        }
        flow
    }
    fn known(flow: Self::Flow, known: ReadKnown) -> ReadKnown {
        let tags = flow.known();
        ReadKnown {
            zero: known.zero | tags.zero,
            one: known.one | tags.one,
        }
    }
    fn join(
        _: &mut FlowEngine<'_, '_, 'm, Self>,
        a: Self::Flow,
        b: Self::Flow,
        c: &mut dyn RunControl,
    ) -> Result<Self::Flow> {
        if a == b {
            return Ok(a);
        }
        c.checkpoint(1)?;
        Ok(ProvenanceValue(std::array::from_fn(|i| {
            if a.bit(i) == b.bit(i) {
                a.bit(i)
            } else {
                Tag::UNKNOWN
            }
        })))
    }
    fn binary(
        e: &mut FlowEngine<'_, '_, 'm, Self>,
        a: Self::Flow,
        b: Self::Flow,
        op: ReadFlowOp,
        c: &mut dyn RunControl,
    ) -> Result<Self::Flow> {
        match op {
            ReadFlowOp::Join => Self::join(e, a, b, c),
            ReadFlowOp::Assemble => {
                c.checkpoint(1)?;
                Ok(ProvenanceValue(std::array::from_fn(|i| {
                    match (a.bit(i), b.bit(i)) {
                        (Tag::UNKNOWN, x) | (x, Tag::UNKNOWN) => x,
                        (x, y) if x == y => x,
                        _ => Tag::UNKNOWN,
                    }
                })))
            }
            ReadFlowOp::Integer(op, _, _) => Ok(ProvenanceValue::integer(
                op,
                a,
                b,
                b.known().constant_value(),
            )),
        }
    }
    fn integer(
        _: &mut FlowEngine<'_, '_, 'm, Self>,
        op: IntegerOp,
        a: ReadCell<Self::Flow>,
        b: ReadCell<Self::Flow>,
        c: &mut dyn RunControl,
    ) -> Result<Self::Flow> {
        c.checkpoint(1)?;
        let amount = b.known.constant_value();
        let mut flow = ProvenanceValue::integer(op, a.flow, b.flow, amount);
        // Keep a structural word proof first. Value knowledge can also prove
        // an operand neutral, e.g. the zero bits of a spilled field in OR.
        for (left, right) in [
            (a.flow, b.flow.value_bits(b.known)),
            (a.flow.value_bits(a.known), b.flow),
            (a.flow.value_bits(a.known), b.flow.value_bits(b.known)),
        ] {
            let value = ProvenanceValue::integer(op, left, right, amount);
            for (tag, fallback) in flow.0.iter_mut().zip(value.0) {
                if *tag == Tag::UNKNOWN {
                    *tag = fallback;
                }
            }
        }
        Ok(flow)
    }
    fn map(
        _: &mut FlowEngine<'_, '_, 'm, Self>,
        flow: Self::Flow,
        mapping: impl Fn(usize) -> Option<usize>,
        c: &mut dyn RunControl,
    ) -> Result<Self::Flow> {
        c.checkpoint(1)?;
        Ok(flow.wire(mapping, Tag::UNKNOWN))
    }
    fn seed(
        e: &mut FlowEngine<'_, '_, 'm, Self>,
        source: usize,
        width: u8,
        signed: bool,
        outputs: u32,
        c: &mut dyn RunControl,
    ) -> Result<Self::Flow> {
        c.checkpoint(1)?;
        let source = e.domain.words.get(source).copied().unwrap_or(source);
        let count = usize::from(width).saturating_mul(8).min(32);
        let mut value = ProvenanceValue::default();
        for (i, tag) in value.0.iter_mut().enumerate() {
            if outputs & (1 << i) == 0 {
                continue;
            }
            *tag = if i < count {
                Tag::source(source, i as u8)?
            } else if signed && count != 0 {
                Tag::source(source, (count - 1) as u8)?
            } else {
                Tag::ZERO
            };
        }
        Ok(value)
    }
    fn load(
        e: &mut FlowEngine<'_, '_, 'm, Self>,
        flow: Self::Flow,
        source: usize,
        width: u8,
        signed: bool,
        forwarded: u32,
        c: &mut dyn RunControl,
    ) -> Result<Self::Flow> {
        let seed = Self::seed(e, source, width, signed, !forwarded, c)?;
        Ok(ProvenanceValue(std::array::from_fn(|i| {
            if forwarded & (1 << i) != 0 {
                flow.bit(i)
            } else {
                seed.bit(i)
            }
        })))
    }
    fn pack(cell: ReadCell<Self::Flow>) -> Self::Byte {
        std::array::from_fn(|i| {
            if cell.known.one & (1 << i) != 0 {
                Tag::ONE
            } else if cell.known.zero & (1 << i) != 0 {
                Tag::ZERO
            } else {
                cell.flow.bit(i)
            }
        })
    }
    fn unpack(byte: Self::Byte) -> ReadCell<Self::Flow> {
        let flow = ProvenanceValue(std::array::from_fn(|i| {
            byte.get(i).copied().unwrap_or(Tag::UNKNOWN)
        }));
        let known = flow.known();
        let atom = if (known.zero | known.one) & 255 == 255 {
            ReadAtom::Constant(known.one & 255)
        } else {
            ReadAtom::Unknown
        };
        ReadCell { flow, known, atom }
    }
    fn call_mapping(
        _: &mut FlowEngine<'_, '_, 'm, Self>,
        mut result: Self::Flow,
        incoming: Self::Flow,
        mapping: &ReturnedArgument,
        covered: &mut u32,
        c: &mut dyn RunControl,
    ) -> Result<Self::Flow> {
        c.checkpoint(1)?;
        for (i, (output, (&exact, &may))) in result
            .0
            .iter_mut()
            .zip(mapping.exact.iter().zip(&mapping.may))
            .enumerate()
        {
            if exact == 0 && may == 0 {
                continue;
            }
            let tag = if exact.count_ones() == 1 && may == 0 {
                let tag = incoming.bit(exact.trailing_zeros() as usize);
                if mapping.inverted & (1 << i) != 0 {
                    tag.not()
                } else {
                    tag
                }
            } else {
                Tag::UNKNOWN
            };
            *output = if *covered & (1 << i) == 0 || *output == tag {
                tag
            } else {
                Tag::UNKNOWN
            };
            *covered |= 1 << i;
        }
        Ok(result)
    }
    fn call_constants(cell: &mut ReadCell<Self::Flow>, summary: &CallSummary, register: u8) {
        if let Some(values) = CallAbi::RiscvInteger
            .results()
            .iter()
            .position(|r| *r == register)
            .and_then(|index| summary.results.get(index))
            .and_then(|v| v.as_ref())
            .filter(|v| !v.is_empty())
        {
            cell.known = values.iter().fold(
                ReadKnown {
                    zero: u32::MAX,
                    one: u32::MAX,
                },
                |known, v| ReadKnown {
                    zero: known.zero & !v,
                    one: known.one & v,
                },
            );
            cell.flow = Self::refine(cell.flow, cell.known);
            cell.atom = cell
                .known
                .constant_value()
                .map(ReadAtom::Constant)
                .unwrap_or_default();
        }
    }
}

fn same_provenance_word(a: &AbstractValue, aw: u8, b: &AbstractValue, bw: u8) -> bool {
    aw == bw
        && match (exact_address(a), exact_address(b)) {
            (Some(a), Some(b)) => a == b,
            _ => a == b,
        }
}

/// Stored bits from a second instantiation of the shared CFG engine.
pub struct StoredProvenanceEvaluator<'f, 'a, 'm> {
    engine: FlowEngine<'f, 'a, 'm, Provenance<'m>>,
}
impl<'f, 'a, 'm> StoredProvenanceEvaluator<'f, 'a, 'm> {
    pub fn new(facts: &'f Facts<'a, 'm>, memory: &'m WorkingMemory) -> Self {
        Self {
            engine: FlowEngine::new(facts, memory),
        }
    }
    pub fn analyze(
        &mut self,
        abi: Option<CallAbi>,
        decoder: &dyn FunctionSemantics,
        calls: &dyn CallAt<'_>,
        c: &mut dyn RunControl,
    ) -> Result<()> {
        self.engine
            .analyze(abi, decoder, &mut |_, _, _| Ok(true), calls, c)
    }
    /// The second pass has its own function budget. A limit here never changes
    /// the read-use pass or its WorkLimit classification.
    pub fn work_limited(&self) -> bool {
        self.engine.work_limit
    }
}

impl StoredProvenanceEvaluator<'_, '_, '_> {
    pub fn stored(
        &self,
        site: u64,
        address: &AbstractValue,
        width: u8,
        value: Option<&AbstractValue>,
        c: &mut dyn RunControl,
    ) -> Result<Vec<StoredBits>> {
        let Some(mask) = width_mask(width) else {
            return Ok(Vec::new());
        };
        let mut cell = if self.engine.prepared {
            self.engine
                .at(site)
                .and_then(|id| self.engine.nodes.get(id))
                .filter(|node| !node.unsettled)
                .and_then(|node| {
                    let SemanticOp::Memory {
                        source: Some(register),
                        ..
                    } = node.instruction.operation
                    else {
                        return None;
                    };
                    node.input.map(|input| input.register(register))
                })
                .unwrap_or_default()
        } else {
            ReadCell::default()
        };
        let known = value.map(ReadKnown::from_fact).unwrap_or_default();
        cell.known.zero |= known.zero;
        cell.known.one |= known.one;
        cell.flow = Provenance::refine(cell.flow, cell.known);
        let mut runs: Vec<StoredBits> = Vec::new();
        let mut previous_source = None;
        c.checkpoint(1)?;
        for bit in 0..mask.count_ones() as u8 {
            let flag = 1 << bit;
            let mut origin = None;
            let source = if (cell.known.zero | cell.known.one) & flag != 0 {
                StoredBitsSource::Constant {
                    value: (cell.known.one >> bit) & 1,
                }
            } else {
                if let Some((id, low)) = cell.flow.bit(usize::from(bit)).origin() {
                    origin = Some(id);
                    {
                        match self
                            .engine
                            .sources
                            .get(id)
                            .ok_or_else(read_integrity)?
                            .origin
                        {
                            ReadOrigin::MissingResult { .. } => StoredBitsSource::Unknown,
                            ReadOrigin::Entry(register) => {
                                StoredBitsSource::EntryRegister { register, low }
                            }
                            ReadOrigin::Load {
                                address: loaded,
                                width: loaded_width,
                                ..
                            } => StoredBitsSource::Load {
                                address: exact_address(loaded),
                                width: loaded_width,
                                low,
                                same_word: same_provenance_word(
                                    loaded,
                                    loaded_width,
                                    address,
                                    width,
                                ),
                            },
                        }
                    }
                } else {
                    StoredBitsSource::Unknown
                }
            };
            if let Some(last) = runs.last_mut() {
                let adjacent = match (&last.source, &source) {
                    (StoredBitsSource::Constant { .. }, StoredBitsSource::Constant { .. }) => true,
                    (StoredBitsSource::Unknown, StoredBitsSource::Unknown) => true,
                    (
                        StoredBitsSource::EntryRegister { low, .. },
                        StoredBitsSource::EntryRegister { low: next, .. },
                    )
                    | (
                        StoredBitsSource::Load { low, .. },
                        StoredBitsSource::Load { low: next, .. },
                    ) => previous_source == origin && *next == *low + last.width,
                    _ => false,
                };
                if adjacent {
                    if let (
                        StoredBitsSource::Constant { value },
                        StoredBitsSource::Constant { value: next },
                    ) = (&mut last.source, &source)
                    {
                        *value |= next << last.width;
                    }
                    last.width += 1;
                    continue;
                }
            }
            runs.push(StoredBits {
                low: bit,
                width: 1,
                source,
            });
            previous_source = origin;
        }
        Ok(runs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn store<'a>(
        reads: &mut StoredProvenanceEvaluator<'_, 'a, '_>,
        cell: ReadCell<ProvenanceValue>,
        value: Option<&AbstractValue>,
    ) -> Vec<StoredBits> {
        let mut input = ReadState::entry();
        input.set(6, cell);
        if reads.engine.nodes.is_empty() {
            reads
                .engine
                .nodes
                .push(
                    ReadNode {
                        instruction: ReadInstruction {
                            site: 0,
                            opaque: 0,
                            operation: SemanticOp::Memory {
                                kind: MemoryKind::Store,
                                base: 5,
                                displacement: 0,
                                width: 4,
                                dest: None,
                                source: Some(6),
                                signed: false,
                                swap: false,
                            },
                            branch: None,
                            flow: InstructionFlow::Next,
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
                        input: Some(input),
                        queued: false,
                        unsettled: false,
                    },
                    RunPosition::default(),
                )
                .expect("store node");
        } else {
            reads.engine.nodes[0].input = Some(input);
        }
        reads.engine.prepared = true;
        reads
            .stored(
                0,
                &AbstractValue::Constant { value: 0x20000 },
                4,
                value,
                &mut || Ok(()),
            )
            .expect("stored projection")
    }
    #[test]
    fn a_limit_invalidates_only_the_forward_closure_and_literal_facts_survive() {
        let memory = WorkingMemory::new(1 << 20).expect("memory");
        let facts = Facts::new(&[], &memory, &mut || Ok(())).expect("facts");
        let mut reads = StoredProvenanceEvaluator::new(&facts, &memory);
        let entry = ReadCell::constant(42);
        store(&mut reads, entry, None);
        let instruction = reads.engine.nodes[0].instruction;
        reads
            .engine
            .nodes
            .push(
                ReadNode {
                    instruction: ReadInstruction {
                        site: 4,
                        ..instruction
                    },
                    input: reads.engine.nodes[0].input,
                    queued: true,
                    unsettled: false,
                },
                RunPosition::default(),
            )
            .expect("pending store");
        reads
            .engine
            .edges
            .push(
                ReadEdge {
                    from: 1,
                    target: Some(1),
                    relation: EdgeKind::Jump,
                },
                RunPosition::default(),
            )
            .expect("cycle");
        reads.engine.nodes[1].instruction.edge_end = 1;
        reads.engine.work_limit = true;
        reads
            .engine
            .invalidate_pending(&mut || Ok(()))
            .expect("pending closure");
        assert!(!reads.engine.nodes[0].unsettled);
        assert!(reads.engine.nodes[1].unsettled);
        assert_eq!(
            reads
                .stored(0, &AbstractValue::Unknown, 4, None, &mut || Ok(()))
                .expect("stable store")[0]
                .source,
            StoredBitsSource::Constant { value: 42 }
        );
        assert_eq!(
            reads
                .stored(4, &AbstractValue::Unknown, 4, None, &mut || Ok(()))
                .expect("unsettled store")[0]
                .source,
            StoredBitsSource::Unknown
        );
        assert_eq!(
            reads
                .stored(
                    4,
                    &AbstractValue::Unknown,
                    4,
                    Some(&AbstractValue::Constant { value: 7 }),
                    &mut || Ok(())
                )
                .expect("independent literal")[0]
                .source,
            StoredBitsSource::Constant { value: 7 }
        );
    }
    #[test]
    fn fixed_cells_join_equal_tags_and_discard_conflicts_and_absence() {
        assert_eq!(std::mem::size_of::<ProvenanceValue>(), 32 * 4);
        assert_eq!(
            std::mem::size_of::<<Provenance as Domain<'_>>::Byte>(),
            8 * 4
        );
        let memory = WorkingMemory::new(1 << 20).expect("memory");
        let facts = Facts::new(&[], &memory, &mut || Ok(())).expect("facts");
        let mut engine = FlowEngine::<Provenance>::new(&facts, &memory);
        let a =
            Provenance::seed(&mut engine, 0, 4, false, u32::MAX, &mut || Ok(())).expect("source a");
        let b =
            Provenance::seed(&mut engine, 1, 4, false, u32::MAX, &mut || Ok(())).expect("source b");
        assert_eq!(
            Provenance::join(&mut engine, a, a, &mut || Ok(())).expect("same"),
            a
        );
        for other in [b, ProvenanceValue::default()] {
            let joined = Provenance::join(&mut engine, a, other, &mut || Ok(())).expect("conflict");
            assert_eq!(joined, ProvenanceValue::default());
            assert_eq!(
                Provenance::join(&mut engine, joined, a, &mut || Ok(())).expect("remains unknown"),
                joined
            );
        }
        let masked = ProvenanceValue::integer(
            IntegerOp::And,
            a,
            ProvenanceValue::constant(0xff),
            Some(0xff),
        );
        let shifted = masked.wire(|i| i.checked_sub(1), Tag::ZERO);
        let joined = Provenance::join(&mut engine, masked, shifted, &mut || Ok(()))
            .expect("per-bit consensus");
        assert!(joined.0[..9].iter().all(|tag| *tag == Tag::UNKNOWN));
        assert!(joined.0[9..].iter().all(|tag| *tag == Tag::ZERO));
    }
    #[test]
    fn provenance_drops_inversion_and_carry_but_keeps_neutral_operations() {
        let source = ProvenanceValue(std::array::from_fn(|i| {
            Tag::source(0, i as u8).expect("source")
        }));
        let zero = ProvenanceValue::constant(0);
        for op in [
            IntegerOp::Add,
            IntegerOp::Sub,
            IntegerOp::Xor,
            IntegerOp::Or,
        ] {
            assert_eq!(ProvenanceValue::integer(op, source, zero, Some(0)), source);
        }
        assert_eq!(
            ProvenanceValue::integer(
                IntegerOp::Xor,
                source,
                ProvenanceValue::constant(u32::MAX),
                Some(u32::MAX)
            ),
            ProvenanceValue::default()
        );
        assert_eq!(
            ProvenanceValue::integer(IntegerOp::Add, source, source, None),
            ProvenanceValue::default()
        );
    }
    #[test]
    fn call_composition_keeps_unknown_contributions_from_other_arguments() {
        let memory = WorkingMemory::new(1 << 20).expect("memory");
        let facts = Facts::new(&[], &memory, &mut || Ok(())).expect("facts");
        let mut engine = FlowEngine::<Provenance>::new(&facts, &memory);
        let mapping = ReturnedArgument {
            argument: 10,
            register: 10,
            exact: std::array::from_fn(|i| 1 << i),
            may: [0; 32],
            inverted: 0,
        };
        let source =
            Provenance::seed(&mut engine, 0, 4, false, u32::MAX, &mut || Ok(())).expect("seed");
        let mut covered = 0;
        let result = Provenance::call_mapping(
            &mut engine,
            ProvenanceValue::default(),
            ProvenanceValue::default(),
            &mapping,
            &mut covered,
            &mut || Ok(()),
        )
        .expect("unknown argument");
        assert_eq!(covered, u32::MAX);
        let result = Provenance::call_mapping(
            &mut engine,
            result,
            source,
            &mapping,
            &mut covered,
            &mut || Ok(()),
        )
        .expect("competing argument");
        assert_eq!(result, ProvenanceValue::default());
    }
    #[test]
    fn word_tags_normalize_numeric_addresses_and_keep_width_and_abstract_identity() {
        let memory = WorkingMemory::new(1 << 20).expect("memory");
        let facts = Facts::new(&[], &memory, &mut || Ok(())).expect("facts");
        let mut engine = FlowEngine::<Provenance>::new(&facts, &memory);
        let addresses = [
            AbstractValue::Constant { value: 0x20000 },
            AbstractValue::ImageAddress { address: 0x20000 },
            AbstractValue::Constant { value: 0x20000 },
            AbstractValue::Expression { id: 42 },
            AbstractValue::Expression { id: 42 },
            AbstractValue::Expression { id: 43 },
        ];
        for (id, address) in addresses.iter().enumerate() {
            engine
                .sources
                .push(
                    ReadSource {
                        origin: ReadOrigin::Load {
                            site: id as u64,
                            address,
                            width: if id == 2 { 1 } else { 4 },
                        },
                        preserved: 0,
                        narrowed: [0; 2],
                        limits: [0; 4],
                        event: 0,
                    },
                    RunPosition::default(),
                )
                .expect("source");
        }
        Provenance::prepare_sources(&mut engine, &mut || Ok(())).expect("words");
        let mut seed = |source, width| {
            Provenance::seed(&mut engine, source, width, false, u32::MAX, &mut || Ok(()))
                .expect("word bits")
        };
        let numeric = seed(0, 4);
        assert_eq!(seed(1, 4), numeric);
        assert_ne!(seed(2, 1).bit(0), numeric.bit(0));
        let abstract_word = seed(3, 4);
        assert_eq!(seed(4, 4), abstract_word);
        assert_ne!(seed(5, 4), abstract_word);
    }
    #[test]
    fn constant_add_subtract_and_shift_add_keep_low_bits_before_carry_or_borrow() {
        let word = ProvenanceValue(std::array::from_fn(|i| {
            Tag::source(0, i as u8).expect("word")
        }));
        for constant in [0u32, 1, 2, 16, 0x8000_0000, 0xffff_fff0] {
            let low = constant.trailing_zeros() as usize;
            for op in [IntegerOp::Add, IntegerOp::Sub] {
                let result = ProvenanceValue::integer(
                    op,
                    word,
                    ProvenanceValue::constant(constant),
                    Some(constant),
                );
                for i in 0..32 {
                    assert_eq!(
                        result.bit(i),
                        if i < low { word.bit(i) } else { Tag::UNKNOWN },
                        "{op:?}, {constant:#x}, bit {i}"
                    );
                }
            }
            for (op, shift) in [
                (IntegerOp::ShiftAdd1, 1),
                (IntegerOp::ShiftAdd2, 2),
                (IntegerOp::ShiftAdd3, 3),
            ] {
                let result = ProvenanceValue::integer(
                    op,
                    word,
                    ProvenanceValue::constant(constant),
                    Some(constant),
                );
                for i in 0..low {
                    assert_eq!(
                        result.bit(i),
                        if i < shift {
                            Tag::ZERO
                        } else {
                            word.bit(i - shift)
                        }
                    );
                }
            }
        }
    }
    #[test]
    fn a_stack_reload_keeps_forwarded_tags_including_unknown_and_constants() {
        let memory = WorkingMemory::new(1 << 20).expect("memory");
        let facts = Facts::new(&[], &memory, &mut || Ok(())).expect("facts");
        let mut engine = FlowEngine::<Provenance>::new(&facts, &memory);
        let word =
            Provenance::seed(&mut engine, 0, 4, false, u32::MAX, &mut || Ok(())).expect("word");
        let incoming_word = Provenance::seed(&mut engine, 1, 4, false, u32::MAX, &mut || Ok(()))
            .expect("forwarded word");
        for flow in [
            ProvenanceValue::default(),
            ProvenanceValue::constant(0xa5),
            incoming_word,
        ] {
            for forwarded in [0, 0xff, 0x00ff_00ff, u32::MAX] {
                let actual =
                    Provenance::load(&mut engine, flow, 0, 4, false, forwarded, &mut || Ok(()))
                        .expect("reload");
                for bit in 0..32 {
                    assert_eq!(
                        actual.bit(bit),
                        if forwarded & (1 << bit) != 0 {
                            flow.bit(bit)
                        } else {
                            word.bit(bit)
                        }
                    );
                }
            }
        }
        let conflict = Provenance::join(
            &mut engine,
            incoming_word,
            ProvenanceValue::constant(0),
            &mut || Ok(()),
        )
        .expect("different incoming tags");
        let reloaded =
            Provenance::load(&mut engine, conflict, 0, 4, false, u32::MAX, &mut || Ok(()))
                .expect("reload joined byte");
        assert_eq!(reloaded, ProvenanceValue::default());
    }
    #[test]
    fn a_known_loaded_value_retains_its_word_identity_when_value_knowledge_joins_away() {
        let memory = WorkingMemory::new(1 << 20).expect("memory");
        let facts = Facts::new(&[], &memory, &mut || Ok(())).expect("facts");
        let address = AbstractValue::Constant { value: 0x20000 };
        let mut evaluator = StoredProvenanceEvaluator::new(&facts, &memory);
        evaluator
            .engine
            .sources
            .push(
                ReadSource {
                    origin: ReadOrigin::Load {
                        site: 0,
                        address: &address,
                        width: 4,
                    },
                    preserved: 0,
                    narrowed: [0; 2],
                    limits: [0; 4],
                    event: 0,
                },
                RunPosition::default(),
            )
            .expect("word");
        let flow = Provenance::seed(&mut evaluator.engine, 0, 4, false, u32::MAX, &mut || Ok(()))
            .expect("source");
        let known = ReadKnown::constant(0);
        let flow = Provenance::refine(flow, known);
        assert_eq!(Provenance::known(flow, known), known);
        let joined = evaluator
            .engine
            .join_cell(
                ReadCell {
                    flow,
                    known,
                    atom: ReadAtom::Constant(0),
                },
                ReadCell {
                    flow,
                    ..ReadCell::default()
                },
                &mut || Ok(()),
            )
            .expect("join independent knowledge");
        let stored = store(&mut evaluator, joined, None);
        assert!(matches!(
            stored[0].source,
            StoredBitsSource::Load {
                same_word: true,
                ..
            }
        ));
    }
    #[test]
    fn a_known_neutral_word_operand_keeps_the_other_words_bits() {
        let memory = WorkingMemory::new(1 << 20).expect("memory");
        let facts = Facts::new(&[], &memory, &mut || Ok(())).expect("facts");
        let mut engine = FlowEngine::<Provenance>::new(&facts, &memory);
        let a = ReadCell {
            flow: Provenance::seed(&mut engine, 0, 4, false, u32::MAX, &mut || Ok(()))
                .expect("word a"),
            ..ReadCell::default()
        };
        for (op, value) in [
            (IntegerOp::Or, 0),
            (IntegerOp::Xor, 0),
            (IntegerOp::And, u32::MAX),
        ] {
            let b = ReadCell {
                flow: Provenance::seed(&mut engine, 1, 4, false, u32::MAX, &mut || Ok(()))
                    .expect("word b"),
                known: ReadKnown::constant(value),
                atom: ReadAtom::Constant(value),
            };
            for (left, right) in [(a, b), (b, a)] {
                assert_eq!(
                    engine
                        .integer(op, left, right, &mut || Ok(()))
                        .expect("neutral operation")
                        .flow,
                    a.flow
                );
            }
        }
    }
}
