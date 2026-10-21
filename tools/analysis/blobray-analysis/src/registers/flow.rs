//! Compact storage and widening of the CFG engine's dependency cells.
use super::*;

/// An internal unknown provenance contribution, never a read source/event.
const UNKNOWN_SOURCE: usize = usize::MAX;
const SOURCE_LIMIT: usize = 8;
const PRECISE: usize = 1 << (usize::BITS - 1);

#[derive(Clone, Copy, Eq, PartialEq, Hash)]
enum Wiring {
    Read(usize),
    /// Zero means absent; 1..=32 names the unchanged input bit.
    Provenance([u8; 32]),
    /// Widening forgets output positions but retains every participating bit.
    May(u32),
}
#[derive(Clone, Copy, Eq, PartialEq, Hash)]
struct Entry {
    source: usize,
    wiring: Wiring,
}

/// Only selected sources retain full dependency matrices. Provenance-only
/// sources occupy one byte per output bit; widened sources occupy one mask.
/// Interning still admits every allocation and shares immutable cells.
pub(super) struct ReadFlows<'m> {
    entries: ReadSets<'m, Entry>,
    precise: ReadSets<'m, ReadDependency>,
    pub scratch: AdmittedVec<'m, ReadDependency>,
}
impl<'m> ReadFlows<'m> {
    pub fn new(memory: &'m WorkingMemory) -> Self {
        Self {
            entries: ReadSets::new(memory),
            precise: ReadSets::new(memory),
            scratch: AdmittedVec::new(memory),
        }
    }
    pub fn values(&self, id: usize) -> Result<Dependencies<'_, 'm>> {
        Ok(Dependencies {
            entries: if id & PRECISE == 0 {
                self.entries.values(id)?
            } else {
                &[]
            },
            direct: if id & PRECISE != 0 {
                self.precise.values(id & !PRECISE)?
            } else {
                &[]
            },
            precise: &self.precise,
        })
    }
    pub fn observed(&self, id: usize) -> Result<bool> {
        if id == 0 {
            return Ok(false);
        }
        if id & PRECISE != 0 {
            return Ok(true);
        }
        Ok(self
            .entries
            .values(id)?
            .iter()
            .any(|entry| !matches!(entry.wiring, Wiring::Provenance(_))))
    }
    pub fn intern(&mut self, sources: &[ReadSource<'_>], c: &mut dyn RunControl) -> Result<usize> {
        // The common selected-only cell keeps the original single interning
        // operation, without a second level of matrix interning or conversion.
        if self.scratch.len() <= SOURCE_LIMIT
            && self.scratch.iter().all(|d| {
                d.source != UNKNOWN_SOURCE
                    && sources.get(d.source).is_some_and(|s| s.observed)
                    && !(d.exact == [0; 32] && d.may.iter().all(|m| *m == d.may[0]))
            })
        {
            for dependency in self.scratch.iter().copied() {
                self.precise.scratch.push(dependency, c.position())?;
            }
            self.scratch.truncate(0);
            let id = self.precise.intern(c)?;
            return Ok(if id == 0 { 0 } else { id | PRECISE });
        }
        let count = self
            .scratch
            .iter()
            .filter(|d| d.source != UNKNOWN_SOURCE)
            .count();
        let widen = count > SOURCE_LIMIT;
        let mut seen = 0;
        let mut conflicts = 0;
        let mut unknown = 0;
        for d in self.scratch.iter() {
            for i in 0..32 {
                let bit = 1 << i;
                if d.provenance & bit == 0 || d.exact[i] | d.may[i] == 0 {
                    continue;
                }
                conflicts |= seen & bit;
                seen |= bit;
                if d.source == UNKNOWN_SOURCE {
                    unknown |= bit;
                }
            }
        }
        for d in self.scratch.iter().copied() {
            c.checkpoint(1)?;
            if d.source == UNKNOWN_SOURCE {
                continue;
            }
            let observed = sources.get(d.source).ok_or_else(read_integrity)?.observed;
            let wiring = if observed {
                let (_, may) = d.selected(u32::MAX);
                let coarse = d.exact == [0; 32] && d.may.iter().all(|x| *x == may);
                if widen || coarse {
                    let (exact, may) = d.selected(u32::MAX);
                    Wiring::May(exact | may)
                } else {
                    self.precise.scratch.push(d, c.position())?;
                    Wiring::Read(self.precise.intern(c)?)
                }
            } else {
                let mut bits = [0; 32];
                for (i, output) in bits.iter_mut().enumerate() {
                    let bit = 1 << i;
                    if d.provenance & bit == 0 || d.exact[i] | d.may[i] == 0 {
                        continue;
                    }
                    if widen
                        || conflicts & bit != 0
                        || d.may[i] != 0
                        || d.inverted & bit != 0
                        || d.exact[i].count_ones() != 1
                    {
                        unknown |= bit;
                    } else {
                        *output = d.exact[i].trailing_zeros() as u8 + 1;
                    }
                }
                if bits == [0; 32] {
                    continue;
                }
                Wiring::Provenance(bits)
            };
            self.entries.scratch.push(
                Entry {
                    source: d.source,
                    wiring,
                },
                c.position(),
            )?;
        }
        if unknown != 0 {
            self.entries.scratch.push(
                Entry {
                    source: UNKNOWN_SOURCE,
                    wiring: Wiring::Provenance(std::array::from_fn(|i| {
                        u8::from(unknown & (1 << i) != 0)
                    })),
                },
                c.position(),
            )?;
        }
        self.scratch.truncate(0);
        self.entries.intern(c)
    }
}

pub(super) struct Dependencies<'a, 'm> {
    entries: &'a [Entry],
    direct: &'a [ReadDependency],
    precise: &'a ReadSets<'m, ReadDependency>,
}
impl Dependencies<'_, '_> {
    pub fn len(&self) -> usize {
        self.entries.len() + self.direct.len()
    }
    pub fn get(&self, index: usize) -> Option<ReadDependency> {
        if !self.direct.is_empty() {
            return self.direct.get(index).copied();
        }
        let entry = self.entries.get(index)?;
        let mut dependency = ReadDependency::empty(entry.source);
        match entry.wiring {
            Wiring::Read(id) => return self.precise.values(id).ok()?.first().copied(),
            Wiring::May(bits) => {
                dependency.may.fill(bits);
                dependency.provenance = u32::MAX;
            }
            Wiring::Provenance(bits) => {
                for (i, bit) in bits.into_iter().enumerate() {
                    if bit != 0 {
                        dependency.exact[i] = 1 << (bit - 1);
                        dependency.provenance |= 1 << i;
                    }
                }
            }
        }
        Some(dependency)
    }
}
impl ReadDependency {
    pub(super) fn is_unknown(&self) -> bool {
        self.source == UNKNOWN_SOURCE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source<'a>(site: u64, address: &'a AbstractValue, observed: bool) -> ReadSource<'a> {
        ReadSource {
            origin: ReadOrigin::Load {
                site,
                address,
                width: 4,
            },
            observed,
            preserved: 0,
            limits: [0; 4],
            event: 0,
        }
    }
    fn store<'a>(
        reads: &mut ReadUsesEvaluator<'_, 'a, '_>,
        cell: ReadCell,
        value: Option<&AbstractValue>,
    ) -> Vec<StoredBits> {
        let mut input = ReadState::entry();
        input.set(6, cell);
        if reads.nodes.is_empty() {
            reads
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
            reads.nodes[0].input = Some(input);
        }
        reads.prepared = true;
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
    fn provenance_only_sources_drop_inversion_may_and_competing_sources_without_read_events() {
        let memory = WorkingMemory::new(1 << 20).expect("memory");
        let facts = Facts::new(&[], &memory, &mut || Ok(())).expect("facts");
        let address = AbstractValue::Constant { value: 0x20000 };
        let mut reads = ReadUsesEvaluator::new(&facts, &memory);
        let mut c = || Ok(());
        for site in 0..2 {
            reads
                .sources
                .push(source(site, &address, false), c.position())
                .expect("source");
        }
        let cell = ReadCell {
            flow: reads.seed(0, 4, false, u32::MAX, &mut c).expect("seed"),
            ..ReadCell::default()
        };
        assert!(matches!(
            store(&mut reads, cell, None)[0].source,
            StoredBitsSource::Load {
                low: 0,
                same_word: true,
                ..
            }
        ));
        reads
            .sink(
                cell.flow,
                u32::MAX,
                ReadSink::Return { register: 10 },
                &mut c,
            )
            .expect("unobserved sink");
        assert!(reads.events.is_empty());
        for op in [IntegerOp::Xor, IntegerOp::Mul] {
            let right = if op == IntegerOp::Xor {
                ReadCell::constant(u32::MAX)
            } else {
                ReadCell::constant(3)
            };
            let altered = reads.integer(op, cell, right, &mut c).expect("operation");
            assert_eq!(
                store(&mut reads, altered, None)[0].source,
                StoredBitsSource::Unknown
            );
        }
        let other = reads
            .seed(1, 4, false, u32::MAX, &mut c)
            .expect("other seed");
        let joined = reads.join_flow(cell.flow, other, &mut c).expect("join");
        assert_eq!(
            store(
                &mut reads,
                ReadCell {
                    flow: joined,
                    ..cell
                },
                None
            )[0]
            .source,
            StoredBitsSource::Unknown
        );
    }
    #[test]
    fn widening_keeps_all_selected_source_bits_as_may_and_keeps_known_bits() {
        let memory = WorkingMemory::new(1 << 20).expect("memory");
        let facts = Facts::new(&[], &memory, &mut || Ok(())).expect("facts");
        let address = AbstractValue::Constant { value: 0x20000 };
        let mut reads = ReadUsesEvaluator::new(&facts, &memory);
        let mut c = || Ok(());
        let mut flow = 0;
        for site in 0..SOURCE_LIMIT + 1 {
            reads
                .sources
                .push(source(site as u64, &address, true), c.position())
                .expect("source");
            let seed = reads.seed(site, 4, false, u32::MAX, &mut c).expect("seed");
            flow = reads.join_flow(flow, seed, &mut c).expect("union");
        }
        reads
            .sink(flow, 1, ReadSink::Branch { site: 99 }, &mut c)
            .expect("widened sink");
        for site in 0..SOURCE_LIMIT + 1 {
            let result = reads
                .reads(site as u64, &address, 4, MemoryKind::Load, &mut c)
                .expect("read");
            assert_eq!(
                result.uses,
                [ReadUse {
                    bits: u32::MAX,
                    sink: ReadSink::Branch { site: 99 },
                    precision: ReadPrecision::May
                }]
            );
        }
        let cell = ReadCell {
            flow,
            atom: ReadAtom::Unknown,
            known: ReadKnown { zero: 0xff, one: 0 },
        };
        let runs = store(&mut reads, cell, None);
        assert_eq!(
            runs[0],
            StoredBits {
                low: 0,
                width: 8,
                source: StoredBitsSource::Constant { value: 0 }
            }
        );
        assert_eq!(runs[1].source, StoredBitsSource::Unknown);
    }
    #[test]
    fn a_limit_invalidates_only_the_forward_closure_and_literal_facts_survive() {
        let memory = WorkingMemory::new(1 << 20).expect("memory");
        let facts = Facts::new(&[], &memory, &mut || Ok(())).expect("facts");
        let mut reads = ReadUsesEvaluator::new(&facts, &memory);
        let entry = ReadCell::constant(42);
        store(&mut reads, entry, None);
        let instruction = reads.nodes[0].instruction;
        reads
            .nodes
            .push(
                ReadNode {
                    instruction: ReadInstruction {
                        site: 4,
                        ..instruction
                    },
                    input: reads.nodes[0].input,
                    queued: true,
                    unsettled: false,
                },
                RunPosition::default(),
            )
            .expect("pending store");
        reads
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
        reads.nodes[1].instruction.edge_end = 1;
        reads.work_limit = true;
        reads
            .invalidate_pending(&mut || Ok(()))
            .expect("pending closure");
        assert!(!reads.nodes[0].unsettled);
        assert!(reads.nodes[1].unsettled);
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
}
