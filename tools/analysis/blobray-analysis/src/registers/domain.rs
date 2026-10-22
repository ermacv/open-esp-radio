//! Value-domain operations for the shared instruction CFG interpreter.
use super::*;
use std::hash::Hash;

pub(super) trait FlowValue: Copy + Default + Eq + Hash {
    fn constant(value: u32) -> Self;
}
impl FlowValue for usize {
    fn constant(_: u32) -> Self {
        0
    }
}

/// The graph, queue, stack offsets, ABI traversal and budgets belong to
/// FlowEngine. A domain owns only cell contents and their observations.
pub(super) trait Domain<'m>: Sized {
    type Flow: FlowValue;
    type Byte: Copy + Default + Eq + Hash;
    const ALL_LOADS: bool = false;
    const RECOVER: bool = false;
    const EMIT: bool = true;
    const INSTRUCTION_WORK: bool = false;
    fn prepare_sources(
        _e: &mut FlowEngine<'_, '_, 'm, Self>,
        _c: &mut dyn RunControl,
    ) -> Result<()> {
        Ok(())
    }
    fn new(memory: &'m WorkingMemory) -> Self;
    fn entry_source(register: u8, summary: bool) -> bool {
        summary && (10..18).contains(&register)
    }
    fn fact_known(value: &AbstractValue) -> ReadKnown {
        ReadAtom::from_fact(value)
            .constant()
            .map(ReadKnown::constant)
            .unwrap_or_default()
    }
    fn refine(flow: Self::Flow, _known: ReadKnown) -> Self::Flow {
        flow
    }
    fn known(_flow: Self::Flow, known: ReadKnown) -> ReadKnown {
        known
    }
    fn join(
        e: &mut FlowEngine<'_, '_, 'm, Self>,
        a: Self::Flow,
        b: Self::Flow,
        c: &mut dyn RunControl,
    ) -> Result<Self::Flow>;
    fn binary(
        e: &mut FlowEngine<'_, '_, 'm, Self>,
        a: Self::Flow,
        b: Self::Flow,
        op: ReadFlowOp,
        c: &mut dyn RunControl,
    ) -> Result<Self::Flow>;
    fn integer(
        e: &mut FlowEngine<'_, '_, 'm, Self>,
        op: IntegerOp,
        a: ReadCell<Self::Flow>,
        b: ReadCell<Self::Flow>,
        c: &mut dyn RunControl,
    ) -> Result<Self::Flow>;
    fn map(
        e: &mut FlowEngine<'_, '_, 'm, Self>,
        flow: Self::Flow,
        mapping: impl Fn(usize) -> Option<usize>,
        c: &mut dyn RunControl,
    ) -> Result<Self::Flow>;
    fn seed(
        e: &mut FlowEngine<'_, '_, 'm, Self>,
        source: usize,
        width: u8,
        signed: bool,
        outputs: u32,
        c: &mut dyn RunControl,
    ) -> Result<Self::Flow>;
    fn load(
        e: &mut FlowEngine<'_, '_, 'm, Self>,
        flow: Self::Flow,
        source: usize,
        width: u8,
        signed: bool,
        _forwarded: u32,
        c: &mut dyn RunControl,
    ) -> Result<Self::Flow> {
        let seed = Self::seed(e, source, width, signed, u32::MAX, c)?;
        Self::join(e, flow, seed, c)
    }
    fn pack(cell: ReadCell<Self::Flow>) -> Self::Byte;
    fn unpack(byte: Self::Byte) -> ReadCell<Self::Flow>;
    fn call_mapping(
        e: &mut FlowEngine<'_, '_, 'm, Self>,
        result: Self::Flow,
        incoming: Self::Flow,
        mapping: &ReturnedArgument,
        covered: &mut u32,
        c: &mut dyn RunControl,
    ) -> Result<Self::Flow>;
    fn call_constants(_cell: &mut ReadCell<Self::Flow>, _summary: &CallSummary, _register: u8) {}
    fn sink(
        _e: &mut FlowEngine<'_, '_, 'm, Self>,
        _flow: Self::Flow,
        _outputs: u32,
        _sink: ReadSink,
        _c: &mut dyn RunControl,
    ) -> Result<()> {
        Ok(())
    }
    fn limit(
        _e: &mut FlowEngine<'_, '_, 'm, Self>,
        _flow: Self::Flow,
        _reason: ReadLimitKind,
        _c: &mut dyn RunControl,
    ) -> Result<()> {
        Ok(())
    }
    fn source_limit(
        _e: &mut FlowEngine<'_, '_, 'm, Self>,
        _source: usize,
        _bits: u32,
        _limit: ReadLimitKind,
    ) -> Result<()> {
        Ok(())
    }
    fn store(
        _e: &mut FlowEngine<'_, '_, 'm, Self>,
        _flow: Self::Flow,
        _outputs: u32,
        _instruction: ReadInstruction<'_>,
        _kind: MemoryKind,
        _width: u8,
        _c: &mut dyn RunControl,
    ) -> Result<()> {
        Ok(())
    }
    fn consume_argument(
        _e: &mut FlowEngine<'_, '_, 'm, Self>,
        _flow: Self::Flow,
        _consumed: ArgumentConsumption,
        _site: u64,
        _register: u8,
        _c: &mut dyn RunControl,
    ) -> Result<()> {
        Ok(())
    }
    fn return_value(
        _e: &mut FlowEngine<'_, '_, 'm, Self>,
        _flow: Self::Flow,
        _register: u8,
        _c: &mut dyn RunControl,
    ) -> Result<()> {
        Ok(())
    }
}

