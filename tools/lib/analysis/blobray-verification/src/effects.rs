//! Complete policy accounting precedes alignment; excluded raw effects remain evidence.
use blobray_domain::*;
use oer_riscv_model::*;

pub(super) struct Report {
    pub claim: Option<EffectClaimCeiling>,
    pub gap: Option<EffectGap>,
    pub violation: Option<EffectViolation>,
}

pub(super) fn inspect(
    left: &ExecutionObservation,
    right: &ExecutionObservation,
    relation: &ComparisonRelation,
    selected: Option<&ResolvedEffectContract>,
    c: &mut dyn RunControl,
) -> Result<Report> {
    if relation.effects.as_ref() != selected.map(|s| &s.id) {
        return Err(Error::new(
            ErrorCode::Integrity,
            "selected effect review differs from comparison input",
        ));
    }
    let mut report = Report {
        claim: None,
        gap: None,
        violation: None,
    };
    let Some(selected) = selected else {
        return Ok(report);
    };
    c.checkpoint((selected.contract.rules.len() as u64 + 1).saturating_pow(2))?;
    selected.contract.validate()?;
    if !(relation.events.mmio_read
        && relation.events.mmio_write
        && relation.events.fence
        && relation.events.delay)
    {
        return Err(Error::new(
            ErrorCode::InvalidRequest,
            "effect contract requires all concrete effect channels",
        ));
    }
    report.claim = Some(selected.contract.claim_ceiling);
    for (replacement, observation) in [(false, left), (true, right)] {
        let tracker = track(
            &selected.contract,
            &observation.events,
            replacement,
            |_, _| {},
            c,
        )?;
        report.gap = report.gap.or(tracker.gap());
        report.violation = report.violation.or(tracker.first_violation());
    }
    Ok(report)
}

/// Run `contract`'s tracker for one side over `events`, reporting each
/// concrete effect's event index and selection to `selected`.
fn track<'a>(
    contract: &'a EffectContract,
    events: &[ExecutionEvent],
    replacement: bool,
    mut selected: impl FnMut(usize, EffectSelection),
    c: &mut dyn RunControl,
) -> Result<EffectTracker<'a>> {
    let mut tracker = EffectTracker::new(contract, replacement, c)?;
    let mut effects = events
        .iter()
        .enumerate()
        .filter(|(_, e)| is_contract_effect(e))
        .peekable();
    while let Some((index, event)) = effects.next() {
        c.checkpoint(1)?;
        let ordinal = u32::try_from(index)
            .map_err(|_| Error::new(ErrorCode::Integrity, "effect ordinal overflow"))?;
        let next = effects.peek().map(|(_, e)| *e);
        selected(index, tracker.observe(event, next, ordinal, c)?);
    }
    Ok(tracker)
}

