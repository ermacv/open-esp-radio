//! Comparison of explicitly selected concrete observations; no execution/store authority.
use blobray_domain::*;
use oer_riscv_model::*;
mod effects;
mod projection;
mod records;
pub use effects::classify as classify_effects;
pub use projection::ProjectionComparison;
pub use records::{RecordedRun, validate_records};
pub const VERIFIER: &str = "reviewed-effects-projected-timeline-calls-returns-memory/model-10";
pub fn compare(
    left: &ExecutionObservation,
    right: &ExecutionObservation,
    relation: &ComparisonRelation,
    projection: Option<ProjectionComparison<'_>>,
    effects: Option<&ResolvedEffectContract>,
    control: &mut dyn RunControl,
) -> Result<CaseComparison> {
    let effect_report = effects::inspect(left, right, relation, effects, control)?;
    let different = |difference| CaseComparison {
        effect_claim: effect_report.claim,
        effect_gap: effect_report.gap,
        verdict: ComparisonVerdict::Diff,
        difference: Some(difference),
    };
    if relation.projection.as_ref() != projection.map(|p| &p.resolved.id) {
        return Err(Error::new(
            ErrorCode::Integrity,
            "selected projection differs from comparison input",
        ));
    }
    if let Some(p) = projection {
        control.checkpoint(
            (p.resolved.projection.fields.len() + p.resolved.projection.branches.len() + 1).pow(2)
                as u64,
        )?;
        p.resolved.projection.validate()?;
        relation.validate(p.vendor, p.replacement)?;
    }
    let layout = projection.map(|p| &p.resolved.projection);
    let mut known = effect_report.gap.is_none();
    control.checkpoint((left.events.len() + right.events.len()) as u64)?;
    let mut le = Selected::new(&left.events, relation, layout, effects, false, control)?;
    let mut re = Selected::new(&right.events, relation, layout, effects, true, control)?;
    if let Some(violation) = effect_report.violation {
        return Ok(different(ComparisonDifference::EffectViolation {
            violation,
        }));
    }
    let mut count = 0;
    let (mut a, mut b) = (None, None);
    loop {
        if a.is_none() {
            a = le.next(control)?;
        }
        if b.is_none() {
            b = re.next(control)?;
        }
        // Unknown classification can change alignment; later positions cannot prove a mismatch.
        if matches!(
            a,
            Some(Observation::Effect(_, EffectSelection::Unclassified))
        ) || matches!(
            b,
            Some(Observation::Effect(_, EffectSelection::Unclassified))
        ) {
            known = false;
            break;
        }
        if let Some(Observation::Effect(event, EffectSelection::Omitted(rule))) = a
            && !matches!(b, Some(Observation::Effect(other, EffectSelection::Omitted(id))) if rule == id && event == other)
        {
            a = None;
            continue;
        }
        match (a, b) {
            (Some(a), Some(b)) => {
                control.checkpoint(1)?;
                match (a, b) {
                    (
                        Observation::Call {
                            target: a,
                            arguments: aa,
                        },
                        Observation::Call {
                            target: b,
                            arguments: ba,
                        },
                    ) => {
                        if a != b {
                            return Ok(different(ComparisonDifference::CallTarget {
                                index: count,
                                vendor: a,
                                replacement: b,
                            }));
                        }
                        if aa.len() != ba.len() {
                            return Err(Error::new(
                                ErrorCode::Integrity,
                                "physical call capture widths differ",
                            ));
                        }
                        for (word, (a, b)) in aa.iter().zip(ba).enumerate() {
                            control.checkpoint(1)?;
                            let value = |e: &ExecutionEvent| match e {
                                ExecutionEvent::TransferArgument { value, .. } => value.value(),
                                _ => unreachable!(),
                            };
                            match (value(a), value(b)) {
                                (Some(a), Some(b)) if a != b => {
                                    return Ok(different(ComparisonDifference::CallArgument {
                                        index: count,
                                        word: word as u16,
                                        vendor: a,
                                        replacement: b,
                                    }));
                                }
                                (Some(_), Some(_)) => {}
                                _ => known = false,
                            }
                        }
                    }
                    (Observation::Unmapped, _) | (_, Observation::Unmapped) => known = false,
                    (Observation::ProjectedMemory(af, a), Observation::ProjectedMemory(bf, b))
                        if af == bf =>
                    {
                        match a.equal(b) {
                            Some(true) => (),
                            None => known = false,
                            Some(false) => {
                                return Ok(different(ComparisonDifference::Event { index: count }));
                            }
                        }
                    }
                    (Observation::ProjectedBranch(a, at), Observation::ProjectedBranch(b, bt))
                        if a == b && at == bt => {}
                    (Observation::Memory(a), Observation::Memory(b)) => match a.equal(b) {
                        Some(true) => (),
                        None => known = false,
                        Some(false) => {
                            return Ok(different(ComparisonDifference::Event { index: count }));
                        }
                    },
                    (
                        Observation::Effect(_, EffectSelection::Replaced(a)),
                        Observation::Effect(_, EffectSelection::Replaced(b)),
                    ) if a == b => {}
                    (Observation::Effect(a, sa), Observation::Effect(b, sb))
                        if sa == sb && a == b => {}
                    (Observation::Event(a), Observation::Event(b)) if a == b => {}
                    _ => return Ok(different(ComparisonDifference::Event { index: count })),
                }
                count += 1;
            }
            (None, None) => break,
            (None, Some(_)) if left.stop.completed() => {
                return Ok(different(ComparisonDifference::Event { index: count }));
            }
            (Some(_), None) if right.stop.completed() => {
                return Ok(different(ComparisonDifference::Event { index: count }));
            }
            _ => {
                known = false;
                break;
            }
        }
        a = None;
        b = None;
    }
    for (word, selected) in [relation.returns.low, relation.returns.high]
        .into_iter()
        .enumerate()
    {
        if !selected {
            continue;
        }
        control.checkpoint(1)?;
        let values = |o: &ExecutionObservation| match &o.stop {
            ExecutionStop::Returned { low, high } => [*low, *high][word],
            _ => None,
        };
        match (values(left), values(right)) {
            (Some(a), Some(b)) if a != b => {
                return Ok(different(ComparisonDifference::Return {
                    word: word as u8,
                    vendor: a,
                    replacement: b,
                }));
            }
            (Some(_), Some(_)) => {}
            _ => known = false,
        }
    }
    fn selected(chunks: &[FinalMemoryChunk], selection: u16) -> &[FinalMemoryChunk] {
        let start = chunks.partition_point(|c| c.selection < selection);
        let end = chunks.partition_point(|c| c.selection <= selection);
        &chunks[start..end]
    }
    for (pair, p) in relation.memory.iter().enumerate() {
        control.checkpoint(
            2 * (left.final_memory.len().max(1).ilog2() as u64
                + right.final_memory.len().max(1).ilog2() as u64
                + 2),
        )?;
        let a = selected(&left.final_memory, p.vendor);
        let b = selected(&right.final_memory, p.replacement);
        if a.is_empty() || b.is_empty() || a.len() != b.len() {
            known = false;
            continue;
        }
        // A stopped intermediate RAM state cannot prove a difference in completed final states.
        let at_goals = left.stop.completed() && right.stop.completed();
        known &= at_goals;
        for (a, b) in a.iter().zip(b) {
            control.checkpoint(MEMORY_CHUNK_BYTES as u64)?;
            a.validate()?;
            b.validate()?;
            if a.offset != b.offset || a.length != b.length {
                known = false;
                continue;
            }
            for i in 0..a.length as usize {
                if a.known & b.known & (1 << i) == 0 {
                    known = false;
                    continue;
                }
                if at_goals && a.bytes[i] != b.bytes[i] {
                    return Ok(different(ComparisonDifference::Memory {
                        pair: pair as u16,
                        offset: a.offset + i as u32,
                        vendor: a.bytes[i],
                        replacement: b.bytes[i],
                    }));
                }
            }
        }
    }
    if let Some(projection) = projection {
        let (complete, difference) = projection.final_memory(left, right, control)?;
        known &= complete;
        if let Some(difference) = difference {
            return Ok(different(difference));
        }
    }
    Ok(CaseComparison {
        effect_claim: effect_report.claim,
        effect_gap: effect_report.gap,
        verdict: if known
            && left.completed()
            && right.completed()
            && (std::mem::discriminant(&left.stop) == std::mem::discriminant(&right.stop)
                || prefix(left, right, relation))
        {
            ComparisonVerdict::Match
        } else {
            ComparisonVerdict::Incomplete
        },
        difference: None,
    })
}
/// A vendor prefix up to a symbol boundary compared with a complete
/// replacement, when the relation compares no return or call.
fn prefix(
    left: &ExecutionObservation,
    right: &ExecutionObservation,
    relation: &ComparisonRelation,
) -> bool {
    matches!(
        left.stop,
        ExecutionStop::ObservedCall { .. } | ExecutionStop::ReachedSymbol { .. }
    ) && matches!(right.stop, ExecutionStop::Returned { .. })
        && !relation.returns.low
        && !relation.returns.high
        && !relation.calls
}

