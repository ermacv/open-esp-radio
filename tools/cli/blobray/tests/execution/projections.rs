use super::*;
fn setup() -> (Fixture, ExecutionRequest, LayoutProjection) {
    let code = [
        0x00b52023, 0x00052283, 0x00060463, 0x00158593, 0x00000513, 0x00008067,
    ];
    setup_code(&code, &[&[0x00000013][..], &code].concat())
}
fn setup_code(left: &[u32], right: &[u32]) -> (Fixture, ExecutionRequest, LayoutProjection) {
    let (a, pa) = super::goals::symbol_elf(left, 0x1000, 0x1000);
    let (b, pb) = super::goals::symbol_elf(right, 0x1000, 0x1000);
    let f = Fixture::from_inputs(vec![a, b]);
    let endpoint = |point: SymbolId, address, length| LayoutEndpoint {
        entry: CallEndpoint {
            object: point.object.clone(),
            symbol: Some(point),
            boundary: ReviewedCallBoundary::Code { address: 0x1000 },
        },
        domains: vec![LayoutDomain { address, length }],
    };
    let p = LayoutProjection {
        vendor: endpoint(pa, 0x3000, 16),
        replacement: endpoint(pb, 0x5000, 24),
        fields: vec![LayoutField {
            count: 1,
            name: "counter".into(),
            vendor: FieldLocation {
                domain: 0,
                offset: 0,
            },
            replacement: FieldLocation {
                domain: 0,
                offset: 8,
            },
            width: 4,
            final_state: true,
            timeline: true,
        }],
        branches: vec![BranchPair {
            vendor: BranchLocation {
                site: 0x1008,
                target: 0x1010,
                fallthrough: 0x100c,
            },
            replacement: BranchLocation {
                site: 0x100c,
                target: 0x1014,
                fallthrough: 0x1010,
            },
        }],
        applicability: "synthetic selected entry layouts".into(),
        reason: "reviewed same field and conditional decision".into(),
    };
    let mut r = f.request();
    r.replacement.as_mut().unwrap().executables = vec![f.id(1)];
    r.max_events = 128;
    for side in [false, true] {
        let input = if side {
            r.cases[0].replacement.as_mut().unwrap()
        } else {
            &mut r.cases[0].vendor
        };
        let domain = p.endpoint(side).domains[0];
        input.arguments = vec![Some(if side { 0x5008 } else { 0x3000 }), Some(7), Some(0)];
        input.memory = vec![ExecutionRegion {
            lifetime: RegionLifetime::Session,
            seed: MemorySeed {
                address: domain.address,
                length: domain.length,
                fill: None,
                bytes: vec![],
            },
        }];
        input.observe_memory = vec![MemorySelection {
            name: "layout including unknown padding".into(),
            address: domain.address,
            length: domain.length,
        }];
        input.observe_timeline = TimelineCapture {
            reads: true,
            writes: true,
            atomics: false,
            branches: true,
            written: false,
        };
    }
    let capture = r.cases[0].vendor.observe_timeline;
    let relation = r.cases[0].relation.as_mut().unwrap();
    relation.returns = ReturnWords {
        low: false,
        high: false,
    };
    relation.events = EventChannels {
        timeline: capture,
        mmio_read: false,
        mmio_write: false,
        fence: false,
        delay: false,
    };
    (f, r, p)
}
fn select(r: &mut ExecutionRequest, p: &LayoutProjection) {
    r.cases[0].relation.as_mut().unwrap().projection =
        Some(app::in_process::projection_id(p).unwrap());
}
fn verify(
    f: &Fixture,
    r: &ExecutionRequest,
    p: &LayoutProjection,
) -> Result<(Verified, Vec<ExecutionEvidence>)> {
    super::verify(f, r, &[], std::slice::from_ref(p))
}
#[test]
fn layout_projection_rebases_fields_and_branches_without_comparing_unknown_padding() {
    let (f, mut r, p) = setup();
    assert_eq!(run(&f, r.clone()).0.verdict, Some(ComparisonVerdict::Diff));
    select(&mut r, &p);
    assert!(super::verify(&f, &r, &[], &[]).is_err());
    let (m, rows) = verify(&f, &r, &p).unwrap();
    assert_eq!(m.verdict, Some(ComparisonVerdict::Match));
    assert!(
        rows.iter()
            .any(|r| matches!(r,ExecutionEvidence::FinalMemory {chunk,..} if !chunk.complete()))
    );
    assert!(rows.iter().any(|r| matches!(
        r,
        ExecutionEvidence::Event {
            event: ExecutionEvent::Memory {
                transaction: MemoryTransaction::Write {
                    address: 0x5008,
                    ..
                },
                ..
            },
            ..
        }
    )));
    r.cases[0].replacement.as_mut().unwrap().arguments[1] = Some(9);
    assert_eq!(
        verify(&f, &r, &p).unwrap().0.verdict,
        Some(ComparisonVerdict::Diff)
    );
}
#[test]
fn projected_final_fields_keep_unknowns_and_do_not_imply_timeline_equality() {
    let (f, mut r, mut p) = setup();
    p.fields[0].timeline = false;
    p.branches.clear();
    r.cases[0].relation.as_mut().unwrap().events.timeline = TimelineCapture::default();
    select(&mut r, &p);
    assert_eq!(
        verify(&f, &r, &p).unwrap().0.verdict,
        Some(ComparisonVerdict::Match)
    );
    r.cases[0].replacement.as_mut().unwrap().arguments[1] = Some(9);
    let (m, rows) = verify(&f, &r, &p).unwrap();
    assert_eq!(m.verdict, Some(ComparisonVerdict::Diff));
    assert!(rows.iter().any(|r| matches!(
        r,
        ExecutionEvidence::Comparison {
            result: CaseComparison {
                difference: Some(ComparisonDifference::ProjectedMemory {
                    field: 0,
                    offset: 0,
                    vendor: 7,
                    replacement: 9
                }),
                ..
            },
            ..
        }
    )));
    r.cases[0].replacement.as_mut().unwrap().arguments[1] = None;
    assert_eq!(
        verify(&f, &r, &p).unwrap().0.verdict,
        Some(ComparisonVerdict::Incomplete)
    );
}
#[test]
fn unmapped_selected_timeline_and_missing_final_capture_cannot_silently_match() {
    let (f, mut r, mut p) = setup();
    p.fields[0].timeline = false;
    p.branches.clear();
    select(&mut r, &p);
    // Capture/selection still requests reads, writes and branches: every unmapped event remains unknown.
    assert_eq!(
        verify(&f, &r, &p).unwrap().0.verdict,
        Some(ComparisonVerdict::Incomplete)
    );
    r.cases[0].replacement.as_mut().unwrap().observe_memory[0].length = 8;
    assert!(verify(&f, &r, &p).is_err());
}
#[test]
fn invalid_projection_geometry_and_applicability_fail_the_comparison() {
    let (f, r, p) = setup();
    for variant in 0..8 {
        let mut bad = p.clone();
        match variant {
            0 => bad.replacement.entry.boundary = ReviewedCallBoundary::Code { address: 0x1004 },
            1 => bad.branches[0].replacement.site = 0x1000,
            2 => bad.fields[0].replacement.offset = 24,
            3 => bad.fields[0].width = 3,
            4 => {
                let mut alias = bad.fields[0].clone();
                alias.name = "alias".into();
                bad.fields.push(alias);
            }
            5 => bad.replacement.domains[0].length = u32::MAX,
            6 => bad.fields[0].replacement.domain = 9,
            _ => bad.applicability.clear(),
        }
        let mut r = r.clone();
        select(&mut r, &bad);
        assert!(verify(&f, &r, &bad).is_err(), "variant {variant}");
    }
}
#[test]
fn completed_execution_with_unknown_selected_final_fields_is_incomplete() {
    let (f, mut r, mut p) = setup();
    p.fields[0].vendor.offset = 4;
    p.fields[0].replacement.offset = 12;
    p.fields[0].timeline = false;
    p.branches.clear();
    r.cases[0].relation.as_mut().unwrap().events.timeline = TimelineCapture::default();
    select(&mut r, &p);
    let (m, rows) = verify(&f, &r, &p).unwrap();
    assert!(m.complete);
    assert_eq!(m.verdict, Some(ComparisonVerdict::Incomplete));
    assert_eq!(
        rows.iter()
            .filter(|r| matches!(
                r,
                ExecutionEvidence::Outcome {
                    stop: ExecutionStop::Returned { .. },
                    ..
                }
            ))
            .count(),
        2
    );
}

