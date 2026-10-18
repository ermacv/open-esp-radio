//! Structural validation of a request's execution records, independent of
//! the execution and comparison algorithms that produced them.
//!
//! The records of a run are checked against their request, the effect
//! contracts and layout projections its relations select, and the reported
//! verdict: order, per-side obligations of models and call models, captured
//! call arguments, final-memory geometry, event capacity, effect accounting
//! and coverage. A MATCH is rejected when a selected observation is unknown
//! or an execution or model obligation is unmet.
use blobray_domain::*;
use oer_riscv_model::*;
mod calls;
mod capture;
mod effects;
mod models;
mod observation;
mod timeline;
use observation::{EvidencePart, MemoryState, difference_valid};

fn integrity(message: impl Into<String>) -> Error {
    Error::new(ErrorCode::Integrity, message)
}

/// The records of one run of `request` with the result it reported.
pub struct RecordedRun<'a> {
    pub request: &'a ExecutionRequest,
    /// Every effect contract and layout projection the run could select.
    pub effects: &'a [ResolvedEffectContract],
    pub projections: &'a [ResolvedProjection],
    pub records: &'a [ExecutionEvidence],
    pub verdict: Option<ComparisonVerdict>,
    pub complete: bool,
}

/// Validate `run`; any inconsistency is an `Integrity` error.
pub fn validate_records(run: &RecordedRun<'_>, c: &mut dyn RunControl) -> Result<()> {
    let request = run.request;
    request.validate()?;
    let mut case = 0u32;
    let mut side = false;
    let mut events = 0u32;
    let mut outcome = false;
    let mut complete = true;
    let mut blocked = false;
    let mut phase_complete = true;
    let mut models = [models::Models::new(), models::Models::new()];
    let mut calls = [calls::Calls::new(), calls::Calls::new()];
    let mut effects: [Option<EffectTracker<'_>>; 2] = [None, None];
    // A contract rule may depend on the side's next concrete effect, so each
    // effect is classified once its successor, or the side's outcome, arrives.
    let mut pending_effect: Option<(ExecutionEvent, u32)> = None;
    let mut prepared = None;
    let mut part = EvidencePart::Events;
    // End of the previous written range of the current side.
    let mut written_end: Option<u64> = None;
    let mut memory_state = [MemoryState::new(), MemoryState::new()];
    let mut capture = [capture::CaptureState::new(), capture::CaptureState::new()];
    let mut relation_complete = true;
    let mut environment_complete = true;
    let mut verdict = run.verdict.map(|_| ComparisonVerdict::Match);
    // Vendor coverage, then replacement coverage, after the last case: per
    // side, whether a record arrived and the highest address so far. Only a
    // side's single record may be empty.
    let mut coverage = [false, request.replacement.is_none()];
    let mut covered: [Option<u32>; 2] = [None, None];
    let mut empty = [false, false];
    for record in run.records {
        if let ExecutionEvidence::Coverage {
            replacement,
            coverage: reached,
        } = record
        {
            let side = usize::from(*replacement);
            if case as usize != request.cases.len()
                || (*replacement && (!coverage[0] || request.replacement.is_none()))
                || (!*replacement && request.replacement.is_some() && coverage[1])
                || empty[side]
                || (coverage[side] && reached.instructions.is_empty())
                || reached
                    .instructions
                    .first()
                    .is_some_and(|first| covered[side].is_some_and(|last| *first <= last))
            {
                return Err(integrity("execution coverage order differs"));
            }
            reached.validate(c)?;
            coverage[side] = true;
            empty[side] = reached.instructions.is_empty();
            if let Some(last) = reached.instructions.last() {
                covered[side] = Some(*last);
            }
            continue;
        }
        if case as usize >= request.cases.len() {
            return Err(integrity("extra execution case"));
        }
        let phase = &request.cases[case as usize];
        c.checkpoint(run.projections.len() as u64 + 1)?;
        let projection =
            selected_projection(phase.relation.as_ref(), run.projections)?.map(|p| &p.projection);
        let must_block = phase.reset == SessionReset::Warm && blocked;
        let close_chain = request
            .cases
            .get(case as usize + 1)
            .is_none_or(|next| next.reset == SessionReset::Cold);
        if prepared != Some(case) {
            relation_complete = true;
            c.checkpoint(run.effects.len() as u64 + 1)?;
            effects = match selected_effect_contract(phase.relation.as_ref(), run.effects)? {
                Some(selected) => {
                    c.checkpoint((selected.contract.rules.len() as u64 + 1).saturating_pow(2))?;
                    selected.contract.validate_use(request, case as usize)?;
                    [
                        Some(EffectTracker::new(&selected.contract, false, c)?),
                        Some(EffectTracker::new(&selected.contract, true, c)?),
                    ]
                }
                None => [None, None],
            };
            capture = [capture::CaptureState::new(), capture::CaptureState::new()];
            memory_state[0].begin(phase.relation.as_ref(), false);
            memory_state[1].begin(phase.relation.as_ref(), true);
            if let Some(p) = projection {
                c.checkpoint((p.fields.len() + p.branches.len() + 1).pow(2) as u64)?;
                p.validate_use(request, case as usize)?;
                memory_state[0].begin_projection(p, &phase.vendor, false, c)?;
                memory_state[1].begin_projection(
                    p,
                    phase.replacement.as_ref().unwrap(),
                    true,
                    c,
                )?;
            }
            calls[0].begin(&phase.vendor.calls, phase.reset, must_block, c)?;
            models[0].begin(&phase.vendor.models, phase.reset, must_block, c)?;
            if let Some(replacement) = &phase.replacement {
                calls[1].begin(&replacement.calls, phase.reset, must_block, c)?;
                models[1].begin(&replacement.models, phase.reset, must_block, c)?;
            }
            prepared = Some(case);
        }
        match record {
            ExecutionEvidence::FinalMemory {
                case: i,
                replacement,
                chunk,
            } => {
                if *i != case
                    || *replacement != side
                    || outcome
                    || part == EvidencePart::Environment
                    || must_block
                {
                    return Err(integrity("final-memory evidence order differs"));
                }
                part = EvidencePart::Memory;
                let input = if side {
                    phase.replacement.as_ref().unwrap()
                } else {
                    &phase.vendor
                };
                memory_state[usize::from(side)].chunk(input, chunk, c)?;
            }
            ExecutionEvidence::Written {
                case: i,
                replacement,
                range,
            } => {
                let input = if side {
                    phase.replacement.as_ref().unwrap()
                } else {
                    &phase.vendor
                };
                if *i != case
                    || *replacement != side
                    || outcome
                    || part == EvidencePart::Environment
                    || must_block
                    || !input.observe_timeline.written
                {
                    return Err(integrity("written-range evidence order differs"));
                }
                if range.length == 0
                    || range.end() > u64::from(u32::MAX)
                    || written_end.is_some_and(|end| u64::from(range.address) <= end)
                {
                    return Err(integrity("written ranges are not ascending and coalesced"));
                }
                written_end = Some(range.end());
                part = EvidencePart::Memory;
            }
            ExecutionEvidence::CallModel {
                case: i,
                replacement,
                observation,
            } => {
                if *i != case || *replacement != side || outcome {
                    return Err(integrity("call model evidence order differs"));
                }
                part = EvidencePart::Environment;
                calls[usize::from(side)].observe(observation, close_chain, must_block, c)?;
                environment_complete &= observation.status != ModelStatus::Incomplete;
            }
            ExecutionEvidence::Model {
                case: i,
                replacement,
                observation,
            } => {
                if *i != case || *replacement != side || outcome {
                    return Err(integrity("model evidence order differs"));
                }
                part = EvidencePart::Environment;
                models[usize::from(side)].observe(observation, close_chain, must_block, c)?;
                environment_complete &= observation.status != ModelStatus::Incomplete;
            }
            ExecutionEvidence::Event {
                case: i,
                replacement,
                event,
            } => {
                if *i != case || *replacement != side || outcome || part != EvidencePart::Events {
                    return Err(integrity("execution event order differs"));
                }
                let input = if side {
                    phase.replacement.as_ref().unwrap()
                } else {
                    &phase.vendor
                };
                c.checkpoint(
                    input
                        .observe_calls
                        .as_ref()
                        .map_or(0, |p| p.overrides.len()) as u64
                        + 1,
                )?;
                timeline::validate(input, event)?;
                if let (
                    Some(p),
                    ExecutionEvent::Branch {
                        site,
                        target,
                        fallthrough,
                        ..
                    },
                ) = (projection, event)
                    && phase
                        .relation
                        .as_ref()
                        .is_some_and(|r| r.events.timeline.branches)
                {
                    c.checkpoint(p.branches.len() as u64 + 1)?;
                    relation_complete &= p
                        .branch_location(
                            BranchLocation {
                                site: *site,
                                target: *target,
                                fallthrough: *fallthrough,
                            },
                            side,
                        )
                        .is_some();
                }
                if phase
                    .relation
                    .as_ref()
                    .is_some_and(|r| r.events.selects(event))
                    && let Some(transaction) = event.normal_memory()
                {
                    relation_complete &= transaction.known();
                    if let Some(p) = projection {
                        c.checkpoint(p.fields.len() as u64 + 1)?;
                        relation_complete &= p.memory_location(transaction, side)?.is_some();
                    }
                }
                capture[usize::from(side)].event(
                    input,
                    event,
                    phase.relation.as_ref().is_some_and(|r| r.calls),
                    c,
                )?;
                calls[usize::from(side)].event(event, c)?;
                if is_contract_effect(event)
                    && let Some(tracker) = &mut effects[usize::from(side)]
                {
                    if let Some((prior, ordinal)) = pending_effect.take() {
                        tracker.observe(&prior, Some(event), ordinal, c)?;
                    }
                    pending_effect = Some((event.clone(), events));
                }
                events += 1;
                if events > request.max_events {
                    return Err(integrity("event capacity exceeded"));
                }
            }
            ExecutionEvidence::Outcome {
                case: i,
                replacement,
                stop,
                steps,
            } => {
                if let Some((prior, ordinal)) = pending_effect.take()
                    && let Some(tracker) = &mut effects[usize::from(side)]
                {
                    tracker.observe(&prior, None, ordinal, c)?;
                }
                if *i != case || *replacement != side || outcome {
                    return Err(integrity("execution outcome order differs"));
                }
                let input = if side {
                    phase
                        .replacement
                        .as_ref()
                        .ok_or_else(|| integrity("outcome has no replacement input"))?
                } else {
                    &phase.vendor
                };
                if matches!(stop, ExecutionStop::BlockedByPriorPhase) != must_block
                    || (must_block && (*steps != 0 || events != 0))
                {
                    return Err(integrity("execution dependency blocking differs"));
                }
                let address_valid = |pc: u32| pc & 1 == 0 && pc < u32::MAX - 1;
                let goal_valid = match (stop, &input.goal) {
                    (ExecutionStop::Returned { .. }, ExecutionGoal::Return) => true,
                    (ExecutionStop::ReachedSymbol { pc }, ExecutionGoal::ReachSymbol { .. }) => {
                        address_valid(*pc)
                    }
                    (
                        ExecutionStop::ObservedCall { pc, target, tail },
                        ExecutionGoal::ObserveCall { include_tail, .. },
                    ) => address_valid(*pc) && address_valid(*target) && (!tail || *include_tail),
                    (ExecutionStop::GoalNotReached { .. }, goal) => {
                        !matches!(goal, ExecutionGoal::Return)
                    }
                    (ExecutionStop::Incomplete { .. } | ExecutionStop::BlockedByPriorPhase, _) => {
                        true
                    }
                    _ => false,
                };
                if !goal_valid {
                    return Err(integrity(
                        "execution outcome does not match its declared goal",
                    ));
                }
                capture[usize::from(side)].finish(input, stop)?;
                if phase.relation.as_ref().is_some_and(|r| r.calls) {
                    relation_complete &= capture[usize::from(side)].known;
                }
                memory_state[usize::from(side)].finish(input, must_block)?;
                relation_complete &= memory_state[usize::from(side)].known;
                if let Some(r) = &phase.relation {
                    for (word, selected) in [r.returns.low, r.returns.high].into_iter().enumerate()
                    {
                        if selected {
                            relation_complete &= match stop {
                                ExecutionStop::Returned { low, high } => {
                                    [*low, *high][word].is_some()
                                }
                                _ => false,
                            };
                        }
                    }
                }
                models[usize::from(side)].finish_side()?;
                calls[usize::from(side)].finish_side()?;
                complete &= stop.completed() && environment_complete;
                phase_complete &= stop.completed() && environment_complete;
                part = EvidencePart::Events;
                written_end = None;
                environment_complete = true;
                if request.replacement.is_none() {
                    blocked = !phase_complete;
                    phase_complete = true;
                    case += 1;
                    events = 0;
                } else if !side {
                    side = true;
                    events = 0;
                } else if phase.relation.is_none() {
                    // A setup case records no comparison.
                    blocked = !phase_complete;
                    phase_complete = true;
                    case += 1;
                    side = false;
                    events = 0;
                } else {
                    outcome = true;
                }
            }
            ExecutionEvidence::Coverage { .. } => unreachable!("coverage is validated above"),
            ExecutionEvidence::Comparison { case: i, result } => {
                if *i != case || !outcome || verdict.is_none() {
                    return Err(integrity("comparison order differs"));
                }
                effects::validate_result(result, &effects)?;
                if !difference_valid(result, phase, request.max_events, projection)
                    || (result.verdict == ComparisonVerdict::Match
                        && (!phase_complete || !relation_complete))
                {
                    return Err(integrity("MATCH has unmet execution/model obligations"));
                }
                verdict = Some(match (verdict.unwrap(), result.verdict) {
                    (ComparisonVerdict::Diff, _) | (_, ComparisonVerdict::Diff) => {
                        ComparisonVerdict::Diff
                    }
                    (ComparisonVerdict::Incomplete, _) | (_, ComparisonVerdict::Incomplete) => {
                        ComparisonVerdict::Incomplete
                    }
                    _ => ComparisonVerdict::Match,
                });
                blocked = !phase_complete;
                phase_complete = true;
                case += 1;
                side = false;
                events = 0;
                outcome = false;
            }
        }
    }
    if case as usize != request.cases.len()
        || coverage != [true, true]
        || side
        || outcome
        || events != 0
        || complete != run.complete
        || verdict != run.verdict
        || !models.iter().all(models::Models::closed)
        || !calls.iter().all(calls::Calls::closed)
    {
        return Err(integrity("execution evidence summary differs"));
    }
    Ok(())
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::records::timeline::tests::input;

    pub(in crate::records) fn fixture_target() -> ExecutionTarget {
        ExecutionTarget {
            executables: vec![ArtifactId::of_bytes(b"fixture")],
            abi: CallAbi::RiscvInteger,
            stack: MemorySeed {
                address: 0x8000,
                length: 4096,
                fill: None,
                bytes: vec![],
            },
        }
    }

    /// The entry of `fixture_target` at 0x1000.
    pub(in crate::records) fn fixture_endpoint() -> CallEndpoint {
        let object = ObjectId {
            artifact: ArtifactId::of_bytes(b"fixture"),
            location: ObjectLocation::Standalone,
        };
        CallEndpoint {
            symbol: Some(SymbolId {
                object: object.clone(),
                table: SymbolTableKind::Static,
                table_section: 3,
                index: 1,
            }),
            object,
            boundary: ReviewedCallBoundary::Code { address: 0x1000 },
        }
    }

    /// Empty coverage records a valid record stream of `request` ends with.
    fn coverage_rows(request: &ExecutionRequest) -> Vec<ExecutionEvidence> {
        std::iter::once(false)
            .chain(request.replacement.as_ref().map(|_| true))
            .map(|replacement| ExecutionEvidence::Coverage {
                replacement,
                coverage: ExecutionCoverage::default(),
            })
            .collect()
    }

    /// Validate complete `rows` of `request` reporting `verdict`.
    fn validate(
        request: &ExecutionRequest,
        effects: &[ResolvedEffectContract],
        projections: &[ResolvedProjection],
        rows: &[ExecutionEvidence],
        verdict: Option<ComparisonVerdict>,
    ) -> Result<()> {
        validate_records(
            &RecordedRun {
                request,
                effects,
                projections,
                records: rows,
                verdict,
                complete: true,
            },
            &mut || Ok(()),
        )
    }

    /// Validate `rows` of `request` followed by empty coverage.
    pub(in crate::records) fn check(
        request: &ExecutionRequest,
        effects: &[ResolvedEffectContract],
        rows: &[ExecutionEvidence],
        verdict: Option<ComparisonVerdict>,
    ) -> Result<()> {
        let mut rows = rows.to_vec();
        rows.extend(coverage_rows(request));
        validate(request, effects, &[], &rows, verdict)
    }

    fn check_projected(
        request: &ExecutionRequest,
        projections: &[ResolvedProjection],
        rows: &[ExecutionEvidence],
        verdict: Option<ComparisonVerdict>,
    ) -> Result<()> {
        let mut rows = rows.to_vec();
        rows.extend(coverage_rows(request));
        validate(request, &[], projections, &rows, verdict)
    }

    /// One compared phase whose sides read `value` at `address`, selected
    /// through the timeline, reporting `verdict`; without coverage.
    fn read(
        address: u32,
        value: MemoryReadValue,
        verdict: ComparisonVerdict,
    ) -> (ExecutionRequest, Vec<ExecutionEvidence>) {
        let target = fixture_target();
        let request = ExecutionRequest {
            schema: EXECUTION_SCHEMA,
            vendor: target.clone(),
            replacement: Some(target),
            binding: Some(CompiledBinding::SharedCore),
            max_events: 4,
            cases: vec![ExecutionCase {
                name: "case".into(),
                reset: SessionReset::Cold,
                stack_fill: None,
                vendor: input(),
                replacement: Some(input()),
                relation: Some(ComparisonRelation {
                    effects: None,
                    projection: None,
                    returns: ReturnWords {
                        low: false,
                        high: false,
                    },
                    events: EventChannels {
                        mmio_read: false,
                        mmio_write: false,
                        fence: false,
                        delay: false,
                        timeline: TimelineCapture {
                            reads: true,
                            ..Default::default()
                        },
                    },
                    memory: vec![],
                    calls: false,
                }),
            }],
        };
        let mut rows = Vec::new();
        for replacement in [false, true] {
            rows.push(ExecutionEvidence::Event {
                case: 0,
                replacement,
                event: ExecutionEvent::Memory {
                    site: 0x1000,
                    transaction: MemoryTransaction::Read {
                        address,
                        width: 4,
                        value,
                    },
                },
            });
            rows.push(ExecutionEvidence::Outcome {
                case: 0,
                replacement,
                steps: 1,
                stop: ExecutionStop::Returned {
                    low: Some(0),
                    high: None,
                },
            });
        }
        rows.push(ExecutionEvidence::Comparison {
            case: 0,
            result: CaseComparison {
                effect_claim: None,
                effect_gap: None,
                verdict,
                difference: None,
            },
        });
        (request, rows)
    }

    /// A request with the verdict and completeness a run reported.
    #[derive(Clone)]
    struct Reported {
        request: ExecutionRequest,
        verdict: Option<ComparisonVerdict>,
        complete: bool,
    }
    impl Reported {
        /// Validate `rows` followed by empty coverage.
        fn validate(&self, rows: &[ExecutionEvidence]) -> Result<()> {
            let mut rows = rows.to_vec();
            rows.extend(coverage_rows(&self.request));
            validate_records(
                &RecordedRun {
                    request: &self.request,
                    effects: &[],
                    projections: &[],
                    records: &rows,
                    verdict: self.verdict,
                    complete: self.complete,
                },
                &mut || Ok(()),
            )
        }
    }

    fn unknown_read() -> (ExecutionRequest, Vec<ExecutionEvidence>) {
        read(
            0x3000,
            MemoryReadValue::Unknown,
            ComparisonVerdict::Incomplete,
        )
    }

    #[test]
    fn completed_code_cannot_forge_match_for_unknown_selected_memory_reads() {
        let (mut request, mut rows) = unknown_read();
        let incomplete = Some(ComparisonVerdict::Incomplete);
        let matched = Some(ComparisonVerdict::Match);
        check(&request, &[], &rows, incomplete).unwrap();
        // The reported verdict and completeness must be those of the records.
        assert!(check(&request, &[], &rows, matched).is_err());
        let mut summary = rows.clone();
        summary.extend(coverage_rows(&request));
        let partial = RecordedRun {
            request: &request,
            effects: &[],
            projections: &[],
            records: &summary,
            verdict: incomplete,
            complete: false,
        };
        assert!(validate_records(&partial, &mut || Ok(())).is_err());
        if let ExecutionEvidence::Comparison { result, .. } = rows.last_mut().unwrap() {
            result.verdict = ComparisonVerdict::Match;
        }
        assert_eq!(
            check(&request, &[], &rows, matched).unwrap_err().code,
            ErrorCode::Integrity
        );
        let relation = request.cases[0].relation.as_mut().unwrap();
        relation.events.timeline.reads = false;
        relation.returns.low = true;
        // explicit exclusion preserves unknown raw evidence
        check(&request, &[], &rows, matched).unwrap();
    }
    #[test]
    fn coverage_is_required_once_per_side_after_the_last_case_and_consistent() {
        let (request, rows) = unknown_read();
        let incomplete = Some(ComparisonVerdict::Incomplete);
        let reached = |instructions: Vec<u32>, branches: Vec<BranchCoverage>| ExecutionCoverage {
            instructions,
            branches,
            transfers: vec![],
        };
        let row = |replacement, coverage| ExecutionEvidence::Coverage {
            replacement,
            coverage,
        };
        let valid = reached(
            vec![0x1000, 0x1004],
            vec![BranchCoverage {
                site: 0x1000,
                taken: true,
                fallthrough: false,
            }],
        );
        let with = |tail: Vec<ExecutionEvidence>| {
            let mut all = rows.clone();
            all.extend(tail);
            validate(&request, &[], &[], &all, incomplete)
        };
        with(vec![row(false, valid.clone()), row(true, valid.clone())]).unwrap();
        // A side may continue in ascending records; empty coverage is one record.
        let later = reached(vec![0x2000], vec![]);
        with(vec![
            row(false, valid.clone()),
            row(false, later.clone()),
            row(true, reached(vec![], vec![])),
        ])
        .unwrap();
        for tail in [
            vec![],
            vec![row(false, valid.clone())],
            vec![row(true, valid.clone()), row(false, valid.clone())],
            vec![
                row(false, valid.clone()),
                row(false, valid.clone()),
                row(true, valid.clone()),
            ],
            // Records of a side must ascend, and an empty record stands alone.
            vec![
                row(false, later.clone()),
                row(false, valid.clone()),
                row(true, valid.clone()),
            ],
            vec![
                row(false, reached(vec![], vec![])),
                row(false, later.clone()),
                row(true, valid.clone()),
            ],
            vec![
                row(false, valid.clone()),
                row(true, valid.clone()),
                row(false, later.clone()),
            ],
            // A branch direction of an instruction that never executed.
            vec![
                row(false, reached(vec![0x1004], valid.branches.clone())),
                row(true, valid.clone()),
            ],
            // Unordered instructions and a branch with no direction.
            vec![
                row(false, reached(vec![0x1004, 0x1000], vec![])),
                row(true, valid.clone()),
            ],
            vec![
                row(
                    false,
                    reached(
                        vec![0x1000],
                        vec![BranchCoverage {
                            site: 0x1000,
                            taken: false,
                            fallthrough: false,
                        }],
                    ),
                ),
                row(true, valid.clone()),
            ],
        ] {
            assert_eq!(with(tail).unwrap_err().code, ErrorCode::Integrity);
        }
        // Coverage cannot precede the last case's records.
        let mut early = vec![row(false, valid.clone())];
        early.extend(rows.clone());
        early.push(row(true, valid));
        assert_eq!(
            validate(&request, &[], &[], &early, incomplete)
                .unwrap_err()
                .code,
            ErrorCode::Integrity
        );
    }
    #[test]
    fn match_needs_every_selected_read_inside_a_projected_field() {
        let endpoint = LayoutEndpoint {
            entry: fixture_endpoint(),
            domains: vec![LayoutDomain {
                address: 0x3000,
                length: 0x100,
            }],
        };
        let at = FieldLocation {
            domain: 0,
            offset: 0,
        };
        let review = ArtifactId::of_bytes(b"projection");
        let projections = [ResolvedProjection {
            id: review.clone(),
            projection: LayoutProjection {
                vendor: endpoint.clone(),
                replacement: endpoint,
                fields: vec![LayoutField {
                    name: "word".into(),
                    vendor: at,
                    replacement: at,
                    width: 4,
                    count: 1,
                    final_state: false,
                    timeline: true,
                }],
                branches: vec![],
                applicability: "fixture".into(),
                reason: "fixture".into(),
            },
        }];
        let matched = Some(ComparisonVerdict::Match);
        let known = MemoryReadValue::Known { value: 7 };
        for (address, valid) in [(0x3000, true), (0x3004, false)] {
            let (mut request, rows) = read(address, known, ComparisonVerdict::Match);
            // Without the projection the read is a physical observation.
            check(&request, &[], &rows, matched).unwrap();
            request.cases[0].relation.as_mut().unwrap().projection = Some(review.clone());
            assert!(check(&request, &[], &rows, matched).is_err());
            assert_eq!(
                check_projected(&request, &projections, &rows, matched).is_ok(),
                valid
            );
        }
    }
    #[test]
    fn outcomes_must_match_the_declared_goal() {
        let request = ExecutionRequest {
            schema: EXECUTION_SCHEMA,
            vendor: fixture_target(),
            replacement: None,
            binding: None,
            cases: vec![ExecutionCase {
                relation: None,
                reset: SessionReset::Cold,
                stack_fill: None,
                name: "one".into(),
                vendor: Invocation {
                    arguments: vec![Some(0); 8],
                    ..input()
                },
                replacement: None,
            }],
            max_events: 1,
        };
        // Receipt validation must not confuse reaching a boundary with returning,
        // or accept a blocked outcome without a failed warm predecessor.
        let mut goal_manifest = Reported {
            request: request.clone(),
            verdict: None,
            complete: false,
        };
        goal_manifest.request.cases[0].vendor.goal = ExecutionGoal::ReachSymbol {
            target: SymbolId {
                object: ObjectId {
                    artifact: ArtifactId::of_bytes(b"fixture"),
                    location: ObjectLocation::Standalone,
                },
                table: SymbolTableKind::Static,
                table_section: 3,
                index: 1,
            },
        };
        for stop in [
            ExecutionStop::Returned {
                low: Some(0),
                high: None,
            },
            ExecutionStop::BlockedByPriorPhase,
            ExecutionStop::ObservedCall {
                pc: 0x1000,
                target: 0x1004,
                tail: false,
            },
        ] {
            let error = goal_manifest
                .validate(&[ExecutionEvidence::Outcome {
                    case: 0,
                    replacement: false,
                    stop,
                    steps: 1,
                }])
                .unwrap_err();
            assert_eq!(error.code, ErrorCode::Integrity);
            assert!(
                error.message.contains("goal") || error.message.contains("blocking"),
                "{error:?}"
            );
        }
        goal_manifest
            .validate(&[ExecutionEvidence::Outcome {
                case: 0,
                replacement: false,
                stop: ExecutionStop::GoalNotReached {
                    low: Some(0),
                    high: None,
                },
                steps: 1,
            }])
            .unwrap();
    }
    #[test]
    fn models_reject_missing_forged_identity_closure_and_match() {
        let target = fixture_target();
        let declaration = DeviceDeclaration {
            id: "sequence".into(),
            applicability: "fixture".into(),
            lifetime: RegionLifetime::Phase,
            behavior: DeviceBehavior::SequenceRead {
                address: 0x3000,
                width: 4,
                runs: vec![
                    ReadRun {
                        value: 7,
                        count: 20_000,
                    },
                    ReadRun::once(9),
                ],
            },
        };
        let input = Invocation {
            observe_calls: None,
            observe_timeline: TimelineCapture::default(),
            observe_memory: vec![],
            entry: 4096,
            goal: ExecutionGoal::Return,
            arguments: vec![],
            memory: vec![],
            preload: vec![],
            models: vec![declaration.clone()],
            calls: vec![],
        };
        let mut rows = Vec::new();
        for replacement in [false, true] {
            rows.push(ExecutionEvidence::Event {
                case: 0,
                replacement,
                event: ExecutionEvent::Read {
                    address: 0x3000,
                    width: 4,
                    value: 7,
                },
            });
            rows.push(ExecutionEvidence::Model {
                case: 0,
                replacement,
                observation: ModelObservation {
                    commands: None,
                    id: declaration.id.clone(),
                    definition: declaration.identity(&mut || Ok(())).unwrap(),
                    lifetime: RegionLifetime::Phase,
                    reads: 1,
                    writes: 0,
                    remaining_reads: 20_000,
                    remaining_writes: 0,
                    closed: true,
                    issue: None,
                    status: ModelStatus::Incomplete,
                },
            });
            rows.push(ExecutionEvidence::Outcome {
                case: 0,
                replacement,
                steps: 2,
                stop: ExecutionStop::Returned {
                    low: Some(7),
                    high: None,
                },
            });
        }
        rows.push(ExecutionEvidence::Comparison {
            case: 0,
            result: CaseComparison {
                effect_claim: None,
                effect_gap: None,
                verdict: ComparisonVerdict::Incomplete,
                difference: None,
            },
        });
        let request = ExecutionRequest {
            schema: EXECUTION_SCHEMA,
            vendor: target.clone(),
            replacement: Some(target),
            binding: Some(CompiledBinding::ProductionEntry),
            cases: vec![ExecutionCase {
                relation: Some(ComparisonRelation {
                    effects: None,
                    projection: None,
                    calls: false,
                    returns: ReturnWords {
                        low: true,
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
                }),
                name: "one".into(),
                reset: SessionReset::Cold,
                stack_fill: None,
                vendor: input.clone(),
                replacement: Some(input),
            }],
            max_events: 4,
        };
        let manifest = Reported {
            request,
            verdict: Some(ComparisonVerdict::Incomplete),
            complete: false,
        };
        let validate = |m: &Reported, rows: &[ExecutionEvidence]| m.validate(rows);
        validate(&manifest, &rows).unwrap();
        let model_index = rows
            .iter()
            .position(|r| matches!(r, ExecutionEvidence::Model { .. }))
            .unwrap();
        for variant in 0..6 {
            let mut forged = rows.clone();
            if variant == 0 {
                forged.remove(model_index);
            } else if let ExecutionEvidence::Model { observation, .. } = &mut forged[model_index] {
                match variant {
                    1 => observation.definition = ArtifactId::of_bytes(b"other assumption"),
                    2 => {
                        observation.closed = false;
                        observation.status = ModelStatus::Open;
                    }
                    3 => observation.status = ModelStatus::Complete,
                    4 => {
                        observation.remaining_reads = 0;
                        observation.status = ModelStatus::Complete;
                    }
                    // Encoded run count is not the remaining logical read count.
                    5 => observation.remaining_reads = 1,
                    _ => unreachable!(),
                }
            }
            assert_eq!(
                validate(&manifest, &forged).unwrap_err().code,
                ErrorCode::Integrity
            );
        }
        // Code completion alone cannot admit MATCH with unknown selected call words.
        {
            let mut m = manifest.clone();
            m.complete = true;
            {
                let i = &mut m.request.cases[0].vendor;
                i.models.clear();
                i.observe_calls = Some(CallCapture {
                    include_tail: false,
                    argument_words: 1,
                    overrides: vec![],
                });
            }
            m.request.cases[0].replacement = Some(m.request.cases[0].vendor.clone());
            m.request.cases[0].relation.as_mut().unwrap().calls = true;
            let mut captured = Vec::new();
            for replacement in [false, true] {
                for event in [
                    ExecutionEvent::CallTransfer {
                        site: 4096,
                        target: 4100,
                        tail: false,
                        indirect: false,
                        stack: Some(12288),
                        target_kind: ObservedCallTarget::CapturedCode,
                        words: 1,
                    },
                    ExecutionEvent::TransferArgument {
                        word: 0,
                        value: ObservedWord::Unknown,
                    },
                ] {
                    captured.push(ExecutionEvidence::Event {
                        case: 0,
                        replacement,
                        event,
                    });
                }
                captured.push(ExecutionEvidence::Outcome {
                    case: 0,
                    replacement,
                    steps: 3,
                    stop: ExecutionStop::Returned {
                        low: Some(7),
                        high: None,
                    },
                });
            }
            captured.push(ExecutionEvidence::Comparison {
                case: 0,
                result: CaseComparison {
                    effect_claim: None,
                    effect_gap: None,
                    verdict: ComparisonVerdict::Incomplete,
                    difference: None,
                },
            });
            validate(&m, &captured).unwrap();
            m.verdict = Some(ComparisonVerdict::Match);
            if let ExecutionEvidence::Comparison { result, .. } = captured.last_mut().unwrap() {
                result.verdict = ComparisonVerdict::Match;
            }
            assert_eq!(
                validate(&m, &captured).unwrap_err().code,
                ErrorCode::Integrity
            );
            m.request.cases[0].relation.as_mut().unwrap().calls = false;
            validate(&m, &captured).unwrap(); // excluded unknown evidence stays retained
        }
        let mut forged = rows;
        for row in &mut forged {
            if let ExecutionEvidence::Comparison { result, .. } = row {
                result.verdict = ComparisonVerdict::Match;
            }
        }
        let mut forged_manifest = manifest;
        forged_manifest.verdict = Some(ComparisonVerdict::Match);
        assert_eq!(
            validate(&forged_manifest, &forged).unwrap_err().code,
            ErrorCode::Integrity
        );
    }
}