/// Replacement observations a comparison under `relation` examines.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ComparedObservations {
    /// Indices of compared replacement events, ascending. A compared call
    /// includes its transfer and argument events.
    pub events: Vec<u32>,
    /// Compared return words, low then high.
    pub returns: [bool; 2],
    /// Compared replacement final memory as `[address, length]`: selected
    /// memory pairs and projected final-state fields.
    pub memory: Vec<[u32; 2]>,
}

/// The replacement observations of `right` that `compare` examines under
/// `relation`, with the same event selection. Events that cannot be mapped
/// through the projection are not compared.
pub fn compared_observations(
    right: &ExecutionObservation,
    replacement: &Invocation,
    relation: &ComparisonRelation,
    projection: Option<&LayoutProjection>,
    effects: Option<&ResolvedEffectContract>,
    control: &mut dyn RunControl,
) -> Result<ComparedObservations> {
    let mut selected = Selected::new(&right.events, relation, projection, effects, true, control)?;
    let mut events = Vec::new();
    while let Some(observation) = selected.next(control)? {
        let after = (selected.total - selected.remaining.len()) as u32;
        match observation {
            Observation::Unmapped => {}
            Observation::Call { arguments, .. } => {
                events.extend(after - 1 - arguments.len() as u32..after);
            }
            _ => events.push(after - 1),
        }
    }
    let mut memory: Vec<[u32; 2]> = relation
        .memory
        .iter()
        .filter_map(|pair| {
            replacement
                .observe_memory
                .get(usize::from(pair.replacement))
        })
        .map(|s| [s.address, s.length])
        .collect();
    if let Some(projection) = projection {
        for field in projection.fields.iter().filter(|f| f.final_state) {
            memory.push([projection.field_address(field, true)?, field.byte_length()?]);
        }
    }
    Ok(ComparedObservations {
        events,
        returns: [relation.returns.low, relation.returns.high],
        memory,
    })
}