#[test]
fn projected_arrays_compare_cross_chunk_snapshots_and_bulk_initialization_once() {
    let code = [0x00008413, 0x000022b7, 0x000280e7, 0x00040067];
    let (f, mut r, mut p) = setup_code(&code, &code);
    p.branches.clear();
    p.fields[0].count = 4;
    p.fields[0].replacement.offset = 16;
    p.replacement.domains[0].length = 32;
    r.cases[0].replacement.as_mut().unwrap().observe_memory[0].address = 0x5008;
    for side in [false, true] {
        let input = if side {
            r.cases[0].replacement.as_mut().unwrap()
        } else {
            &mut r.cases[0].vendor
        };
        input.memory.clear();
        input.arguments = vec![Some(16)];
        input.observe_timeline = TimelineCapture {
            writes: true,
            ..Default::default()
        };
        input.calls = vec![CallDeclaration {
            repetition: blobray_domain::CallRepetition::Finite,
            id: "allocate".into(),
            applicability: "test".into(),
            lifetime: RegionLifetime::Phase,
            binding: CallBinding {
                address: 0x2000,
                boundary: CallBoundary::Unmapped,
                allow_tail: false,
            },
            argument_words: 1,
            responses: vec![CallResponse {
                return_words: [Some(if side { 0x5010 } else { 0x3000 }), None],
                outputs: vec![],
                delay_micros: None,
                allocation: Some(CallAllocation {
                    address: if side { 0x5010 } else { 0x3000 },
                    size_argument: 0,
                    capacity: 16,
                    lifetime: RegionLifetime::Session,
                }),
            }],
        }];
    }
    r.cases[0].relation.as_mut().unwrap().events.timeline = TimelineCapture {
        writes: true,
        ..Default::default()
    };
    select(&mut r, &p);
    let (m, rows) = verify(&f, &r, &p).unwrap();
    assert_eq!(m.verdict, Some(ComparisonVerdict::Match));
    assert_eq!(
        rows.iter()
            .filter(|r| matches!(
                r,
                ExecutionEvidence::Event {
                    event: ExecutionEvent::Allocation { requested: 16, .. },
                    ..
                }
            ))
            .count(),
        2
    );
    r.cases[0].replacement.as_mut().unwrap().arguments[0] = Some(12);
    assert_eq!(
        verify(&f, &r, &p).unwrap().0.verdict,
        Some(ComparisonVerdict::Diff)
    );
    p.fields[0].count = 0;
    assert!(p.validate().is_err());
    p.fields[0].count = u32::MAX;
    assert!(p.validate().is_err());
}
