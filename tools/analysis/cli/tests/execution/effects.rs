use super::*;
fn setup(omit: bool) -> (Fixture, ExecutionRequest, EffectContract) {
    let vendor = [0x00b52023, 0x00000513, 0x00008067];
    let replacement = if omit {
        [0x00000013, 0x00000513, 0x00008067]
    } else {
        vendor
    };
    let (a, pa) = super::goals::symbol_elf(&vendor, 0x1000, 0x1000);
    let (b, pb) = super::goals::symbol_elf(&replacement, 0x1000, 0x1000);
    let f = Fixture::from_inputs(vec![a, b]);
    let endpoint = |point: SymbolId| CallEndpoint {
        object: point.object.clone(),
        symbol: Some(point),
        boundary: ReviewedCallBoundary::Code { address: 0x1000 },
    };
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
    let p = EffectContract {
        unclassified: UnclassifiedEffects::Incomplete,
        vendor: endpoint(pa),
        replacement: endpoint(pb),
        rules: vec![EffectRule {
            name: "initial register write".into(),
            vendor: Some(pattern),
            replacement: Some(pattern),
            disposition: if omit {
                EffectDisposition::Omitted
            } else {
                EffectDisposition::Required
            },
            min_occurrences: 1,
            max_occurrences: 1,
            reason: "explicit synthetic refinement".into(),
        }],
        claim_ceiling: EffectClaimCeiling::ReviewedEffectRefinement,
        applicability: "exact fixture root inputs".into(),
        reason: "synthetic effect policy".into(),
    };
    let mut r = f.request();
    r.replacement.as_mut().unwrap().executables = vec![f.id(1)];
    let v = &mut r.cases[0].vendor;
    v.arguments[0] = Some(0x3000);
    v.arguments[1] = Some(7);
    v.models = vec![register_bank(vec![RegisterCell {
        address: 0x3000,
        width: 4,
        value: 0,
    }])];
    r.cases[0].replacement = Some(v.clone());
    (f, r, p)
}
fn select(r: &mut ExecutionRequest, p: &EffectContract) {
    r.cases[0].relation.as_mut().unwrap().effects =
        Some(app::in_process::effect_contract_id(p).unwrap());
}
fn verify(
    f: &Fixture,
    r: &ExecutionRequest,
    p: &EffectContract,
) -> Result<(Verified, Vec<ExecutionEvidence>)> {
    super::verify(f, r, std::slice::from_ref(p), &[])
}
#[test]
fn content_effect_contract_preserves_raw_omissions() {
    let (f, mut r, p) = setup(true);
    assert_eq!(run(&f, r.clone()).0.verdict, Some(ComparisonVerdict::Diff));
    select(&mut r, &p);
    assert!(super::verify(&f, &r, &[], &[]).is_err());
    let (m, rows) = verify(&f, &r, &p).unwrap();
    assert_eq!(m.verdict, Some(ComparisonVerdict::Match));
    assert!(rows.iter().any(|r| matches!(
        r,
        ExecutionEvidence::Event {
            replacement: false,
            event: ExecutionEvent::Write {
                address: 0x3000,
                value: 7,
                ..
            },
            ..
        }
    )));
    assert!(rows.iter().any(|r| matches!(
        r,
        ExecutionEvidence::Comparison {
            result: CaseComparison {
                effect_claim: Some(EffectClaimCeiling::ReviewedEffectRefinement),
                effect_gap: None,
                verdict: ComparisonVerdict::Match,
                ..
            },
            ..
        }
    )));
}
#[test]
fn effect_replacement_requires_exact_values_and_case_applicability() {
    let (f, mut r, mut p) = setup(false);
    r.cases[0].replacement.as_mut().unwrap().arguments[1] = Some(9);
    p.rules[0].disposition = EffectDisposition::Replaced;
    p.rules[0].vendor.as_mut().unwrap().value = EffectValue::Exact { value: 7 };
    p.rules[0].replacement.as_mut().unwrap().value = EffectValue::Exact { value: 9 };
    select(&mut r, &p);
    assert_eq!(
        verify(&f, &r, &p).unwrap().0.verdict,
        Some(ComparisonVerdict::Match)
    );
    let mut wrong = r.clone();
    wrong.cases[0].replacement.as_mut().unwrap().arguments[1] = Some(8);
    let (m, rows) = verify(&f, &wrong, &p).unwrap();
    assert_eq!(m.verdict, Some(ComparisonVerdict::Diff));
    assert!(rows.iter().any(|r| matches!(
        r,
        ExecutionEvidence::Comparison {
            result: CaseComparison {
                difference: Some(ComparisonDifference::EffectViolation {
                    violation: EffectViolation {
                        replacement: true,
                        kind: EffectViolationKind::Value,
                        ..
                    }
                }),
                ..
            },
            ..
        }
    )));
    for variant in 0..4 {
        let mut wrong = r.clone();
        match variant {
            0 => wrong.cases[0].replacement.as_mut().unwrap().entry = 0x1004,
            // The contract's replacement entry is not in the executable mapped.
            1 => {
                wrong.replacement.as_mut().unwrap().executables =
                    vec![ArtifactId::of_bytes(b"an executable without the entry")]
            }
            2 => {
                let mut other = p.clone();
                other.reason = "a contract the comparison does not receive".into();
                select(&mut wrong, &other)
            }
            _ => wrong.cases[0].relation.as_mut().unwrap().events.delay = false,
        }
        assert!(verify(&f, &wrong, &p).is_err(), "variant {variant}");
    }
}