#[derive(Clone, Copy)]
enum Observation<'a> {
    Effect(&'a ExecutionEvent, EffectSelection),
    Unmapped,
    ProjectedMemory(u16, MemoryTransaction),
    ProjectedBranch(u16, bool),
    Memory(MemoryTransaction),
    Event(&'a ExecutionEvent),
    Call {
        target: u32,
        arguments: &'a [ExecutionEvent],
    },
}
struct Selected<'a> {
    remaining: &'a [ExecutionEvent],
    relation: &'a ComparisonRelation,
    projection: Option<&'a LayoutProjection>,
    side: bool,
    effects: Option<EffectTracker<'a>>,
    total: usize,
    /// Index of the next concrete effect after the current event; only advances.
    next_effect: usize,
}
impl<'a> Selected<'a> {
    fn new(
        remaining: &'a [ExecutionEvent],
        relation: &'a ComparisonRelation,
        projection: Option<&'a LayoutProjection>,
        effects: Option<&'a ResolvedEffectContract>,
        side: bool,
        c: &mut dyn RunControl,
    ) -> Result<Self> {
        Ok(Self {
            next_effect: 0,
            total: remaining.len(),
            effects: effects
                .map(|e| EffectTracker::new(&e.contract, side, c))
                .transpose()?,
            remaining,
            relation,
            projection,
            side,
        })
    }
    fn next(&mut self, c: &mut dyn RunControl) -> Result<Option<Observation<'a>>> {
        let invalid = || {
            Error::new(
                ErrorCode::Integrity,
                "invalid physical call observation group",
            )
        };
        while let Some((event, rest)) = self.remaining.split_first() {
            self.remaining = rest;
            if let ExecutionEvent::CallTransfer { target, words, .. } = event {
                if usize::from(*words) > MAX_EXECUTION_ARGUMENT_WORDS
                    || rest.len() < usize::from(*words)
                {
                    return Err(invalid());
                }
                let (arguments, rest) = rest.split_at(usize::from(*words));
                if !arguments.iter().enumerate().all(|(i, e)|
                    matches!(e, ExecutionEvent::TransferArgument { word, .. } if usize::from(*word) == i)) {
                    return Err(invalid());
                }
                self.remaining = rest;
                if self.relation.calls {
                    return Ok(Some(Observation::Call {
                        target: *target,
                        arguments,
                    }));
                }
            } else if matches!(event, ExecutionEvent::TransferArgument { .. }) {
                return Err(invalid());
            } else if is_contract_effect(event) && self.effects.is_some() {
                let position = self.total - self.remaining.len() - 1;
                let ordinal = u32::try_from(position)
                    .map_err(|_| Error::new(ErrorCode::Integrity, "effect ordinal overflow"))?;
                // `remaining` starts after this event; each scan ends at the
                // next effect, so the scans cover every event once.
                if self.next_effect <= position {
                    let start = self.total - self.remaining.len();
                    self.next_effect = self
                        .remaining
                        .iter()
                        .position(is_contract_effect)
                        .map_or(self.total, |i| start + i);
                }
                let next = self
                    .next_effect
                    .checked_sub(self.total - self.remaining.len())
                    .and_then(|i| self.remaining.get(i));
                let selection = self
                    .effects
                    .as_mut()
                    .unwrap()
                    .observe(event, next, ordinal, c)?;
                if !matches!(
                    selection,
                    EffectSelection::Added(_) | EffectSelection::Ignored(_)
                ) {
                    return Ok(Some(Observation::Effect(event, selection)));
                }
            } else if self.relation.events.selects(event) {
                return Ok(Some(if let Some(transaction) = event.normal_memory() {
                    transaction.validate()?;
                    if let Some(p) = self.projection {
                        c.checkpoint(p.fields.len() as u64 + 1)?;
                        match p.memory_location(transaction, self.side)? {
                            Some((field, offset)) => {
                                Observation::ProjectedMemory(field, transaction.at_address(offset))
                            }
                            None => Observation::Unmapped,
                        }
                    } else {
                        Observation::Memory(transaction)
                    }
                } else if let (
                    Some(p),
                    ExecutionEvent::Branch {
                        site,
                        target,
                        fallthrough,
                        taken,
                    },
                ) = (self.projection, event)
                {
                    c.checkpoint(p.branches.len() as u64 + 1)?;
                    match p.branch_location(
                        BranchLocation {
                            site: *site,
                            target: *target,
                            fallthrough: *fallthrough,
                        },
                        self.side,
                    ) {
                        Some(id) => Observation::ProjectedBranch(id, *taken),
                        None => Observation::Unmapped,
                    }
                } else {
                    Observation::Event(event)
                }));
            }
        }
        Ok(None)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn compare(
        left: &ExecutionObservation,
        right: &ExecutionObservation,
        compare_return: bool,
        c: &mut dyn RunControl,
    ) -> Result<CaseComparison> {
        super::compare(
            left,
            right,
            &ComparisonRelation {
                effects: None,
                projection: None,
                calls: false,
                returns: ReturnWords {
                    low: compare_return,
                    high: false,
                },
                events: EventChannels {
                    timeline: TimelineCapture::default(),
                    mmio_read: true,
                    mmio_write: true,
                    fence: true,
                    delay: true,
                },
                memory: vec![],
            },
            None,
            None,
            c,
        )
    }
    fn observed(stop: ExecutionStop, events: Vec<ExecutionEvent>) -> ExecutionObservation {
        ExecutionObservation {
            stop,
            steps: 1,
            events,
            models: vec![],
            calls: vec![],
            final_memory: vec![],
            written: vec![],
        }
    }
    #[test]
    fn verdicts_respect_unknown_returns_and_incomplete_prefixes() {
        let done = observed(
            ExecutionStop::Returned {
                low: Some(7),
                high: None,
            },
            vec![],
        );
        let missing = observed(ExecutionStop::BlockedByPriorPhase, vec![]);
        assert_eq!(
            compare(&done, &done, true, &mut || Ok(())).unwrap().verdict,
            ComparisonVerdict::Match
        );
        assert_eq!(
            compare(&done, &missing, true, &mut || Ok(()))
                .unwrap()
                .verdict,
            ComparisonVerdict::Incomplete
        );
        let more = observed(
            ExecutionStop::BlockedByPriorPhase,
            vec![ExecutionEvent::Write {
                address: 16,
                width: 4,
                value: 1,
            }],
        );
        assert_eq!(
            compare(&done, &more, true, &mut || Ok(())).unwrap().verdict,
            ComparisonVerdict::Diff
        );
        assert_eq!(
            compare(&missing, &more, true, &mut || Ok(()))
                .unwrap()
                .verdict,
            ComparisonVerdict::Incomplete
        );
        let unknown = observed(
            ExecutionStop::Returned {
                low: None,
                high: None,
            },
            vec![],
        );
        assert_eq!(
            compare(&unknown, &done, true, &mut || Ok(()))
                .unwrap()
                .verdict,
            ComparisonVerdict::Incomplete
        );
    }
}

