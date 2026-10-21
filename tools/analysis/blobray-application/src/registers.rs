//! Candidate memory addresses one analyzed function accesses.
use crate::*;
use oer_analysis_blobray_analysis::navigation::Facts;

fn invalid(s: &str) -> Error {
    Error::new(ErrorCode::InvalidRequest, s)
}
/// One candidate address a function's record accesses.
pub(crate) struct Candidate<'a> {
    pub record: u64,
    pub fact: &'a FunctionRecord,
    pub address: Option<u32>,
    pub alternative: Option<u8>,
    /// The progression of an unresolved address computed from one index.
    pub indexed: Option<IndexedAddress>,
    pub mask: Option<RegisterMask>,
    /// The sources of a store's bits.
    pub stored: Option<Vec<StoredBits>>,
}

/// Load-use analysis belongs only to register-accesses. Other consumers of
/// address and expression observations do not run it or change their results.
pub(crate) struct ReadObservations<'f, 'a, 'm> {
    evaluator: oer_analysis_blobray_analysis::registers::ReadUsesEvaluator<'f, 'a, 'm>,
    decoder: &'f dyn FunctionSemantics,
    ranges: &'f [ImageRegion],
    abi: Option<CallAbi>,
}

impl<'f, 'a, 'm> ReadObservations<'f, 'a, 'm> {
    pub fn new(
        facts: &'f Facts<'a, 'm>,
        memory: &'m WorkingMemory,
        decoder: &'f dyn FunctionSemantics,
        ranges: &'f [ImageRegion],
        abi: Option<CallAbi>,
    ) -> Self {
        Self {
            evaluator: oer_analysis_blobray_analysis::registers::ReadUsesEvaluator::new(
                facts, memory,
            ),
            decoder,
            ranges,
            abi,
        }
    }

    pub fn reads(
        &mut self,
        fact: &FunctionRecord,
        c: &mut dyn RunControl,
    ) -> Result<Option<ReadUses>> {
        if let FunctionRecord::MemoryAccess {
            offset,
            access:
                access @ (MemoryKind::Load
                | MemoryKind::LoadReserved
                | MemoryKind::FloatLoad
                | MemoryKind::Atomic),
            address,
            width,
            ..
        } = fact
        {
            self.evaluator.analyze(
                self.abi,
                self.decoder,
                &mut |address, width, c| selected_read(address, width, self.ranges, c),
                c,
            )?;
            self.evaluator
                .reads(*offset, address, *width, *access, c)
                .map(Some)
        } else {
            Ok(None)
        }
    }
}

fn selected_read(
    address: &AbstractValue,
    width: u8,
    ranges: &[ImageRegion],
    c: &mut dyn RunControl,
) -> Result<bool> {
    let mut selected = false;
    candidates(address, c, &mut |address, _, c| {
        c.checkpoint(ranges.len() as u64 + 1)?;
        selected |= address.is_none_or(|address| {
            ranges.is_empty()
                || ranges.iter().any(|range| {
                    u64::from(address) < range.end().unwrap_or(0)
                        && u64::from(range.start) < u64::from(address) + u64::from(width)
                })
        });
        Ok(())
    })?;
    Ok(selected)
}

type Observer<'a> = dyn FnMut(Candidate<'_>, &mut dyn RunControl) -> Result<()> + 'a;

