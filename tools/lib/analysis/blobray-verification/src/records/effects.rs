//! Effect-contract accounting of a comparison against its recorded events.
use super::*;

/// Check policy accounting against the side's recorded events; no ISA replay.
pub(super) fn validate_result(
    result: &CaseComparison,
    trackers: &[Option<EffectTracker<'_>>; 2],
) -> Result<()> {
    let claim = trackers[0].as_ref().map(EffectTracker::claim_ceiling);
    let gap = trackers.iter().flatten().find_map(EffectTracker::gap);
    let violation = trackers
        .iter()
        .flatten()
        .find_map(EffectTracker::first_violation);
    if result.effect_claim != claim
        || result.effect_gap != gap
        || (result.verdict == ComparisonVerdict::Match && (gap.is_some() || violation.is_some()))
        || (violation.is_some() && result.verdict != ComparisonVerdict::Diff)
        || matches!(&result.difference, Some(ComparisonDifference::EffectViolation { violation: v }) if Some(*v) != violation)
    {
        return Err(integrity(
            "effect accounting differs from recorded observations",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::records::tests::{check, fixture_endpoint, fixture_target};

    fn contract() -> EffectContract {
        let endpoint = fixture_endpoint();
        let pattern = EffectPattern {
            preceded_by: None,
            occurrence: None,
            followed_by: None,
            selector: EffectSelector::MmioWrite {
                address: 0x3000,
                width: 4,
            },
            value: EffectValue::Any,
        };
        EffectContract {
            unclassified: UnclassifiedEffects::Incomplete,
            vendor: endpoint.clone(),
            replacement: endpoint,
            rules: vec![EffectRule {
                name: "register".into(),
                vendor: Some(pattern),
                replacement: Some(pattern),
                disposition: EffectDisposition::Required,
                min_occurrences: 1,
                max_occurrences: 4,
                reason: "fixture".into(),
            }],
            claim_ceiling: EffectClaimCeiling::SelectedEffectEquality,
            applicability: "fixture".into(),
            reason: "fixture".into(),
        }
    }

    fn request(review: &ArtifactId) -> ExecutionRequest {
        let input = Invocation {
            entry: 0x1000,
            goal: ExecutionGoal::Return,
            arguments: vec![],
            memory: vec![],
            preload: vec![],
            models: vec![],
            calls: vec![],
            observe_calls: None,
            observe_timeline: TimelineCapture::default(),
            observe_memory: vec![],
        };
        let target = fixture_target();
        ExecutionRequest {
            schema: EXECUTION_SCHEMA,
            vendor: target.clone(),
            replacement: Some(target),
            binding: Some(CompiledBinding::SharedCore),
            max_events: 4,
            cases: vec![ExecutionCase {
                name: "case".into(),
                reset: SessionReset::Cold,
                stack_fill: None,
                vendor: input.clone(),
                replacement: Some(input),
                relation: Some(ComparisonRelation {
                    effects: Some(review.clone()),
                    projection: None,
                    calls: false,
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
                }),
            }],
        }
    }

    /// Records of one compared case whose sides record `events`.
    fn rows(events: &[ExecutionEvent], result: CaseComparison) -> Vec<ExecutionEvidence> {
        let mut rows = Vec::new();
        for replacement in [false, true] {
            for event in events {
                rows.push(ExecutionEvidence::Event {
                    case: 0,
                    replacement,
                    event: event.clone(),
                });
            }
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
        rows.push(ExecutionEvidence::Comparison { case: 0, result });
        rows
    }

    #[test]
    fn effect_accounting_must_follow_the_recorded_events() {
        let review = ArtifactId::of_bytes(b"contract");
        let effects = vec![ResolvedEffectContract {
            id: review.clone(),
            contract: contract(),
        }];
        let request = request(&review);
        let matched = Some(ComparisonVerdict::Match);
        let incomplete = Some(ComparisonVerdict::Incomplete);
        let result = CaseComparison {
            effect_claim: Some(EffectClaimCeiling::SelectedEffectEquality),
            effect_gap: None,
            verdict: ComparisonVerdict::Match,
            difference: None,
        };
        let event = ExecutionEvent::Write {
            address: 0x3000,
            width: 4,
            value: 7,
        };
        let first = rows(std::slice::from_ref(&event), result.clone());
        check(&request, &effects, &first, matched).unwrap();
        // The selected contract must be supplied.
        assert!(check(&request, &[], &first, matched).is_err());
        let mut forged = result.clone();
        forged.effect_claim = None;
        assert!(
            check(
                &request,
                &effects,
                &rows(std::slice::from_ref(&event), forged),
                matched
            )
            .is_err()
        );
        assert!(check(&request, &effects, &rows(&[], result.clone()), matched).is_err());
        let mut missing = result.clone();
        missing.verdict = ComparisonVerdict::Incomplete;
        missing.effect_gap = Some(EffectGap::Unexercised {
            replacement: false,
            rule: 0,
        });
        check(&request, &effects, &rows(&[], missing.clone()), incomplete).unwrap();
        missing.effect_gap = None;
        assert!(check(&request, &effects, &rows(&[], missing), incomplete).is_err());
        let mut unknown = result.clone();
        unknown.verdict = ComparisonVerdict::Incomplete;
        unknown.effect_gap = Some(EffectGap::Unclassified {
            replacement: false,
            event: 1,
        });
        let events = [
            event.clone(),
            ExecutionEvent::Fence {
                predecessor: 3,
                successor: 3,
            },
        ];
        check(
            &request,
            &effects,
            &rows(&events, unknown.clone()),
            incomplete,
        )
        .unwrap();
        unknown.effect_gap = Some(EffectGap::Unclassified {
            replacement: false,
            event: 0,
        });
        assert!(check(&request, &effects, &rows(&events, unknown), incomplete).is_err());
        assert!(check(&request, &effects, &rows(&events, result.clone()), matched).is_err());
        let mut strict = effects.clone();
        let rule = &mut strict[0].contract.rules[0];
        rule.vendor.as_mut().unwrap().value = EffectValue::Exact { value: 9 };
        rule.replacement = rule.vendor;
        let mut difference = result.clone();
        difference.verdict = ComparisonVerdict::Diff;
        difference.difference = Some(ComparisonDifference::EffectViolation {
            violation: EffectViolation {
                replacement: false,
                event: 0,
                rule: 0,
                kind: EffectViolationKind::Value,
            },
        });
        let diff = Some(ComparisonVerdict::Diff);
        check(
            &request,
            &strict,
            &rows(std::slice::from_ref(&event), difference.clone()),
            diff,
        )
        .unwrap();
        if let Some(ComparisonDifference::EffectViolation { violation }) =
            &mut difference.difference
        {
            violation.event = 1;
        }
        assert!(check(&request, &strict, &rows(&[event], difference), diff).is_err());
        // Every case gets fresh exercise accounting, including a warm continuation.
        let mut phases = request.clone();
        let mut second = phases.cases[0].clone();
        second.name = "second".into();
        second.reset = SessionReset::Warm;
        phases.cases.push(second);
        let mut both = first;
        let unexercised = CaseComparison {
            effect_claim: result.effect_claim,
            effect_gap: Some(EffectGap::Unexercised {
                replacement: false,
                rule: 0,
            }),
            verdict: ComparisonVerdict::Incomplete,
            difference: None,
        };
        for mut row in rows(&[], unexercised) {
            match &mut row {
                ExecutionEvidence::Outcome { case, .. }
                | ExecutionEvidence::Comparison { case, .. } => *case = 1,
                _ => unreachable!(),
            };
            both.push(row);
        }
        check(&phases, &effects, &both, incomplete).unwrap();
        if let ExecutionEvidence::Comparison { result: r, .. } = both.last_mut().unwrap() {
            *r = result;
        }
        assert!(check(&phases, &effects, &both, matched).is_err());
    }
}