#[cfg(test)]
mod physical_calls {
    use super::*;
    fn observation(targets: &[u32], word: ObservedWord, complete: bool) -> ExecutionObservation {
        let events = targets
            .iter()
            .flat_map(|target| {
                [
                    ExecutionEvent::CallTransfer {
                        site: 0x1000,
                        target: *target,
                        tail: false,
                        indirect: false,
                        stack: Some(0x9000),
                        target_kind: ObservedCallTarget::CapturedCode,
                        words: 1,
                    },
                    ExecutionEvent::TransferArgument {
                        word: 0,
                        value: word,
                    },
                ]
            })
            .collect();
        ExecutionObservation {
            stop: if complete {
                ExecutionStop::Returned {
                    low: Some(0),
                    high: None,
                }
            } else {
                ExecutionStop::BlockedByPriorPhase
            },
            steps: 1,
            events,
            final_memory: vec![],
            written: vec![],
            models: vec![],
            calls: vec![],
        }
    }
    #[test]
    fn reordered_missing_and_unknown_call_prefixes_fail_closed() {
        let r = ComparisonRelation {
            effects: None,
            projection: None,
            returns: ReturnWords {
                low: false,
                high: false,
            },
            events: EventChannels {
                timeline: TimelineCapture::default(),
                mmio_read: true,
                mmio_write: true,
                fence: true,
                delay: true,
            },
            memory: vec![],
            calls: true,
        };
        let known = ObservedWord::Known { value: 7 };
        let a = observation(&[0x2000, 0x3000], known, true);
        for b in [
            observation(&[0x3000, 0x2000], known, true),
            observation(&[0x2000], known, true),
        ] {
            assert_eq!(
                compare(&a, &b, &r, None, None, &mut || Ok(()))
                    .unwrap()
                    .verdict,
                ComparisonVerdict::Diff
            );
        }
        let b = observation(&[0x2000], known, false);
        assert_eq!(
            compare(&a, &b, &r, None, None, &mut || Ok(()))
                .unwrap()
                .verdict,
            ComparisonVerdict::Incomplete
        );
        let b = observation(&[0x2000, 0x3000], ObservedWord::Unknown, true);
        assert_eq!(
            compare(&a, &b, &r, None, None, &mut || Ok(()))
                .unwrap()
                .verdict,
            ComparisonVerdict::Incomplete
        );
        let mut b = observation(&[0x2000, 0x4000], ObservedWord::Unknown, true);
        assert_eq!(
            compare(&a, &b, &r, None, None, &mut || Ok(()))
                .unwrap()
                .verdict,
            ComparisonVerdict::Diff
        );
        b.events.truncate(1);
        assert_eq!(
            compare(&a, &b, &r, None, None, &mut || Ok(()))
                .unwrap_err()
                .code,
            ErrorCode::Integrity
        );
    }
}