/// The #317 dependency domain. Source selection, interning, transfer functions
/// and every checkpoint retain their original semantics and work cost.
pub(super) struct Reads<'m> {
    pub(super) flows: ReadSets<'m, ReadDependency>,
    pub(super) events: AdmittedVec<'m, ReadEvent>,
    pub(super) returned: AdmittedVec<'m, arguments::ReturnedDependency>,
}
impl<'m> Domain<'m> for Reads<'m> {
    type Flow = usize;
    type Byte = ReadCell<usize>;
    fn new(memory: &'m WorkingMemory) -> Self {
        Self {
            flows: ReadSets::new(memory),
            events: AdmittedVec::new(memory),
            returned: AdmittedVec::new(memory),
        }
    }
    fn join(
        e: &mut FlowEngine<'_, '_, 'm, Self>,
        a: usize,
        b: usize,
        c: &mut dyn RunControl,
    ) -> Result<usize> {
        e.read_join_flow(a, b, c)
    }
    fn binary(
        e: &mut FlowEngine<'_, '_, 'm, Self>,
        a: usize,
        b: usize,
        op: ReadFlowOp,
        c: &mut dyn RunControl,
    ) -> Result<usize> {
        e.read_binary_flow(a, b, op, c)
    }
    fn integer(
        e: &mut FlowEngine<'_, '_, 'm, Self>,
        op: IntegerOp,
        a: ReadCell<usize>,
        b: ReadCell<usize>,
        c: &mut dyn RunControl,
    ) -> Result<usize> {
        e.read_integer(op, a, b, c)
    }
    fn map(
        e: &mut FlowEngine<'_, '_, 'm, Self>,
        flow: usize,
        mapping: impl Fn(usize) -> Option<usize>,
        c: &mut dyn RunControl,
    ) -> Result<usize> {
        e.read_map_flow(flow, mapping, c)
    }
    fn seed(
        e: &mut FlowEngine<'_, '_, 'm, Self>,
        source: usize,
        width: u8,
        signed: bool,
        _: u32,
        c: &mut dyn RunControl,
    ) -> Result<usize> {
        e.read_seed(source, width, signed, c)
    }
    fn pack(cell: ReadCell<usize>) -> ReadCell<usize> {
        cell
    }
    fn unpack(byte: ReadCell<usize>) -> ReadCell<usize> {
        byte
    }
    fn call_mapping(
        e: &mut FlowEngine<'_, '_, 'm, Self>,
        result: usize,
        incoming: usize,
        mapping: &ReturnedArgument,
        _: &mut u32,
        c: &mut dyn RunControl,
    ) -> Result<usize> {
        e.read_call_mapping(result, incoming, mapping, c)
    }
    fn sink(
        e: &mut FlowEngine<'_, '_, 'm, Self>,
        flow: usize,
        outputs: u32,
        sink: ReadSink,
        c: &mut dyn RunControl,
    ) -> Result<()> {
        e.read_sink(flow, outputs, sink, c)
    }
    fn limit(
        e: &mut FlowEngine<'_, '_, 'm, Self>,
        flow: usize,
        reason: ReadLimitKind,
        c: &mut dyn RunControl,
    ) -> Result<()> {
        e.read_limit(flow, reason, c)
    }
    fn source_limit(
        e: &mut FlowEngine<'_, '_, 'm, Self>,
        source: usize,
        bits: u32,
        limit: ReadLimitKind,
    ) -> Result<()> {
        e.read_source_limit(source, bits, limit)
    }
    fn store(
        e: &mut FlowEngine<'_, '_, 'm, Self>,
        flow: usize,
        outputs: u32,
        instruction: ReadInstruction<'_>,
        kind: MemoryKind,
        width: u8,
        c: &mut dyn RunControl,
    ) -> Result<()> {
        e.read_store(flow, outputs, instruction, kind, width, c)
    }
    fn consume_argument(
        e: &mut FlowEngine<'_, '_, 'm, Self>,
        flow: usize,
        consumed: ArgumentConsumption,
        site: u64,
        register: u8,
        c: &mut dyn RunControl,
    ) -> Result<()> {
        e.read_consume_argument(flow, consumed, site, register, c)
    }
    fn return_value(
        e: &mut FlowEngine<'_, '_, 'm, Self>,
        flow: usize,
        register: u8,
        c: &mut dyn RunControl,
    ) -> Result<()> {
        e.read_return_value(flow, register, c)
    }
}