/// Every candidate address the analyzed function `records` access: memory
/// accesses with the bits a read-modify-write replaces, and masked reads.
/// A resolved address outside every range of nonempty `ranges` is skipped.
/// An unresolved address is always kept; it carries its indexed progression
/// when it has one and, with nonempty `ranges`, the progression may reach a
/// range: its base lies in one, or its bounded index reaches one.
pub(crate) fn observe<'a, 'm>(
    records: &'a [FunctionRecord],
    facts: &Facts<'a, 'm>,
    memory: &'m WorkingMemory,
    ranges: &[ImageRegion],
    c: &mut dyn RunControl,
    emit: &mut Observer<'_>,
) -> Result<()> {
    // A store's stored bits are evaluated only once one of its candidates
    // is in range, and once for all of them, so a store the ranges exclude
    // costs no work and no memo.
    let mut evaluator =
        oer_analysis_blobray_analysis::registers::StoredBitsEvaluator::new(facts, memory);
    let mut indexed_addresses =
        oer_analysis_blobray_analysis::registers::IndexedAddresses::new(facts, memory);
    let mut observe = |record: u64,
                       width: u8,
                       address: &'a AbstractValue,
                       mask: Option<RegisterMask>,
                       stored_value: Option<&'a AbstractValue>,
                       c: &mut dyn RunControl| {
        let fact = &records[record as usize];
        let mut stored: Option<Vec<StoredBits>> = None;
        let indexed = match indexed_addresses.address(address, c)? {
            Some(indexed) if in_ranges(&indexed, width, ranges, c)? => Some(indexed),
            _ => None,
        };
        candidates(address, c, &mut |resolved, alternative, c| {
            c.checkpoint(ranges.len() as u64 + 1)?;
            if let Some(resolved) = resolved
                && !ranges.is_empty()
                && !ranges.iter().any(|r| {
                    u64::from(resolved) < r.end().unwrap_or(0)
                        && u64::from(r.start) < u64::from(resolved) + u64::from(width)
                })
            {
                return Ok(());
            }
            if let Some(value) = stored_value
                && stored.is_none()
            {
                stored = Some(oer_analysis_blobray_analysis::registers::stored_bits(
                    &mut evaluator,
                    address,
                    width,
                    value,
                    c,
                )?);
            }
            emit(
                Candidate {
                    record,
                    fact,
                    address: resolved,
                    alternative,
                    indexed: if resolved.is_none() { indexed } else { None },
                    mask,
                    stored: stored.clone(),
                },
                c,
            )
        })
    };
    facts.accesses(c, &mut |access, c| {
        let mask = access.value.and_then(|v| {
            oer_analysis_blobray_analysis::registers::write_mask(
                facts,
                access.address,
                access.width,
                v,
            )
        });
        let stored_value = access
            .value
            .filter(|_| matches!(access.access, MemoryKind::Store));
        observe(
            access.record,
            access.width,
            access.address,
            mask,
            stored_value,
            c,
        )
    })?;
    for (record, fact) in records.iter().enumerate() {
        c.checkpoint(1)?;
        if let FunctionRecord::Expression { expression, .. } = fact
            && let Some((address, width, mask)) =
                oer_analysis_blobray_analysis::registers::read_mask(facts, expression)
        {
            observe(record as u64, width, address, Some(mask), None, c)?;
        }
    }
    Ok(())
}

/// Index values of a bounded progression checked against the ranges; a
/// longer bound is judged by its base alone.
const MAX_CHECKED_INDICES: u32 = 4096;

/// Whether the `width`-byte accesses of `indexed` may reach a range of
/// nonempty `ranges`: its base does, or, for a bound up to
/// [`MAX_CHECKED_INDICES`], the address at some index does.
fn in_ranges(
    indexed: &IndexedAddress,
    width: u8,
    ranges: &[ImageRegion],
    c: &mut dyn RunControl,
) -> Result<bool> {
    let hits = |address: u32| {
        ranges.iter().any(|r| {
            u64::from(address) < r.end().unwrap_or(0)
                && u64::from(r.start) < u64::from(address) + u64::from(width)
        })
    };
    if ranges.is_empty() || hits(indexed.base) {
        return Ok(true);
    }
    match indexed.count {
        Some(count) if count <= MAX_CHECKED_INDICES => {
            c.checkpoint(u64::from(count) * (ranges.len() as u64 + 1))?;
            Ok((1..count).any(|i| hits(indexed.at(i))))
        }
        _ => Ok(false),
    }
}

/// Validate explicit candidate intervals: at most 256, each nonempty and
/// inside the address space.
pub(crate) fn validate_ranges(ranges: &[ImageRegion]) -> Result<()> {
    if ranges.len() > 256 || ranges.iter().any(|r| r.length == 0 || r.end().is_none()) {
        return Err(invalid(
            "register query requires at most 256 valid address intervals",
        ));
    }
    Ok(())
}