#[test]
fn effect_policy_composes_with_layout_timeline_returns_and_final_ram() {
    // RAM write/read, conditional branch, omittable MMIO, captured call and explicit root return.
    let code = [
        0x00008413, 0x00b52023, 0x00052283, 0x00060463, 0x00158593, 0x00e6a023, 0x018000ef,
        0x00000513, 0x00040067, 0x00000013, 0x00000013, 0x00000013, 0x00008067,
    ];
    let mut right = vec![0x00000013];
    right.extend(code);
    right[6] = 0x00000013;
    let (a, pa) = super::goals::symbol_elf(&code, 0x1000, 0x1030);
    let (b, pb) = super::goals::symbol_elf(&right, 0x1000, 0x1034);
    let f = Fixture::from_inputs(vec![a, b]);
    let endpoint = |point: SymbolId, address| CallEndpoint {
        object: point.object.clone(),
        symbol: Some(point),
        boundary: ReviewedCallBoundary::Code { address },
    };
    let callee_a = endpoint(pa, 0x1030);
    let callee_b = endpoint(pb, 0x1034);
    let mut root_a = callee_a.clone();
    root_a.boundary = ReviewedCallBoundary::Code { address: 0x1000 };
    root_a.symbol.as_mut().unwrap().index = 1;
    let mut root_b = callee_b.clone();
    root_b.boundary = ReviewedCallBoundary::Code { address: 0x1000 };
    root_b.symbol.as_mut().unwrap().index = 1;
    let layout = LayoutProjection {
        vendor: LayoutEndpoint {
            entry: root_a.clone(),
            domains: vec![LayoutDomain {
                address: 0x3000,
                length: 4,
            }],
        },
        replacement: LayoutEndpoint {
            entry: root_b.clone(),
            domains: vec![LayoutDomain {
                address: 0x5000,
                length: 8,
            }],
        },
        fields: vec![LayoutField {
            name: "counter".into(),
            vendor: FieldLocation {
                domain: 0,
                offset: 0,
            },
            replacement: FieldLocation {
                domain: 0,
                offset: 4,
            },
            width: 4,
            count: 1,
            final_state: true,
            timeline: true,
        }],
        branches: vec![BranchPair {
            vendor: BranchLocation {
                site: 0x100c,
                target: 0x1014,
                fallthrough: 0x1010,
            },
            replacement: BranchLocation {
                site: 0x1010,
                target: 0x1018,
                fallthrough: 0x1014,
            },
        }],
        applicability: "synthetic root entries".into(),
        reason: "same counter and branch".into(),
    };
    let pattern = EffectPattern {
        preceded_by: None,
        occurrence: None,
        followed_by: None,
        selector: EffectSelector::MmioWrite {
            address: 0x6000,
            width: 4,
        },
        value: EffectValue::Exact { value: 11 },
    };
    let contract = EffectContract {
        unclassified: UnclassifiedEffects::Incomplete,
        vendor: root_a,
        replacement: root_b,
        rules: vec![EffectRule {
            name: "optional synthetic write".into(),
            vendor: Some(pattern),
            replacement: Some(pattern),
            disposition: EffectDisposition::Omitted,
            min_occurrences: 1,
            max_occurrences: 1,
            reason: "fixture relaxation".into(),
        }],
        claim_ceiling: EffectClaimCeiling::ReviewedEffectRefinement,
        applicability: "exact synthetic entries".into(),
        reason: "composition regression".into(),
    };
    let mut r = f.request();
    r.max_events = 128;
    r.replacement.as_mut().unwrap().executables = vec![f.id(1)];
    let capture = TimelineCapture {
        reads: true,
        writes: true,
        branches: true,
        atomics: false,
        written: false,
    };
    for side in [false, true] {
        let input = if side {
            r.cases[0].replacement.as_mut().unwrap()
        } else {
            &mut r.cases[0].vendor
        };
        let address = if side { 0x5000 } else { 0x3000 };
        let length = if side { 8 } else { 4 };
        input.arguments = vec![
            Some(if side { 0x5004 } else { 0x3000 }),
            Some(7),
            Some(0),
            Some(0x6000),
            Some(if side { 7 } else { 11 }),
            Some(0),
            Some(0),
            Some(0),
        ];
        input.memory = vec![
            ExecutionRegion {
                lifetime: RegionLifetime::Session,
                seed: MemorySeed {
                    address,
                    length,
                    fill: None,
                    bytes: vec![],
                },
            },
            ExecutionRegion {
                lifetime: RegionLifetime::Phase,
                seed: MemorySeed {
                    address: 0x7000,
                    length: 4,
                    fill: Some(9),
                    bytes: vec![],
                },
            },
        ];
        input.observe_memory = vec![
            MemorySelection {
                name: "layout".into(),
                address,
                length,
            },
            MemorySelection {
                name: "physical".into(),
                address: 0x7000,
                length: 4,
            },
        ];
        input.observe_timeline = capture;
        input.observe_calls = Some(CallCapture {
            include_tail: false,
            argument_words: 8,
            overrides: vec![],
        });
        input.models = vec![register_bank(vec![RegisterCell {
            address: 0x6000,
            width: 4,
            value: 0,
        }])];
    }
    let relation = r.cases[0].relation.as_mut().unwrap();
    relation.returns.high = true;
    relation.memory = vec![MemoryPair {
        vendor: 1,
        replacement: 1,
    }];
    relation.events.timeline = capture;
    relation.projection = Some(app::in_process::projection_id(&layout).unwrap());
    relation.effects = Some(app::in_process::effect_contract_id(&contract).unwrap());
    let compare = |r: &ExecutionRequest| {
        super::verify(
            &f,
            r,
            std::slice::from_ref(&contract),
            std::slice::from_ref(&layout),
        )
        .unwrap()
    };
    let (m, rows) = compare(&r);
    assert_eq!(m.verdict, Some(ComparisonVerdict::Match));
    assert!(rows.iter().any(|r| matches!(
        r,
        ExecutionEvidence::Event {
            event: ExecutionEvent::CallTransfer { target: 0x1034, .. },
            ..
        }
    )));
    for variant in 0..4 {
        let mut changed = r.clone();
        match variant {
            0 => changed.cases[0].relation.as_mut().unwrap().effects = None,
            1 => changed.cases[0].relation.as_mut().unwrap().projection = None,
            2 => changed.cases[0].replacement.as_mut().unwrap().arguments[2] = Some(1),
            _ => {
                changed.cases[0].replacement.as_mut().unwrap().memory[1]
                    .seed
                    .fill = Some(8)
            }
        }
        let (manifest, rows) = compare(&changed);
        assert_eq!(
            manifest.verdict,
            Some(ComparisonVerdict::Diff),
            "variant {variant}: {rows:?}"
        );
    }
}