impl<'f, 'a, 'm> FlowEngine<'f, 'a, 'm, Reads<'m>> {
    fn read_join_flow(&mut self, a: usize, b: usize, c: &mut dyn RunControl) -> Result<usize> {
        if a == b || b == 0 {
            return Ok(a);
        }
        if a == 0 {
            return Ok(b);
        }
        self.read_binary_flow(a, b, ReadFlowOp::Join, c)
    }

    fn read_binary_flow(
        &mut self,
        left: usize,
        right: usize,
        operation: ReadFlowOp,
        c: &mut dyn RunControl,
    ) -> Result<usize> {
        let (mut l, mut r) = (0, 0);
        loop {
            let a = self.domain.flows.values(left)?.get(l).copied();
            let b = self.domain.flows.values(right)?.get(r).copied();
            let load = match (a, b) {
                (Some(a), Some(b)) => a.source.min(b.source),
                (Some(a), _) => a.source,
                (_, Some(b)) => b.source,
                _ => break,
            };
            c.checkpoint(1)?;
            let a = a.filter(|a| a.source == load);
            let b = b.filter(|b| b.source == load);
            if a.is_some() {
                l += 1;
            }
            if b.is_some() {
                r += 1;
            }
            let dependency = match operation {
                ReadFlowOp::Integer(op, x, y) => read_operation(
                    op,
                    x,
                    y,
                    a.unwrap_or_else(|| ReadDependency::empty(load)),
                    b.unwrap_or_else(|| ReadDependency::empty(load)),
                ),
                ReadFlowOp::Join => match (a, b) {
                    (Some(a), Some(b)) => a.join(b),
                    (Some(a), _) => a,
                    (_, Some(b)) => b,
                    _ => return Err(read_integrity()),
                },
                ReadFlowOp::Assemble => match (a, b) {
                    (Some(mut a), Some(b)) => {
                        for ((exact, may), (e, m)) in a
                            .exact
                            .iter_mut()
                            .zip(&mut a.may)
                            .zip(b.exact.iter().zip(&b.may))
                        {
                            *exact |= e;
                            *may |= m;
                        }
                        a.inverted |= b.inverted;
                        a
                    }
                    (Some(a), _) => a,
                    (_, Some(b)) => b,
                    _ => return Err(read_integrity()),
                },
            };
            if dependency.active() {
                self.domain.flows.scratch.push(dependency, c.position())?;
            }
        }
        self.domain.flows.intern(c)
    }

    fn read_map_flow(
        &mut self,
        flow: usize,
        mapping: impl Fn(usize) -> Option<usize>,
        c: &mut dyn RunControl,
    ) -> Result<usize> {
        let len = self.domain.flows.values(flow)?.len();
        for index in 0..len {
            c.checkpoint(1)?;
            let dependency = self
                .domain
                .flows
                .values(flow)?
                .get(index)
                .copied()
                .ok_or_else(read_integrity)?
                .wire(&mapping);
            if dependency.active() {
                self.domain.flows.scratch.push(dependency, c.position())?;
            }
        }
        self.domain.flows.intern(c)
    }

    fn read_seed(
        &mut self,
        source: usize,
        width: u8,
        signed: bool,
        c: &mut dyn RunControl,
    ) -> Result<usize> {
        let count = u32::from(width).saturating_mul(8).min(32) as usize;
        let mut dependency = ReadDependency::empty(source);
        for (i, bits) in dependency.exact.iter_mut().enumerate() {
            if i < count {
                *bits = 1 << i;
            } else if signed && count != 0 {
                *bits = 1 << (count - 1);
            }
        }
        self.domain.flows.scratch.push(dependency, c.position())?;
        self.domain.flows.intern(c)
    }

    fn read_store(
        &mut self,
        flow: usize,
        outputs: u32,
        instruction: ReadInstruction<'_>,
        kind: MemoryKind,
        width: u8,
        c: &mut dyn RunControl,
    ) -> Result<()> {
        let len = self.domain.flows.values(flow)?.len();
        for index in 0..len {
            c.checkpoint(1)?;
            let dependency = self
                .domain
                .flows
                .values(flow)?
                .get(index)
                .copied()
                .ok_or_else(read_integrity)?;
            let source = self
                .sources
                .get(dependency.source)
                .ok_or_else(read_integrity)?;
            let same = instruction
                .address
                .is_some_and(|address| match source.origin {
                    ReadOrigin::Load {
                        address: loaded,
                        width: loaded_width,
                        ..
                    } => same_word(loaded, loaded_width, address, width),
                    ReadOrigin::Entry(_) => false,
                });
            let mut preserved = 0;
            if same && kind == MemoryKind::Store {
                for (i, (&exact, &may)) in dependency.exact.iter().zip(&dependency.may).enumerate()
                {
                    if exact == 1 << i && may == 0 && dependency.inverted & (1 << i) == 0 {
                        preserved |= (1 << i) & outputs;
                    }
                }
            }
            self.sources
                .get_mut(dependency.source)
                .ok_or_else(read_integrity)?
                .preserved |= preserved;
            self.use_dependency(
                dependency,
                outputs & !preserved,
                ReadSink::Store {
                    site: instruction.site,
                    address: instruction.address.and_then(exact_address),
                    same_word: same,
                },
                c,
            )?;
        }
        Ok(())
    }

    fn read_source_limit(&mut self, source: usize, bits: u32, limit: ReadLimitKind) -> Result<()> {
        let index = match limit {
            ReadLimitKind::PartialFunction => 0,
            ReadLimitKind::StackSpill => 1,
            ReadLimitKind::UnsupportedOperation => 2,
            ReadLimitKind::WorkLimit => 3,
        };
        if let Some(slot) = self
            .sources
            .get_mut(source)
            .and_then(|source| source.limits.get_mut(index))
        {
            *slot |= bits;
            Ok(())
        } else {
            Err(read_integrity())
        }
    }

    fn read_limit(
        &mut self,
        flow: usize,
        reason: ReadLimitKind,
        c: &mut dyn RunControl,
    ) -> Result<()> {
        let len = self.domain.flows.values(flow)?.len();
        for index in 0..len {
            c.checkpoint(1)?;
            let dependency = self
                .domain
                .flows
                .values(flow)?
                .get(index)
                .copied()
                .ok_or_else(read_integrity)?;
            let (a, b) = dependency.selected(u32::MAX);
            self.read_source_limit(dependency.source, a | b, reason)?;
        }
        Ok(())
    }

    fn read_sink(
        &mut self,
        flow: usize,
        outputs: u32,
        sink: ReadSink,
        c: &mut dyn RunControl,
    ) -> Result<()> {
        let len = self.domain.flows.values(flow)?.len();
        for index in 0..len {
            c.checkpoint(1)?;
            let dependency = self
                .domain
                .flows
                .values(flow)?
                .get(index)
                .copied()
                .ok_or_else(read_integrity)?;
            self.use_dependency(dependency, outputs, sink.clone(), c)?;
        }
        Ok(())
    }

    fn use_dependency(
        &mut self,
        dependency: ReadDependency,
        outputs: u32,
        sink: ReadSink,
        c: &mut dyn RunControl,
    ) -> Result<()> {
        let (exact, may) = dependency.selected(outputs);
        for (bits, precision) in [(exact, ReadPrecision::Exact), (may, ReadPrecision::May)] {
            if bits == 0 {
                continue;
            }
            let head = self
                .sources
                .get(dependency.source)
                .ok_or_else(read_integrity)?
                .event;
            let mut event = head;
            let mut found = false;
            while event != 0 {
                c.checkpoint(1)?;
                let existing = self
                    .domain
                    .events
                    .get_mut(event - 1)
                    .ok_or_else(read_integrity)?;
                if existing.usage.sink == sink && existing.usage.precision == precision {
                    existing.usage.bits |= bits;
                    found = true;
                    break;
                }
                event = existing.next;
            }
            if !found {
                self.domain.events.push(
                    ReadEvent {
                        usage: ReadUse {
                            bits,
                            sink: sink.clone(),
                            precision,
                        },
                        next: head,
                    },
                    c.position(),
                )?;
                self.sources
                    .get_mut(dependency.source)
                    .ok_or_else(read_integrity)?
                    .event = self.domain.events.len();
            }
        }
        Ok(())
    }

    pub fn reads(
        &self,
        offset: u64,
        _address: &AbstractValue,
        width: u8,
        _access: MemoryKind,
        c: &mut dyn RunControl,
    ) -> Result<ReadUses> {
        let mask = width_mask(width).unwrap_or(u32::MAX);
        let mut result = ReadUses {
            uses: Vec::new(),
            unknown: Vec::new(),
            preserved: 0,
            unused: 0,
        };
        let source = self.source_at(offset).and_then(|id| self.sources.get(id));
        let mut event = source.map(|source| source.event).unwrap_or(0);
        while event != 0 {
            c.checkpoint(1)?;
            let entry = self
                .domain
                .events
                .get(event - 1)
                .ok_or_else(read_integrity)?;
            result.uses.push(entry.usage.clone());
            event = entry.next;
        }
        result.uses.sort_unstable();
        let used = result.uses.iter().fold(0, |bits, usage| bits | usage.bits);
        let limits = source.map(|source| source.limits).unwrap_or_default();
        let mut remaining = mask & !used;
        for (bits, reason) in [
            (
                limits.get(1).copied().unwrap_or(0),
                ReadLimitKind::StackSpill,
            ),
            (
                limits.get(2).copied().unwrap_or(0),
                ReadLimitKind::UnsupportedOperation,
            ),
            (
                if self.work_limit { mask } else { 0 },
                ReadLimitKind::WorkLimit,
            ),
            (
                limits.first().copied().unwrap_or(0) | if source.is_none() { mask } else { 0 },
                ReadLimitKind::PartialFunction,
            ),
        ] {
            let bits = bits & remaining;
            if bits != 0 {
                result.unknown.push(ReadLimit {
                    bits,
                    limit: reason,
                });
                remaining &= !bits;
            }
        }
        result.preserved = source.map(|source| source.preserved).unwrap_or(0) & remaining;
        result.unused = remaining & !result.preserved;
        Ok(result)
    }

    fn read_integer(
        &mut self,
        op: IntegerOp,
        left: ReadCell<usize>,
        right: ReadCell<usize>,
        c: &mut dyn RunControl,
    ) -> Result<usize> {
        let flow = if left.flow == 0 && right.flow == 0 {
            0
        } else if matches!(op, IntegerOp::Add | IntegerOp::Sub)
            && right.atom == ReadAtom::Constant(0)
        {
            left.flow
        } else if op == IntegerOp::Add && left.atom == ReadAtom::Constant(0) {
            right.flow
        } else {
            self.read_binary_flow(
                left.flow,
                right.flow,
                ReadFlowOp::Integer(op, left.known, right.known),
                c,
            )?
        };
        Ok(flow)
    }

    fn read_consume_argument(
        &mut self,
        flow: usize,
        consumed: ArgumentConsumption,
        site: u64,
        register: u8,
        c: &mut dyn RunControl,
    ) -> Result<()> {
        let sink = ReadSink::CallArgument {
            site,
            register,
            callee: None,
        };
        self.read_sink(flow, consumed.exact, sink.clone(), c)?;
        let len = self.domain.flows.values(flow)?.len();
        for index in 0..len {
            c.checkpoint(1)?;
            let mut dependency = *self
                .domain
                .flows
                .values(flow)?
                .get(index)
                .ok_or_else(read_integrity)?;
            for (exact, may) in dependency.exact.iter_mut().zip(&mut dependency.may) {
                *may |= *exact;
                *exact = 0;
            }
            self.use_dependency(dependency, consumed.may, sink.clone(), c)?;
        }
        Ok(())
    }

    fn read_return_value(
        &mut self,
        flow: usize,
        register: u8,
        c: &mut dyn RunControl,
    ) -> Result<()> {
        if !self.summary_mode {
            return self.read_sink(flow, u32::MAX, ReadSink::Return { register }, c);
        }
        let len = self.domain.flows.values(flow)?.len();
        for index in 0..len {
            c.checkpoint(1)?;
            let dependency = *self
                .domain
                .flows
                .values(flow)?
                .get(index)
                .ok_or_else(read_integrity)?;
            if let Some(existing) = self
                .domain
                .returned
                .iter_mut()
                .find(|r| r.register == register && r.dependency.source == dependency.source)
            {
                existing.dependency = existing.dependency.join(dependency);
            } else {
                self.domain.returned.push(
                    ReturnedDependency {
                        register,
                        dependency,
                    },
                    c.position(),
                )?;
            }
        }
        Ok(())
    }

    fn read_call_mapping(
        &mut self,
        result: usize,
        flow: usize,
        mapping: &ReturnedArgument,
        c: &mut dyn RunControl,
    ) -> Result<usize> {
        let len = self.domain.flows.values(flow)?.len();
        for index in 0..len {
            let incoming = *self
                .domain
                .flows
                .values(flow)?
                .get(index)
                .ok_or_else(read_integrity)?;
            let mut dependency = ReadDependency::empty(incoming.source);
            for (output, ((exact, may), (&map_exact, &map_may))) in dependency
                .exact
                .iter_mut()
                .zip(&mut dependency.may)
                .zip(mapping.exact.iter().zip(&mapping.may))
                .enumerate()
            {
                c.checkpoint(1)?;
                let (e, m) = incoming.selected(map_exact);
                let (a, b) = incoming.selected(map_may);
                *may = m | a | b;
                *exact = e & !*may;
                if map_exact.count_ones() == 1 && map_may == 0 {
                    let bit = map_exact.trailing_zeros();
                    let inverted = (incoming.inverted >> bit) ^ (mapping.inverted >> output);
                    dependency.inverted |= (inverted & 1) << output;
                }
            }
            if dependency.active() {
                self.domain.flows.scratch.push(dependency, c.position())?;
            }
        }
        let composed = self.domain.flows.intern(c)?;
        self.read_binary_flow(result, composed, ReadFlowOp::Assemble, c)
    }
}