type CandidateSink<'a> = dyn FnMut(Option<u32>, Option<u8>, &mut dyn RunControl) -> Result<()> + 'a;
fn candidates(
    value: &AbstractValue,
    c: &mut dyn RunControl,
    emit: &mut CandidateSink<'_>,
) -> Result<()> {
    match value {
        AbstractValue::Constant { value } | AbstractValue::ImageAddress { address: value } => {
            emit(Some(*value), None, c)
        }
        AbstractValue::Alternatives { values } => {
            for (i, value) in values.values().iter().enumerate() {
                let address = match value {
                    ValueAlternative::Constant { value }
                    | ValueAlternative::ImageAddress { address: value } => Some(*value),
                    _ => None,
                };
                emit(address, Some(i as u8), c)?;
            }
            Ok(())
        }
        _ => emit(None, None, c),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NODES: u32 = 40;

    /// `x0 = a0; x(i+1) = x(i) + x(i)` and a store of `value` to 0x20000.
    fn records(value: AbstractValue) -> Vec<FunctionRecord> {
        let mut records: Vec<FunctionRecord> = (0..NODES)
            .map(|id| FunctionRecord::Expression {
                id,
                offset: 0,
                expression: if id == 0 {
                    Expression::EntryRegister { register: 10 }
                } else {
                    Expression::Integer {
                        op: IntegerOp::Add,
                        left: AbstractValue::Expression { id: id - 1 },
                        right: AbstractValue::Expression { id: id - 1 },
                    }
                },
            })
            .collect();
        records.push(FunctionRecord::MemoryAccess {
            offset: 4,
            access: MemoryKind::Store,
            width: 4,
            address: AbstractValue::Constant { value: 0x20000 },
            value: Some(value),
            relocation: None,
        });
        records
    }

    /// The checkpoints `observe` takes over `records` with `ranges`.
    fn work(records: &[FunctionRecord], ranges: &[ImageRegion]) -> u64 {
        let memory = WorkingMemory::new(1 << 20).unwrap();
        let facts = Facts::new(records, &memory, &mut || Ok(())).unwrap();
        let mut checkpoints = 0;
        let mut count = || {
            checkpoints += 1;
            Ok(())
        };
        observe(records, &facts, &memory, ranges, &mut count, &mut |_, _| {
            Ok(())
        })
        .unwrap();
        checkpoints
    }

    #[test]
    fn a_store_outside_the_ranges_evaluates_no_stored_bits() {
        let deep = records(AbstractValue::Expression { id: NODES - 1 });
        let flat = records(AbstractValue::Constant { value: 3 });
        let outside = [ImageRegion {
            start: 0x40000,
            length: 4,
        }];
        assert_eq!(
            work(&deep, &outside),
            work(&flat, &outside),
            "an excluded store's value is never walked"
        );
        let inside = [ImageRegion {
            start: 0x20000,
            length: 4,
        }];
        assert!(
            work(&deep, &inside) >= work(&flat, &inside) + u64::from(NODES),
            "an included store's value is walked, one checkpoint per node"
        );
    }

    /// The `(address, indexed)` of every candidate `observe` emits for a
    /// store to `(a0 & mask) << 4 + base` (no mask: `a0 << 4 + base`) and a
    /// store to the constant 0x20000, with `ranges`.
    fn indexed_candidates(
        mask: Option<u32>,
        base: u32,
        ranges: &[ImageRegion],
    ) -> Vec<(Option<u32>, Option<IndexedAddress>)> {
        let mut expressions = vec![Expression::EntryRegister { register: 10 }];
        if let Some(mask) = mask {
            expressions.push(Expression::Integer {
                op: IntegerOp::And,
                left: AbstractValue::Expression { id: 0 },
                right: AbstractValue::Constant { value: mask },
            });
        }
        let index = expressions.len() as u32 - 1;
        expressions.push(Expression::Integer {
            op: IntegerOp::Shl,
            left: AbstractValue::Expression { id: index },
            right: AbstractValue::Constant { value: 4 },
        });
        expressions.push(Expression::Integer {
            op: IntegerOp::Add,
            left: AbstractValue::Expression { id: index + 1 },
            right: AbstractValue::Constant { value: base },
        });
        let address = AbstractValue::Expression { id: index + 2 };
        let mut records: Vec<FunctionRecord> = expressions
            .into_iter()
            .enumerate()
            .map(|(id, expression)| FunctionRecord::Expression {
                id: id as u32,
                offset: 0,
                expression,
            })
            .collect();
        for address in [address, AbstractValue::Constant { value: 0x20000 }] {
            records.push(FunctionRecord::MemoryAccess {
                offset: 4,
                access: MemoryKind::Store,
                width: 4,
                address,
                value: Some(AbstractValue::Constant { value: 1 }),
                relocation: None,
            });
        }
        let memory = WorkingMemory::new(1 << 20).unwrap();
        let facts = Facts::new(&records, &memory, &mut || Ok(())).unwrap();
        let mut candidates = Vec::new();
        observe(
            &records,
            &facts,
            &memory,
            ranges,
            &mut || Ok(()),
            &mut |c, _| {
                candidates.push((c.address, c.indexed));
                Ok(())
            },
        )
        .unwrap();
        candidates
    }

    #[test]
    fn ranges_keep_a_progression_that_may_reach_them_and_never_drop_its_access() {
        let element_one = [ImageRegion {
            start: 0x2010_f410,
            length: 4,
        }];
        let progression = |count| IndexedAddress {
            base: 0x2010_f400,
            stride: 0x10,
            count,
        };
        // A bounded index reaches the range at index one though its base
        // lies outside; the constant store outside the range is skipped.
        assert_eq!(
            indexed_candidates(Some(7), 0x2010_f400, &element_one),
            [(None, Some(progression(Some(8))))]
        );
        // An unbounded index is judged by its base: outside, the access stays
        // a plain unresolved one.
        assert_eq!(
            indexed_candidates(None, 0x2010_f400, &element_one),
            [(None, None)]
        );
        let base = [ImageRegion {
            start: 0x2010_f400,
            length: 4,
        }];
        assert_eq!(
            indexed_candidates(None, 0x2010_f400, &base),
            [(None, Some(progression(None)))]
        );
        // A bound beyond the checked indices is judged by its base too.
        assert_eq!(
            indexed_candidates(Some(0x1fff), 0x2010_f400, &element_one),
            [(None, None)]
        );
        // Without ranges every progression is kept; a resolved address never
        // carries one.
        assert_eq!(
            indexed_candidates(Some(7), 0x2010_f400, &[]),
            [(None, Some(progression(Some(8)))), (Some(0x20000), None)]
        );
    }
}