#[cfg(test)]
mod timeline_order {
    use super::*;
    #[test]
    fn normal_memory_stays_interleaved_with_calls_mmio_fences_and_delays() {
        let memory = ExecutionEvent::Memory {
            site: 0x1000,
            transaction: MemoryTransaction::Read {
                address: 0x3000,
                width: 4,
                value: MemoryReadValue::Known { value: 7 },
            },
        };
        let relation = ComparisonRelation {
            effects: None,
            projection: None,
            returns: ReturnWords {
                low: false,
                high: false,
            },
            events: EventChannels {
                timeline: TimelineCapture {
                    reads: true,
                    ..Default::default()
                },
                mmio_read: true,
                mmio_write: true,
                fence: true,
                delay: true,
            },
            memory: vec![],
            calls: true,
        };
        for effect in [
            ExecutionEvent::CallTransfer {
                site: 0x1004,
                target: 0x2000,
                tail: false,
                indirect: true,
                stack: Some(0x9000),
                target_kind: ObservedCallTarget::CapturedCode,
                words: 0,
            },
            ExecutionEvent::Write {
                address: 0x4000,
                width: 4,
                value: 7,
            },
            ExecutionEvent::Fence {
                predecessor: 3,
                successor: 3,
            },
            ExecutionEvent::DelayMicros { value: 7 },
        ] {
            let observation = |events| ExecutionObservation {
                stop: ExecutionStop::Returned {
                    low: Some(0),
                    high: None,
                },
                steps: 1,
                events,
                models: vec![],
                calls: vec![],
                final_memory: vec![],
                written: vec![],
            };
            let a = observation(vec![memory.clone(), effect.clone()]);
            let b = observation(vec![effect, memory.clone()]);
            assert_eq!(
                compare(&a, &b, &relation, None, None, &mut || Ok(()))
                    .unwrap()
                    .difference,
                Some(ComparisonDifference::Event { index: 0 })
            );
        }
    }
}
