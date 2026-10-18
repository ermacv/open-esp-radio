use super::run;
use super::*;
fn profile(words: u16) -> CallCapture {
    CallCapture {
        include_tail: false,
        argument_words: words,
        overrides: vec![],
    }
}
fn select(r: &mut ExecutionRequest, words: u16) {
    r.max_events = 1024;
    for phase in &mut r.cases {
        phase.vendor.observe_calls = Some(profile(words));
        phase.replacement.as_mut().unwrap().observe_calls = Some(profile(words));
        phase.relation = Some(ComparisonRelation {
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
        });
    }
}
fn events(rows: &[ExecutionEvidence]) -> Vec<&ExecutionEvent> {
    rows.iter()
        .filter_map(|r| match r {
            ExecutionEvidence::Event {
                replacement: false,
                event,
                ..
            } => Some(event),
            _ => None,
        })
        .collect()
}
fn diff(rows: &[ExecutionEvidence]) -> Option<&ComparisonDifference> {
    rows.iter().find_map(|r| match r {
        ExecutionEvidence::Comparison { result, .. } => result.difference.as_ref(),
        _ => None,
    })
}
#[test]
fn physical_capture_compares_register_and_stack_words_and_retains_exclusions() {
    // save ra, call callee at 0x1010, return through s0; callee leaves arguments unchanged.
    let f = Fixture::new(&[0x00008413, 0x00c000ef, 0x00040067, 0x00000013, 0x00008067]);
    let mut r = f.request();
    r.cases[0].vendor.arguments.extend([Some(42), Some(99)]);
    r.cases[0].replacement = Some(r.cases[0].vendor.clone());
    select(&mut r, 10);
    let (m, rows) = run(&f, r.clone());
    assert_eq!(m.verdict, Some(ComparisonVerdict::Match));
    let e = events(&rows);
    assert_eq!(e.len(), 11); // neither canonical return nor root sentinel are calls
    assert!(matches!(
        e[0],
        ExecutionEvent::CallTransfer {
            site: 0x1004,
            target: 0x1010,
            tail: false,
            indirect: false,
            target_kind: ObservedCallTarget::CapturedCode,
            words: 10,
            ..
        }
    ));
    assert_eq!(
        *e[9],
        ExecutionEvent::TransferArgument {
            word: 8,
            value: ObservedWord::Known { value: 42 }
        }
    );
    for word in [0, 7, 8, 9] {
        let mut changed = r.clone();
        changed.cases[0].replacement.as_mut().unwrap().arguments[word] = Some(123);
        let (m, rows) = run(&f, changed);
        assert_eq!(m.verdict, Some(ComparisonVerdict::Diff));
        assert!(
            matches!(diff(&rows), Some(ComparisonDifference::CallArgument { word: w, replacement: 123, .. }) if usize::from(*w)==word)
        );
    }
    r.cases[0].replacement.as_mut().unwrap().arguments[8] = None;
    let (m, _) = run(&f, r.clone());
    assert!(m.complete); // code completion is independent of selected argument knowledge
    assert_eq!(m.verdict, Some(ComparisonVerdict::Incomplete));
    r.cases[0].relation = Some(fixture_relation(true));
    let (m, rows) = run(&f, r);
    assert_eq!(m.verdict, Some(ComparisonVerdict::Match));
    assert!(
        events(&rows)
            .iter()
            .any(|e| matches!(e, ExecutionEvent::CallTransfer { .. }))
    );
}
#[test]
fn physical_target_order_and_effect_interleaving_are_compared() {
    // Vendor call then fence; replacement fence then call. Callee address is identical.
    let f = Fixture::from_inputs(vec![
        elf(&[
            0x00008413, 0x010000ef, 0x0ff0000f, 0x00040067, 0x00000013, 0x00008067,
        ]),
        elf(&[
            0x00008413, 0x0ff0000f, 0x00c000ef, 0x00040067, 0x00000013, 0x00008067,
        ]),
    ]);
    let mut r = f.request();
    r.replacement.as_mut().unwrap().executables = vec![f.id(1)];
    select(&mut r, 0);
    let (m, rows) = run(&f, r.clone());
    assert_eq!(m.verdict, Some(ComparisonVerdict::Diff));
    assert_eq!(diff(&rows), Some(&ComparisonDifference::Event { index: 0 }));
    r.cases[0].relation.as_mut().unwrap().events.fence = false;
    assert_eq!(run(&f, r).0.verdict, Some(ComparisonVerdict::Match));

    // Same indirect call site, different known target, with equal bodies and return values.
    let f = Fixture::new(&[
        0x00008413, 0x000580e7, 0x00040067, 0x00000013, 0x00008067, 0x00008067,
    ]);
    let mut r = f.request();
    select(&mut r, 0);
    r.cases[0].vendor.arguments[1] = Some(0x1010);
    r.cases[0].replacement.as_mut().unwrap().arguments[1] = Some(0x1014);
    let (m, rows) = run(&f, r);
    assert_eq!(m.verdict, Some(ComparisonVerdict::Diff));
    assert_eq!(
        diff(&rows),
        Some(&ComparisonDifference::CallTarget {
            index: 0,
            vendor: 0x1010,
            replacement: 0x1014
        })
    );
}
#[test]
fn capture_before_goal_and_explicit_tail_scope_preserve_physical_boundary() {
    for (code, tail, indirect) in [
        (
            vec![0x010000ef, 0x00008067, 0x00000013, 0x00000013, 0x00000073],
            false,
            false,
        ),
        (
            vec![0x010002ef, 0x00008067, 0x00000013, 0x00000013, 0x00000073],
            false,
            false,
        ),
        (
            vec![0x000580e7, 0x00008067, 0x00000013, 0x00000013, 0x00000073],
            false,
            true,
        ),
        (
            vec![0x0100006f, 0x00008067, 0x00000013, 0x00000013, 0x00000073],
            true,
            false,
        ),
    ] {
        let (f, point) = super::goals::fixture(&code, 0x1010);
        let mut r = f.request();
        select(&mut r, 1);
        {
            let i = &mut r.cases[0].vendor;
            i.goal = ExecutionGoal::ObserveCall {
                target: point.clone(),
                include_tail: true,
            };
            i.arguments[1] = Some(0x1010);
            i.observe_calls.as_mut().unwrap().include_tail = true;
        }
        r.cases[0].replacement = Some(r.cases[0].vendor.clone());
        let (m, rows) = run(&f, r.clone());
        assert!(m.complete);
        assert_eq!(m.verdict, Some(ComparisonVerdict::Match));
        assert_eq!(events(&rows).len(), 2);
        assert!(
            matches!(events(&rows)[0], ExecutionEvent::CallTransfer { tail: t, indirect: i, .. } if *t==tail && *i==indirect)
        );
        assert!(rows.iter().any(|r| matches!(
            r,
            ExecutionEvidence::Outcome {
                stop: ExecutionStop::ObservedCall { .. },
                ..
            }
        )));
        if tail {
            r.cases[0]
                .vendor
                .observe_calls
                .as_mut()
                .unwrap()
                .include_tail = false;
            r.cases[0].replacement = Some(r.cases[0].vendor.clone());
            assert!(events(&run(&f, r).1).is_empty());
        }
    }
}
#[test]
fn capture_stack_unavailability_does_not_block_code_or_invent_words() {
    for (prefix, reason) in [
        (0x00410113, WordAccess::MisalignedStack), // addi sp,sp,4
        (0x00002137, WordAccess::OutsidePrivateStack), // lui sp,2
        (0x00010113, WordAccess::OutsidePrivateStack), // entry sp=stack end, no incoming words
    ] {
        let f = Fixture::new(&[
            0x00008413, prefix, 0x00c000ef, 0x00040067, 0x00000013, 0x00008067,
        ]);
        let mut r = f.request();
        select(&mut r, 9);
        r.cases[0].vendor.arguments[2] = None;
        r.cases[0].replacement = Some(r.cases[0].vendor.clone());
        let (m, rows) = run(&f, r);
        assert!(m.complete);
        assert_eq!(m.verdict, Some(ComparisonVerdict::Incomplete));
        assert!(events(&rows).iter().any(|e| matches!(e, ExecutionEvent::TransferArgument { word: 8, value: ObservedWord::Unavailable { reason: r } } if *r==reason)));
    }
}
#[test]
fn modeled_boundaries_have_generic_capture_before_their_effects() {
    let f = Fixture::new(&[0x00008413, 0x000022b7, 0x000280e7, 0x00040067]);
    let mut r = f.request();
    select(&mut r, 1);
    r.cases[0].vendor.calls = vec![CallDeclaration {
        repetition: blobray_domain::CallRepetition::Finite,
        id: "delay".into(),
        applicability: "test".into(),
        lifetime: RegionLifetime::Phase,
        binding: CallBinding {
            address: 0x2000,
            boundary: CallBoundary::Unmapped,
            allow_tail: false,
        },
        argument_words: 1,
        responses: vec![CallResponse {
            return_words: [Some(0), None],
            outputs: vec![],
            allocation: None,
            delay_micros: Some(CallValue::Constant { value: 7 }),
        }],
    }];
    r.cases[0].replacement = Some(r.cases[0].vendor.clone());
    let (m, rows) = run(&f, r.clone());
    assert_eq!(m.verdict, Some(ComparisonVerdict::Match));
    assert!(matches!(
        events(&rows)[0],
        ExecutionEvent::CallTransfer {
            target_kind: ObservedCallTarget::CallModel,
            ..
        }
    ));
    r.cases[0].replacement.as_mut().unwrap().calls[0].responses[0].delay_micros =
        Some(CallValue::Constant { value: 9 });
    let (m, rows) = run(&f, r);
    assert_eq!(m.verdict, Some(ComparisonVerdict::Diff));
    assert_eq!(diff(&rows), Some(&ComparisonDifference::Event { index: 1 }));
}
#[test]
fn physical_call_words_count_against_event_capacity() {
    let f = Fixture::new(&[0x00008413, 0x00c000ef, 0x00040067, 0x00000013, 0x00008067]);
    let mut r = f.request();
    select(&mut r, 8);
    r.max_events = 8; // boundary plus eight words must be admitted together
    let failed = f.run(r, budget());
    assert_eq!(failed.unwrap_err().code, ErrorCode::ResourceLimited);
}
#[test]
fn invalid_capture_profiles_are_rejected_and_target_overrides_are_exact() {
    let f = Fixture::new(&[0x00008413, 0x00c000ef, 0x00040067, 0x00000013, 0x00008067]);
    let mut r = f.request();
    select(&mut r, 0);
    r.cases[0].vendor.observe_calls.as_mut().unwrap().overrides = vec![
        CallWordCount {
            target: 0x2000,
            words: 256,
        },
        CallWordCount {
            target: 0x1010,
            words: 1,
        },
    ];
    r.cases[0].replacement = Some(r.cases[0].vendor.clone());
    let (_, rows) = run(&f, r.clone());
    assert_eq!(events(&rows).len(), 2);
    for variant in 0..7 {
        let mut bad = r.clone();
        let p = bad.cases[0].vendor.observe_calls.as_mut().unwrap();
        match variant {
            0 => p.argument_words = 257,
            1 => p.overrides[0].words = 257,
            2 => p.overrides[0].target = 0x1001,
            3 => p.overrides[0].target = u32::MAX - 1,
            4 => p.overrides.push(p.overrides[0]),
            5 => p.overrides.resize(129, p.overrides[0]),
            _ => p.include_tail = true, // valid alone, mismatched exact profile
        }
        assert!(matches!(
            f.run(bad, budget()),
            Err(Error {
                code: ErrorCode::InvalidRequest,
                ..
            })
        ));
    }
}