/// The selection `contract` gives each concrete effect of one side's
/// `events`, by event index: the classification the comparison applies.
pub fn classify(
    contract: &EffectContract,
    events: &[ExecutionEvent],
    replacement: bool,
    c: &mut dyn RunControl,
) -> Result<Vec<(usize, EffectSelection)>> {
    let mut selections = vec![];
    track(
        contract,
        events,
        replacement,
        |index, selection| selections.push((index, selection)),
        c,
    )?;
    Ok(selections)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn endpoint() -> CallEndpoint {
        let id = ArtifactId::of_bytes(b"effect fixture");
        let object = ObjectId {
            artifact: id.clone(),
            location: ObjectLocation::Standalone,
        };
        CallEndpoint {
            object: object.clone(),
            symbol: Some(SymbolId {
                object,
                table: SymbolTableKind::Static,
                table_section: 3,
                index: 1,
            }),
            boundary: ReviewedCallBoundary::Code { address: 0x1000 },
        }
    }
    fn write(value: u32) -> ExecutionEvent {
        ExecutionEvent::Write {
            address: 0x4000,
            width: 4,
            value,
        }
    }
    fn pattern(selector: EffectSelector) -> EffectPattern {
        EffectPattern {
            preceded_by: None,
            occurrence: None,
            followed_by: None,
            selector,
            value: EffectValue::Any,
        }
    }
    fn rule(disposition: EffectDisposition) -> EffectRule {
        let p = pattern(EffectSelector::MmioWrite {
            address: 0x4000,
            width: 4,
        });
        EffectRule {
            name: "register".into(),
            vendor: Some(p),
            replacement: Some(p),
            disposition,
            min_occurrences: 1,
            max_occurrences: 8,
            reason: "reviewed synthetic obligation".into(),
        }
    }
    fn policy(rules: Vec<EffectRule>) -> ResolvedEffectContract {
        let id = ArtifactId::of_bytes(b"effect fixture");
        ResolvedEffectContract {
            id: id.clone(),
            contract: EffectContract {
                unclassified: UnclassifiedEffects::Incomplete,
                vendor: endpoint(),
                replacement: endpoint(),
                rules,
                claim_ceiling: EffectClaimCeiling::ReviewedEffectRefinement,
                applicability: "exact fixture entries".into(),
                reason: "synthetic policy".into(),
            },
        }
    }
    fn relation(p: &ResolvedEffectContract) -> ComparisonRelation {
        ComparisonRelation {
            effects: Some(p.id.clone()),
            projection: None,
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
            calls: false,
        }
    }
    fn observed(events: Vec<ExecutionEvent>) -> ExecutionObservation {
        ExecutionObservation {
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
        }
    }
    fn compare(
        p: &ResolvedEffectContract,
        a: Vec<ExecutionEvent>,
        b: Vec<ExecutionEvent>,
    ) -> CaseComparison {
        super::super::compare(
            &observed(a),
            &observed(b),
            &relation(p),
            None,
            Some(p),
            &mut || Ok(()),
        )
        .unwrap()
    }
    fn read(address: u32, value: u32) -> ExecutionEvent {
        ExecutionEvent::Read {
            address,
            width: 4,
            value,
        }
    }
    fn delay(value: u32) -> ExecutionEvent {
        ExecutionEvent::DelayMicros { value }
    }
    /// Unlisted effects compare exactly; transport reads and the one-microsecond
    /// delay immediately before one are ignored plumbing.
    fn plumbing() -> ResolvedEffectContract {
        let transport = EffectSelector::MmioRead {
            address: 0x5000,
            width: 4,
        };
        let ignored = |name: &str, pattern: EffectPattern| EffectRule {
            name: name.into(),
            vendor: Some(pattern),
            replacement: Some(pattern),
            disposition: EffectDisposition::Ignored,
            min_occurrences: 0,
            max_occurrences: 64,
            reason: "transport plumbing".into(),
        };
        let mut p = policy(vec![
            ignored("transport-read", pattern(transport)),
            ignored(
                "transport-wait",
                EffectPattern {
                    preceded_by: None,
                    occurrence: None,
                    followed_by: Some(transport),
                    ..pattern(EffectSelector::Delay { micros: Some(1) })
                },
            ),
        ]);
        p.contract.unclassified = UnclassifiedEffects::Required;
        p
    }
    #[test]
    fn classification_names_each_effect_by_its_event_index() {
        let p = plumbing();
        let events = vec![
            ExecutionEvent::CallReturn {
                words: [Some(0), None],
            },
            delay(1),
            read(0x5000, 9),
            write(1),
        ];
        let selections = classify(&p.contract, &events, true, &mut || Ok(())).unwrap();
        // Indices count every event, as the comparison's ordinals do; the
        // wait before a transport read is plumbing only in that context.
        assert_eq!(
            selections,
            [
                (1, EffectSelection::Ignored(1)),
                (2, EffectSelection::Ignored(0)),
                (3, EffectSelection::Unlisted),
            ]
        );
    }

    #[test]
    fn unlisted_effects_compare_exactly_around_ignored_plumbing() {
        let p = plumbing();
        p.contract.validate().unwrap();
        // Different transport polling and its waits do not matter.
        let vendor = vec![write(1), read(0x5000, 0), read(0x6000, 7), delay(10)];
        let production = vec![
            write(1),
            delay(1),
            read(0x5000, 9),
            read(0x5000, 9),
            read(0x6000, 7),
            delay(10),
        ];
        let result = compare(&p, vendor.clone(), production);
        assert_eq!(result.verdict, ComparisonVerdict::Match);
        assert_eq!(result.effect_gap, None);
        // A one-microsecond wait before anything else stays a compared effect.
        let visible = vec![write(1), delay(1), read(0x6000, 7), delay(10)];
        assert_eq!(
            compare(&p, vendor.clone(), visible).verdict,
            ComparisonVerdict::Diff
        );
        // Unlisted values and order still compare.
        assert_eq!(
            compare(
                &p,
                vendor.clone(),
                vec![write(2), read(0x6000, 7), delay(10)]
            )
            .verdict,
            ComparisonVerdict::Diff
        );
        // A register only one side reads from the environment is a difference.
        let environment = vec![write(1), read(0x7000, 0), read(0x6000, 7), delay(10)];
        assert_eq!(
            compare(&p, vendor.clone(), environment).verdict,
            ComparisonVerdict::Diff
        );
        // Without the explicit policy, the same unlisted effects are unclassified.
        let mut strict = p.clone();
        strict.contract.unclassified = UnclassifiedEffects::Incomplete;
        assert_eq!(
            compare(&strict, vendor.clone(), vendor).verdict,
            ComparisonVerdict::Incomplete
        );
    }
    #[test]
    fn context_selectors_overlap_only_when_both_contexts_can() {
        let delay_before = |address| EffectPattern {
            preceded_by: None,
            occurrence: None,
            followed_by: Some(EffectSelector::MmioRead { address, width: 4 }),
            ..pattern(EffectSelector::Delay { micros: Some(1) })
        };
        let rule = |name: &str, p: EffectPattern| EffectRule {
            name: name.into(),
            vendor: Some(p),
            replacement: Some(p),
            disposition: EffectDisposition::Ignored,
            min_occurrences: 0,
            max_occurrences: 4,
            reason: "plumbing".into(),
        };
        let mut distinct = plumbing();
        distinct.contract.rules = vec![
            rule("a", delay_before(0x5000)),
            rule("b", delay_before(0x5004)),
        ];
        distinct.contract.validate().unwrap();
        let mut ambiguous = distinct.clone();
        ambiguous.contract.rules[1] = rule("b", pattern(EffectSelector::Delay { micros: Some(1) }));
        assert!(ambiguous.contract.validate().is_err());
        // An ignored rule never constrains values.
        let mut exact = distinct;
        exact.contract.rules[0].vendor.as_mut().unwrap().value = EffectValue::Exact { value: 1 };
        exact.contract.rules[0].replacement = exact.contract.rules[0].vendor;
        assert!(exact.contract.validate().is_err());
    }
    #[test]
    fn omission_preserves_order_and_values_of_every_retained_effect() {
        let p = policy(vec![rule(EffectDisposition::Omitted)]);
        for right in [
            vec![],
            vec![write(2)],
            vec![write(1), write(3)],
            vec![write(1), write(2), write(3)],
        ] {
            let result = compare(&p, vec![write(1), write(2), write(3)], right);
            assert_eq!(result.verdict, ComparisonVerdict::Match);
            assert_eq!(
                result.effect_claim,
                Some(EffectClaimCeiling::ReviewedEffectRefinement)
            );
            assert_eq!(result.effect_gap, None);
        }
        for right in [
            vec![write(4)],
            vec![write(3), write(1)],
            vec![write(2), write(2)],
        ] {
            assert_eq!(
                compare(&p, vec![write(1), write(2), write(3)], right).verdict,
                ComparisonVerdict::Diff
            );
        }
        let result = compare(&p, vec![], vec![]);
        assert_eq!(result.verdict, ComparisonVerdict::Incomplete);
        assert_eq!(
            result.effect_gap,
            Some(EffectGap::Unexercised {
                replacement: false,
                rule: 0
            })
        );
    }
    #[test]
    fn required_replaced_added_and_forbidden_effects_have_distinct_obligations() {
        let required = policy(vec![rule(EffectDisposition::Required)]);
        assert_eq!(
            compare(&required, vec![write(1)], vec![write(1)]).verdict,
            ComparisonVerdict::Match
        );
        assert_eq!(
            compare(&required, vec![write(1)], vec![write(2)]).verdict,
            ComparisonVerdict::Diff
        );
        assert_eq!(
            compare(&required, vec![write(1)], vec![]).verdict,
            ComparisonVerdict::Diff
        );
        let mut replaced = rule(EffectDisposition::Replaced);
        replaced.vendor.as_mut().unwrap().value = EffectValue::Exact { value: 7 };
        replaced.replacement = Some(EffectPattern {
            preceded_by: None,
            occurrence: None,
            followed_by: None,
            selector: EffectSelector::Delay { micros: Some(9) },
            value: EffectValue::Exact { value: 9 },
        });
        let mut added = rule(EffectDisposition::Added);
        added.name = "barrier".into();
        added.vendor = None;
        added.replacement = Some(pattern(EffectSelector::Fence {
            predecessor: 3,
            successor: 3,
        }));
        let fence = ExecutionEvent::Fence {
            predecessor: 3,
            successor: 3,
        };
        let delay = ExecutionEvent::DelayMicros { value: 9 };
        let p = policy(vec![replaced, added]);
        assert_eq!(
            compare(&p, vec![write(7)], vec![fence.clone(), delay.clone()]).verdict,
            ComparisonVerdict::Match
        );
        assert_eq!(
            compare(&p, vec![write(7)], vec![delay.clone()]).verdict,
            ComparisonVerdict::Incomplete
        );
        let bad = compare(&p, vec![write(8)], vec![delay.clone(), fence]);
        assert_eq!(
            bad.difference,
            Some(ComparisonDifference::EffectViolation {
                violation: EffectViolation {
                    replacement: false,
                    event: 0,
                    rule: 0,
                    kind: EffectViolationKind::Value
                }
            })
        );
        let mut forbidden = rule(EffectDisposition::Forbidden);
        forbidden.min_occurrences = 0;
        forbidden.max_occurrences = 0;
        let p = policy(vec![forbidden]);
        assert_eq!(
            compare(&p, vec![], vec![]).verdict,
            ComparisonVerdict::Match
        );
        assert_eq!(
            compare(&p, vec![write(0)], vec![write(0)]).difference,
            Some(ComparisonDifference::EffectViolation {
                violation: EffectViolation {
                    replacement: false,
                    event: 0,
                    rule: 0,
                    kind: EffectViolationKind::Forbidden
                }
            })
        );
        let mut r = rule(EffectDisposition::Required);
        r.max_occurrences = 1;
        let p = policy(vec![r]);
        assert_eq!(
            compare(&p, vec![write(1), write(1)], vec![write(1)]).difference,
            Some(ComparisonDifference::EffectViolation {
                violation: EffectViolation {
                    replacement: false,
                    event: 1,
                    rule: 0,
                    kind: EffectViolationKind::Count
                }
            })
        );
    }
    #[test]
    fn unknown_alignment_is_incomplete_but_independent_known_differences_survive() {
        let p = policy(vec![rule(EffectDisposition::Required)]);
        let unknown = ExecutionEvent::DelayMicros { value: 3 };
        let a = observed(vec![unknown.clone(), write(1)]);
        let mut b = observed(vec![write(2)]);
        let compare = |b: &ExecutionObservation| {
            super::super::compare(&a, b, &relation(&p), None, Some(&p), &mut || Ok(())).unwrap()
        };
        assert_eq!(compare(&b).verdict, ComparisonVerdict::Incomplete);
        assert_eq!(
            compare(&b).effect_gap,
            Some(EffectGap::Unclassified {
                replacement: false,
                event: 0
            })
        );
        b.stop = ExecutionStop::Returned {
            low: Some(7),
            high: None,
        };
        assert!(matches!(
            compare(&b).difference,
            Some(ComparisonDifference::Return {
                vendor: 0,
                replacement: 7,
                ..
            })
        ));
        let a = observed(vec![write(1)]);
        let mut b = a.clone();
        b.stop = ExecutionStop::BlockedByPriorPhase;
        assert_eq!(
            super::super::compare(&a, &b, &relation(&p), None, Some(&p), &mut || Ok(()))
                .unwrap()
                .verdict,
            ComparisonVerdict::Incomplete
        );
        // A violation after an unknown event is still directly observable; no alignment inference.
        let mut strict = p.clone();
        strict.contract.rules[0].vendor.as_mut().unwrap().value = EffectValue::Exact { value: 7 };
        strict.contract.rules[0].replacement = strict.contract.rules[0].vendor;
        assert!(matches!(
            self::compare(&strict, vec![unknown, write(9)], vec![write(7)]).difference,
            Some(ComparisonDifference::EffectViolation {
                violation: EffectViolation { event: 1, .. }
            })
        ));
    }
    #[test]
    fn policies_reject_ambiguous_rules_invalid_geometry_and_false_claim_ceilings() {
        let p = policy(vec![rule(EffectDisposition::Required)]);
        p.contract.validate().unwrap();
        let mut bad = Vec::new();
        let mut x = p.clone();
        x.contract.rules.clear();
        bad.push(x);
        let mut x = p.clone();
        let mut duplicate = x.contract.rules[0].clone();
        duplicate.name = "other".into();
        x.contract.rules.push(duplicate);
        bad.push(x);
        let mut x = p.clone();
        x.contract.rules[0].min_occurrences = 9;
        bad.push(x);
        let mut x = p.clone();
        x.contract.claim_ceiling = EffectClaimCeiling::SelectedEffectEquality;
        x.contract.rules[0].disposition = EffectDisposition::Omitted;
        bad.push(x);
        for width in [0, 3, 8] {
            let mut x = p.clone();
            let pat = pattern(EffectSelector::MmioWrite {
                address: 0x4000,
                width,
            });
            x.contract.rules[0].vendor = Some(pat);
            x.contract.rules[0].replacement = Some(pat);
            bad.push(x);
        }
        let mut x = p.clone();
        let pat = EffectPattern {
            preceded_by: None,
            occurrence: None,
            followed_by: None,
            selector: EffectSelector::MmioWrite {
                address: 0x4000,
                width: 1,
            },
            value: EffectValue::Exact { value: 256 },
        };
        x.contract.rules[0].vendor = Some(pat);
        x.contract.rules[0].replacement = Some(pat);
        bad.push(x);
        let mut x = p.clone();
        let mut r = rule(EffectDisposition::Required);
        r.vendor = Some(pattern(EffectSelector::Delay { micros: None }));
        r.replacement = r.vendor;
        let mut other = r.clone();
        other.name = "specific".into();
        other.vendor = Some(pattern(EffectSelector::Delay { micros: Some(3) }));
        other.replacement = other.vendor;
        x.contract.rules = vec![r, other];
        bad.push(x);
        for x in bad {
            assert_eq!(
                x.contract.validate().unwrap_err().code,
                ErrorCode::InvalidRequest
            );
        }
        let a = observed(vec![write(1)]);
        assert_eq!(
            super::super::compare(&a, &a, &relation(&p), None, None, &mut || Ok(()))
                .unwrap_err()
                .code,
            ErrorCode::Integrity
        );
        let mut r = relation(&p);
        r.events.delay = false;
        assert_eq!(
            super::super::compare(&a, &a, &r, None, Some(&p), &mut || Ok(()))
                .unwrap_err()
                .code,
            ErrorCode::InvalidRequest
        );
        assert!(
            super::super::compare(&a, &a, &relation(&p), None, Some(&p), &mut || Err(
                Error::new(ErrorCode::Cancelled, "cancel")
            ))
            .is_err()
        );
    }
    #[test]
    fn malformed_concrete_effects_cannot_authorize_match() {
        let p = policy(vec![rule(EffectDisposition::Required)]);
        for event in [
            ExecutionEvent::Write {
                address: 0x4000,
                width: 0,
                value: 0,
            },
            ExecutionEvent::Read {
                address: 0x4000,
                width: 1,
                value: 256,
            },
            ExecutionEvent::Write {
                address: 0x4001,
                width: 4,
                value: 7,
            },
            ExecutionEvent::Fence {
                predecessor: 16,
                successor: 3,
            },
        ] {
            let a = observed(vec![event]);
            assert_eq!(
                super::super::compare(&a, &a, &relation(&p), None, Some(&p), &mut || Ok(()))
                    .unwrap_err()
                    .code,
                ErrorCode::Integrity
            );
        }
    }
    #[test]
    fn optional_and_added_effects_remain_ordered_against_calls_and_internal_memory() {
        let p = policy(vec![rule(EffectDisposition::Omitted)]);
        let memory = ExecutionEvent::Memory {
            site: 0x1000,
            transaction: MemoryTransaction::Write {
                address: 0x3000,
                width: 4,
                value: 7,
            },
        };
        let call = ExecutionEvent::CallTransfer {
            site: 0x1004,
            target: 0x2000,
            tail: false,
            indirect: false,
            stack: None,
            target_kind: ObservedCallTarget::CapturedCode,
            words: 0,
        };
        let mut r = relation(&p);
        r.events.timeline.writes = true;
        r.calls = true;
        let a = observed(vec![write(1), memory.clone(), call.clone(), write(2)]);
        for events in [
            vec![memory.clone(), call.clone(), write(2)],
            vec![write(1), memory.clone(), call.clone()],
        ] {
            let b = observed(events);
            assert_eq!(
                super::super::compare(&a, &b, &r, None, Some(&p), &mut || Ok(()))
                    .unwrap()
                    .verdict,
                ComparisonVerdict::Match
            );
        }
        for events in [
            vec![memory.clone(), write(1), call.clone()],
            vec![call, memory, write(2)],
        ] {
            let b = observed(events);
            assert_eq!(
                super::super::compare(&a, &b, &r, None, Some(&p), &mut || Ok(()))
                    .unwrap()
                    .verdict,
                ComparisonVerdict::Diff
            );
        }
    }

    #[test]
    fn occurrence_and_predecessor_patterns_tell_samples_of_one_register_apart() {
        const CONTROL: u32 = 0x2010_4080;
        const NEXT: u32 = 0x2010_4088;
        const LAST: u32 = 0x2010_408c;
        let read_of = |address| EffectSelector::MmioRead { address, width: 4 };
        let added = |name: &str, pattern: EffectPattern| EffectRule {
            name: name.into(),
            vendor: None,
            replacement: Some(pattern),
            disposition: EffectDisposition::Added,
            min_occurrences: 1,
            max_occurrences: 1,
            reason: "production proves release before the append".into(),
        };
        let first = |address| EffectPattern {
            occurrence: Some(1),
            ..pattern(read_of(address))
        };
        let mut p = policy(vec![
            added("settled-check", first(CONTROL)),
            added(
                "proof-last",
                EffectPattern {
                    preceded_by: Some(read_of(CONTROL)),
                    ..pattern(read_of(LAST))
                },
            ),
            added("proof-next", first(NEXT)),
        ]);
        // Effects no rule selects compare exactly.
        p.contract.unclassified = UnclassifiedEffects::Required;
        let doorbell = ExecutionEvent::Write {
            address: CONTROL,
            width: 4,
            value: 1,
        };
        let vendor = |settled| vec![read(CONTROL, 0), doorbell.clone(), read(NEXT, settled)];
        let production = |settled| {
            vec![
                read(CONTROL, 0),
                read(LAST, 0x11c),
                read(NEXT, 0x128),
                read(CONTROL, 0),
                doorbell.clone(),
                read(NEXT, settled),
            ]
        };
        // The proof samples are production's own; the settle sample and
        // every doorbell effect compare exactly.
        assert_eq!(
            compare(&p, vendor(0x128), production(0x128)).verdict,
            ComparisonVerdict::Match
        );
        assert_eq!(
            compare(&p, vendor(0x128), production(0x134)).verdict,
            ComparisonVerdict::Diff
        );
        // A LAST sample not preceded by the settled check is not a proof.
        let reordered = vec![
            read(LAST, 0x11c),
            read(CONTROL, 0),
            read(NEXT, 0x128),
            read(CONTROL, 0),
            doorbell.clone(),
            read(NEXT, 0x128),
        ];
        assert_ne!(
            compare(&p, vendor(0x128), reordered).verdict,
            ComparisonVerdict::Match
        );
    }

    #[test]
    fn distinct_occurrences_of_one_selector_do_not_overlap() {
        let selector = EffectSelector::MmioRead {
            address: 0x2010_4088,
            width: 4,
        };
        let nth = |n| EffectPattern {
            occurrence: Some(n),
            ..pattern(selector)
        };
        assert!(!nth(1).overlaps(nth(2)));
        assert!(nth(1).overlaps(nth(1)));
        assert!(nth(1).overlaps(pattern(selector)));
        let zero = EffectRule {
            vendor: Some(nth(0)),
            replacement: Some(nth(0)),
            ..rule(EffectDisposition::Ignored)
        };
        assert!(policy(vec![zero]).contract.validate().is_err());
    }
}
