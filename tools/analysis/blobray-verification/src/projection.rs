//! Borrowed reviewed byte-field comparison; no review lookup, memory acquisition or retained mutation.
use super::*;
#[derive(Clone, Copy)]
pub struct ProjectionComparison<'a> {
    pub resolved: &'a ResolvedProjection,
    pub vendor: &'a Invocation,
    pub replacement: &'a Invocation,
}
impl ProjectionComparison<'_> {
    pub(super) fn final_memory(
        self,
        left: &ExecutionObservation,
        right: &ExecutionObservation,
        c: &mut dyn RunControl,
    ) -> Result<(bool, Option<ComparisonDifference>)> {
        let p = &self.resolved.projection;
        let mut known = true;
        for (i, field) in p.fields.iter().enumerate().filter(|(_, f)| f.final_state) {
            c.checkpoint(
                (self.vendor.observe_memory.len() + self.replacement.observe_memory.len()) as u64
                    + 1,
            )?;
            let (ai, ao) = p.final_selection(self.vendor, field, false)?;
            let (bi, bo) = p.final_selection(self.replacement, field, true)?;
            let goals = left.stop.completed() && right.stop.completed();
            known &= goals;
            c.checkpoint(
                (left.final_memory.len().max(1).ilog2()
                    + right.final_memory.len().max(1).ilog2()
                    + 2) as u64,
            )?;
            let mut a = Bytes::new(&left.final_memory, ai as u16, ao);
            let mut b = Bytes::new(&right.final_memory, bi as u16, bo);
            for offset in 0..field.byte_length()? {
                let (a, b) = (a.next(c)?, b.next(c)?);
                match (a, b) {
                    (Some(a), Some(b)) if goals && a != b => {
                        return Ok((
                            known,
                            Some(ComparisonDifference::ProjectedMemory {
                                field: i as u16,
                                offset,
                                vendor: a,
                                replacement: b,
                            }),
                        ));
                    }
                    (Some(_), Some(_)) => (),
                    _ => known = false,
                }
            }
        }
        Ok((known, None))
    }
}
struct Bytes<'a> {
    remaining: &'a [FinalMemoryChunk],
    selection: u16,
    offset: u32,
    validated: bool,
}
impl<'a> Bytes<'a> {
    fn new(chunks: &'a [FinalMemoryChunk], selection: u16, offset: u32) -> Self {
        let i = chunks.partition_point(|c| (c.selection, c.offset) <= (selection, offset));
        Self {
            remaining: &chunks[i.saturating_sub(1)..],
            selection,
            offset,
            validated: false,
        }
    }
    fn next(&mut self, c: &mut dyn RunControl) -> Result<Option<u8>> {
        c.checkpoint(1)?;
        let offset = self.offset;
        self.offset = self
            .offset
            .checked_add(1)
            .ok_or_else(|| Error::new(ErrorCode::Integrity, "projected byte cursor overflow"))?;
        while let Some((chunk, rest)) = self.remaining.split_first() {
            if !self.validated {
                c.checkpoint(MEMORY_CHUNK_BYTES as u64)?;
                chunk.validate()?;
                self.validated = true;
            }
            if chunk.selection < self.selection
                || (chunk.selection == self.selection
                    && offset
                        >= chunk
                            .offset
                            .checked_add(u32::from(chunk.length))
                            .ok_or_else(|| {
                                Error::new(ErrorCode::Integrity, "projected chunk offset overflow")
                            })?)
            {
                self.remaining = rest;
                self.validated = false;
                continue;
            }
            if chunk.selection != self.selection || offset < chunk.offset {
                return Ok(None);
            }
            let byte = (offset - chunk.offset) as usize;
            return Ok((chunk.known & (1 << byte) != 0).then_some(chunk.bytes[byte]));
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn projected_memory_and_control_preserve_order_width_unknowns_and_atomic_outcomes() {
        let id = ArtifactId::of_bytes(b"fixture");
        let object = ObjectId {
            artifact: id.clone(),
            location: ObjectLocation::Standalone,
        };
        let entry = CallEndpoint {
            object: object.clone(),
            symbol: Some(SymbolId {
                object,
                table: SymbolTableKind::Static,
                table_section: 3,
                index: 1,
            }),
            boundary: ReviewedCallBoundary::Code { address: 0x1000 },
        };
        let p = ResolvedProjection {
            id: id.clone(),
            projection: LayoutProjection {
                vendor: LayoutEndpoint {
                    entry: entry.clone(),
                    domains: vec![LayoutDomain {
                        address: 0x3000,
                        length: 8,
                    }],
                },
                replacement: LayoutEndpoint {
                    entry,
                    domains: vec![LayoutDomain {
                        address: 0x5000,
                        length: 16,
                    }],
                },
                fields: vec![LayoutField {
                    count: 1,
                    name: "word".into(),
                    vendor: FieldLocation {
                        domain: 0,
                        offset: 0,
                    },
                    replacement: FieldLocation {
                        domain: 0,
                        offset: 8,
                    },
                    width: 8,
                    final_state: false,
                    timeline: true,
                }],
                branches: vec![
                    BranchPair {
                        vendor: BranchLocation {
                            site: 0x1000,
                            target: 0x1010,
                            fallthrough: 0x1004,
                        },
                        replacement: BranchLocation {
                            site: 0x2000,
                            target: 0x2010,
                            fallthrough: 0x2004,
                        },
                    },
                    BranchPair {
                        vendor: BranchLocation {
                            site: 0x1004,
                            target: 0x1014,
                            fallthrough: 0x1008,
                        },
                        replacement: BranchLocation {
                            site: 0x2004,
                            target: 0x2014,
                            fallthrough: 0x2008,
                        },
                    },
                ],
                applicability: "test".into(),
                reason: "test".into(),
            },
        };
        p.projection.validate().unwrap();
        let input = Invocation {
            entry: 0x1000,
            goal: ExecutionGoal::Return,
            arguments: vec![],
            memory: vec![],
            preload: vec![],
            models: vec![],
            calls: vec![],
            observe_calls: None,
            observe_memory: vec![],
            observe_timeline: TimelineCapture {
                reads: true,
                writes: true,
                atomics: true,
                branches: true,
                written: false,
            },
        };
        let relation = ComparisonRelation {
            effects: None,
            projection: Some(p.id.clone()),
            calls: false,
            returns: ReturnWords {
                low: false,
                high: false,
            },
            events: EventChannels {
                timeline: input.observe_timeline,
                mmio_read: false,
                mmio_write: false,
                fence: false,
                delay: false,
            },
            memory: vec![],
        };
        let memory = |side, transaction: MemoryTransaction| ExecutionEvent::Memory {
            site: if side { 0x2000 } else { 0x1000 },
            transaction,
        };
        let branch = |side, i: usize| {
            let b = if side {
                p.projection.branches[i].replacement
            } else {
                p.projection.branches[i].vendor
            };
            ExecutionEvent::Branch {
                site: b.site,
                target: b.target,
                fallthrough: b.fallthrough,
                taken: true,
            }
        };
        let observation = |events| ExecutionObservation {
            steps: 4,
            stop: ExecutionStop::Returned {
                low: Some(0),
                high: None,
            },
            events,
            models: vec![],
            calls: vec![],
            final_memory: vec![],
            written: vec![],
        };
        let left = observation(vec![
            memory(
                false,
                MemoryTransaction::Read {
                    address: 0x3000,
                    width: 2,
                    value: MemoryReadValue::Known { value: 7 },
                },
            ),
            branch(false, 0),
            memory(
                false,
                MemoryTransaction::ReadModifyWrite {
                    address: 0x3004,
                    order: ExecutionOrdering {
                        acquire: false,
                        release: true,
                    },
                    old: 1,
                    value: 2,
                },
            ),
            branch(false, 1),
        ]);
        let right = observation(vec![
            memory(
                true,
                MemoryTransaction::Read {
                    address: 0x5008,
                    width: 2,
                    value: MemoryReadValue::Known { value: 7 },
                },
            ),
            branch(true, 0),
            memory(
                true,
                MemoryTransaction::ReadModifyWrite {
                    address: 0x500c,
                    order: ExecutionOrdering {
                        acquire: false,
                        release: true,
                    },
                    old: 1,
                    value: 2,
                },
            ),
            branch(true, 1),
        ]);
        let compare = |right: &ExecutionObservation| {
            super::super::compare(
                &left,
                right,
                &relation,
                Some(ProjectionComparison {
                    resolved: &p,
                    vendor: &input,
                    replacement: &input,
                }),
                None,
                &mut || Ok(()),
            )
            .unwrap()
            .verdict
        };
        assert_eq!(compare(&right), ComparisonVerdict::Match);
        for variant in 0..7 {
            let mut changed = right.clone();
            match variant {
                0 => changed.events.swap(1, 3),
                1 => changed.events.swap(0, 2),
                2 => {
                    if let ExecutionEvent::Branch { taken, .. } = &mut changed.events[1] {
                        *taken = false
                    }
                }
                3 => {
                    if let ExecutionEvent::Memory {
                        transaction: MemoryTransaction::Read { width, .. },
                        ..
                    } = &mut changed.events[0]
                    {
                        *width = 4
                    }
                }
                4 => {
                    if let ExecutionEvent::Memory {
                        transaction: MemoryTransaction::ReadModifyWrite { value, .. },
                        ..
                    } = &mut changed.events[2]
                    {
                        *value = 3
                    }
                }
                5 => {
                    if let ExecutionEvent::Memory {
                        transaction: MemoryTransaction::Read { value, .. },
                        ..
                    } = &mut changed.events[0]
                    {
                        *value = MemoryReadValue::Unknown
                    }
                }
                _ => {
                    if let ExecutionEvent::Memory {
                        transaction: MemoryTransaction::Read { address, .. },
                        ..
                    } = &mut changed.events[0]
                    {
                        *address = 0x6000
                    }
                }
            }
            assert_eq!(
                compare(&changed),
                if variant < 5 {
                    ComparisonVerdict::Diff
                } else {
                    ComparisonVerdict::Incomplete
                },
                "variant {variant}"
            );
        }
    }
}
