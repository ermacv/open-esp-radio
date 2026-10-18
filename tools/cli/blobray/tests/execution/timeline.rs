use super::run;
use super::*;
fn select(r: &mut ExecutionRequest, capture: TimelineCapture) {
    r.max_events = 128;
    for case in &mut r.cases {
        case.vendor.observe_timeline = capture;
        case.replacement.as_mut().unwrap().observe_timeline = capture;
        case.relation = Some(ComparisonRelation {
            effects: None,
            projection: None,
            returns: ReturnWords {
                low: false,
                high: false,
            },
            events: EventChannels {
                timeline: capture,
                mmio_read: false,
                mmio_write: false,
                fence: false,
                delay: false,
            },
            memory: vec![],
            calls: false,
        });
    }
}
fn ram_request(f: &Fixture) -> ExecutionRequest {
    let mut r = f.request();
    r.cases[0].vendor.arguments = vec![Some(0x3000), Some(7), Some(2)];
    r.cases[0].vendor.memory = vec![ExecutionRegion {
        lifetime: RegionLifetime::Session,
        seed: MemorySeed {
            address: 0x3000,
            length: 8,
            fill: Some(0),
            bytes: vec![],
        },
    }];
    r.cases[0].replacement = Some(r.cases[0].vendor.clone());
    r
}
fn transactions(rows: &[ExecutionEvidence]) -> Vec<(u32, MemoryTransaction)> {
    rows.iter()
        .filter_map(|r| match r {
            ExecutionEvidence::Event {
                replacement: false,
                event: ExecutionEvent::Memory { site, transaction },
                ..
            } => Some((*site, *transaction)),
            _ => None,
        })
        .collect()
}
#[test]
fn normal_memory_widths_values_and_instruction_sites_are_explicit() {
    for (width, store, load, expected) in [
        (1, 0x00b50023, 0x00054283, 0x78),
        (2, 0x00b51023, 0x00055283, 0x5678),
        (4, 0x00b52023, 0x00052283, 0x12345678),
    ] {
        let f = Fixture::new(&[store, load, 0x00000513, 0x00008067]);
        let mut r = ram_request(&f);
        r.cases[0].vendor.arguments[1] = Some(0x12345678);
        r.cases[0].replacement = Some(r.cases[0].vendor.clone());
        select(
            &mut r,
            TimelineCapture {
                reads: true,
                writes: true,
                ..Default::default()
            },
        );
        let (m, rows) = run(&f, r);
        assert_eq!(m.verdict, Some(ComparisonVerdict::Match));
        assert_eq!(
            transactions(&rows),
            vec![
                (
                    0x1000,
                    MemoryTransaction::Write {
                        address: 0x3000,
                        width,
                        value: expected
                    }
                ),
                (
                    0x1004,
                    MemoryTransaction::Read {
                        address: 0x3000,
                        width,
                        value: MemoryReadValue::Known { value: expected }
                    }
                )
            ]
        );
    }
}
#[test]
fn equal_final_ram_cannot_hide_different_intermediate_writes() {
    let f = Fixture::new(&[0x00b52023, 0x00c52023, 0x00000513, 0x00008067]);
    let mut r = ram_request(&f);
    r.cases[0].vendor.observe_memory = vec![MemorySelection {
        name: "output".into(),
        address: 0x3000,
        length: 4,
    }];
    r.cases[0].replacement = Some(r.cases[0].vendor.clone());
    r.cases[0].replacement.as_mut().unwrap().arguments[1] = Some(9);
    select(
        &mut r,
        TimelineCapture {
            writes: true,
            ..Default::default()
        },
    );
    r.cases[0].relation.as_mut().unwrap().memory = vec![MemoryPair {
        vendor: 0,
        replacement: 0,
    }];
    let (m, rows) = run(&f, r.clone());
    assert_eq!(m.verdict, Some(ComparisonVerdict::Diff));
    assert!(
        rows.iter()
            .filter_map(|r| match r {
                ExecutionEvidence::FinalMemory { chunk, .. } => Some(chunk),
                _ => None,
            })
            .all(|c| c.bytes[0] == 2)
    );
    r.cases[0].relation.as_mut().unwrap().events.timeline.writes = false;
    let (m, rows) = run(&f, r);
    assert_eq!(m.verdict, Some(ComparisonVerdict::Match));
    assert_eq!(transactions(&rows).len(), 2);
}
#[test]
fn read_order_and_branch_choices_are_independent_of_equal_returns() {
    let f = Fixture::from_inputs(vec![
        elf(&[0x00052283, 0x00452303, 0x00000513, 0x00008067]),
        elf(&[0x00452283, 0x00052303, 0x00000513, 0x00008067]),
    ]);
    let mut r = ram_request(&f);
    r.replacement.as_mut().unwrap().executables = vec![f.id(1)];
    select(
        &mut r,
        TimelineCapture {
            reads: true,
            ..Default::default()
        },
    );
    assert_eq!(run(&f, r).0.verdict, Some(ComparisonVerdict::Diff));
    let f = Fixture::from_inputs(vec![
        elf(&[0x00052283, 0x0ff0000f, 0x00008067]),
        elf(&[0x0ff0000f, 0x00052283, 0x00008067]),
    ]);
    let mut r = ram_request(&f);
    r.replacement.as_mut().unwrap().executables = vec![f.id(1)];
    select(
        &mut r,
        TimelineCapture {
            reads: true,
            ..Default::default()
        },
    );
    r.cases[0].relation.as_mut().unwrap().events.fence = true;
    assert_eq!(run(&f, r.clone()).0.verdict, Some(ComparisonVerdict::Diff));
    r.cases[0].relation.as_mut().unwrap().events.fence = false;
    assert_eq!(run(&f, r).0.verdict, Some(ComparisonVerdict::Match)); // load sites remain provenance
    for (code, fallthrough, target) in [
        (
            vec![0x00b50463, 0x00000013, 0x00000513, 0x00008067],
            0x1004,
            0x1008,
        ),
        (vec![0x0001c111, 0x00000513, 0x00008067], 0x1002, 0x1004),
    ] {
        let f = Fixture::new(&code);
        let mut r = f.request();
        r.cases[0].replacement.as_mut().unwrap().arguments[0] = Some(1);
        select(
            &mut r,
            TimelineCapture {
                branches: true,
                ..Default::default()
            },
        );
        let (m, rows) = run(&f, r.clone());
        assert_eq!(m.verdict, Some(ComparisonVerdict::Diff), "{rows:?}");
        assert!(rows.iter().any(|r| matches!(r, ExecutionEvidence::Event { replacement: false,
            event: ExecutionEvent::Branch { site: 0x1000, target: t, fallthrough: f, taken: true }, .. } if *t==target && *f==fallthrough)));
        r.cases[0].relation = Some(fixture_relation(true));
        assert_eq!(run(&f, r).0.verdict, Some(ComparisonVerdict::Match));
    }
}
#[test]
fn unknown_and_inaccessible_reads_are_not_equal_known_transactions() {
    let f = Fixture::new(&[0x00052283, 0x00000513, 0x00008067]);
    let mut r = ram_request(&f);
    r.cases[0].vendor.memory[0].seed.fill = None;
    r.cases[0].replacement = Some(r.cases[0].vendor.clone());
    select(
        &mut r,
        TimelineCapture {
            reads: true,
            ..Default::default()
        },
    );
    let (m, rows) = run(&f, r.clone());
    assert_eq!(m.verdict, Some(ComparisonVerdict::Incomplete));
    assert!(matches!(
        transactions(&rows)[0].1,
        MemoryTransaction::Read {
            value: MemoryReadValue::Unknown,
            ..
        }
    ));
    r.cases[0].replacement.as_mut().unwrap().memory[0].seed.fill = Some(0);
    assert_eq!(
        run(&f, r.clone()).0.verdict,
        Some(ComparisonVerdict::Incomplete)
    );
    r.cases[0].replacement.as_mut().unwrap().arguments[0] = Some(0x9000); // unowned access: gap, no invented RAM read
    let (_, rows) = run(&f, r);
    assert!(rows.iter().any(|r| matches!(
        r,
        ExecutionEvidence::Outcome {
            replacement: true,
            stop: ExecutionStop::Incomplete {
                reason: ExecutionGap::Memory {
                    address: 0x9000,
                    ..
                },
                ..
            },
            ..
        }
    )));
    let code = [0x00052283, 0x00000513, 0x00008067];
    let mut bytes = elf(&code);
    bytes[76..80].copy_from_slice(&1u32.to_le_bytes()); // executable-only load mapping
    let f = Fixture::from_inputs(vec![bytes]);
    let mut r = f.request();
    r.cases[0].vendor.arguments[0] = Some(0x1000);
    r.cases[0].replacement = Some(r.cases[0].vendor.clone());
    select(
        &mut r,
        TimelineCapture {
            reads: true,
            ..Default::default()
        },
    );
    let (m, rows) = run(&f, r);
    assert_eq!(m.verdict, Some(ComparisonVerdict::Incomplete));
    assert!(matches!(
        transactions(&rows)[0].1,
        MemoryTransaction::Read {
            value: MemoryReadValue::Unavailable,
            ..
        }
    ));
}
#[test]
fn atomic_timeline_retains_old_new_ordering_and_failed_sc() {
    use super::atomics::{op, scenario};
    for (function, old, operand, expected) in [
        (1, 9, 5, 5u32),
        (0, u32::MAX, 5, 4),
        (4, 0xaa, 0xf, 0xa5),
        (12, 0xaa, 0xf, 0xa),
        (8, 0xaa, 0xf, 0xaf),
        (16, 0x80000000, 5, 0x80000000),
        (20, 0x80000000, 5, 5),
        (24, 0x80000000, 5, 5),
        (28, 0x80000000, 5, 0x80000000),
    ] {
        for bits in 0..4 {
            let f = Fixture::new(&[op(function, bits, 5, 10, 11), 0x00008067]);
            let mut r = scenario(&f, Some(old));
            r.cases[0].vendor.arguments[1] = Some(operand);
            r.cases[0].replacement = Some(r.cases[0].vendor.clone());
            select(
                &mut r,
                TimelineCapture {
                    atomics: true,
                    ..Default::default()
                },
            );
            let (m, rows) = run(&f, r);
            assert_eq!(m.verdict, Some(ComparisonVerdict::Match));
            assert_eq!(
                transactions(&rows),
                vec![(
                    0x1000,
                    MemoryTransaction::ReadModifyWrite {
                        address: 0x3000,
                        order: ExecutionOrdering {
                            acquire: bits & 2 != 0,
                            release: bits & 1 != 0
                        },
                        old,
                        value: expected
                    }
                )]
            );
        }
    }
    let f = Fixture::new(&[
        op(2, 2, 5, 10, 0),
        op(3, 1, 6, 10, 11),
        op(3, 0, 7, 10, 11),
        0x00008067,
    ]);
    let mut r = scenario(&f, Some(7));
    select(
        &mut r,
        TimelineCapture {
            atomics: true,
            ..Default::default()
        },
    );
    let (m, rows) = run(&f, r);
    assert_eq!(m.verdict, Some(ComparisonVerdict::Match));
    let tx = transactions(&rows);
    assert!(matches!(
        tx[0].1,
        MemoryTransaction::LoadReserved { value: 7, .. }
    ));
    assert!(matches!(
        tx[1].1,
        MemoryTransaction::StoreConditional {
            stored: true,
            value: 5,
            ..
        }
    ));
    assert!(matches!(
        tx[2].1,
        MemoryTransaction::StoreConditional {
            stored: false,
            value: 5,
            ..
        }
    ));
}
#[test]
fn declared_model_and_service_memory_effects_enter_the_timeline_once() {
    let f = Fixture::from_inputs(vec![
        elf(&[0x00008413, 0x000022b7, 0x000280e7, 0x00040067]),
        elf(&[0x00b52023, 0x00000513, 0x00008067]),
    ]);
    let mut r = ram_request(&f);
    r.replacement.as_mut().unwrap().executables = vec![f.id(1)];
    r.cases[0].vendor.calls = vec![CallDeclaration {
        repetition: blobray_domain::CallRepetition::Finite,
        id: "output".into(),
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
            allocation: None,
            delay_micros: None,
            outputs: vec![CallOutput {
                pointer_argument: 0,
                byte_offset: 0,
                width: 4,
                value: 7,
                scope: CallOutputScope::NormalMemory,
            }],
        }],
    }];
    select(
        &mut r,
        TimelineCapture {
            writes: true,
            ..Default::default()
        },
    );
    let (m, rows) = run(&f, r.clone());
    assert_eq!(m.verdict, Some(ComparisonVerdict::Match));
    let effects: Vec<_> = rows
        .iter()
        .filter_map(|r| match r {
            ExecutionEvidence::Event { event, .. } => event.normal_memory(),
            _ => None,
        })
        .collect();
    assert_eq!(
        effects,
        vec![
            MemoryTransaction::Write {
                address: 0x3000,
                width: 4,
                value: 7
            };
            2
        ]
    );
    assert!(rows.iter().any(|r| matches!(
        r,
        ExecutionEvidence::Event {
            replacement: false,
            event: ExecutionEvent::CallOutput { .. },
            ..
        }
    )));
}
#[test]
fn timeline_capture_is_phase_owned_and_uncaptured_comparison_is_rejected() {
    let f = Fixture::new(&[0x00052283, 0x00000513, 0x00008067]);
    let mut r = ram_request(&f);
    select(
        &mut r,
        TimelineCapture {
            reads: true,
            ..Default::default()
        },
    );
    let mut warm = r.cases[0].clone();
    warm.name = "without-capture".into();
    warm.reset = SessionReset::Warm;
    warm.vendor.memory.clear();
    warm.replacement.as_mut().unwrap().memory.clear();
    warm.vendor.observe_timeline = TimelineCapture::default();
    warm.replacement.as_mut().unwrap().observe_timeline = TimelineCapture::default();
    warm.relation = Some(fixture_relation(true));
    r.cases.push(warm);
    let (m, rows) = run(&f, r.clone());
    assert_eq!(m.verdict, Some(ComparisonVerdict::Match));
    assert!(!rows.iter().any(|r| matches!(
        r,
        ExecutionEvidence::Event {
            case: 1,
            event: ExecutionEvent::Memory { .. },
            ..
        }
    )));
    r.cases[1].relation.as_mut().unwrap().events.timeline.reads = true;
    assert!(matches!(
        f.run(r, budget()),
        Err(Error {
            code: ErrorCode::InvalidRequest,
            ..
        })
    ));
}
#[test]
fn branch_loops_obey_event_capacity() {
    let f = Fixture::new(&[0xfff50513, 0xfe051ee3, 0x00008067]); // decrement; bnez -4; ret
    let mut r = f.request();
    r.cases[0].vendor.arguments[0] = Some(3);
    r.cases[0].replacement = Some(r.cases[0].vendor.clone());
    select(
        &mut r,
        TimelineCapture {
            branches: true,
            ..Default::default()
        },
    );
    let (m, rows) = run(&f, r.clone());
    assert_eq!(m.verdict, Some(ComparisonVerdict::Match));
    let branches: Vec<_> = rows
        .iter()
        .filter_map(|r| match r {
            ExecutionEvidence::Event {
                replacement: false,
                event: ExecutionEvent::Branch { taken, .. },
                ..
            } => Some(*taken),
            _ => None,
        })
        .collect();
    assert_eq!(branches, vec![true, true, false]);
    r.max_events = 2;
    let failed = f.run(r, budget());
    assert_eq!(failed.unwrap_err().code, ErrorCode::ResourceLimited);
}

#[test]
fn modeled_allocation_compares_the_initialized_prefix_once_not_backing_capacity() {
    let f = Fixture::new(&[0x00008413, 0x000022b7, 0x000280e7, 0x00040067]);
    for requested in [0, 8] {
        let mut r = f.request();
        r.cases[0].vendor.arguments = vec![Some(requested)];
        r.cases[0].vendor.calls = vec![CallDeclaration {
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
                return_words: [Some(0x4000), None],
                outputs: vec![],
                delay_micros: None,
                allocation: Some(CallAllocation {
                    address: 0x4000,
                    size_argument: 0,
                    capacity: 16,
                    lifetime: RegionLifetime::Session,
                }),
            }],
        }];
        r.cases[0].replacement = Some(r.cases[0].vendor.clone());
        r.cases[0].replacement.as_mut().unwrap().calls[0].responses[0]
            .allocation
            .as_mut()
            .unwrap()
            .capacity = 32;
        select(
            &mut r,
            TimelineCapture {
                writes: true,
                ..Default::default()
            },
        );
        let (m, rows) = run(&f, r.clone());
        assert_eq!(m.verdict, Some(ComparisonVerdict::Match));
        let effects: Vec<_> = rows
            .iter()
            .filter_map(|row| match row {
                ExecutionEvidence::Event { event, .. } => event.normal_memory(),
                _ => None,
            })
            .collect();
        assert_eq!(
            effects,
            if requested == 0 {
                vec![]
            } else {
                vec![
                    MemoryTransaction::InitializeZeroed {
                        address: 0x4000,
                        length: requested
                    };
                    2
                ]
            }
        );
        assert_eq!(
            rows.iter()
                .filter(|row| matches!(
                    row,
                    ExecutionEvidence::Event {
                        event: ExecutionEvent::Allocation { .. },
                        ..
                    }
                ))
                .count(),
            2
        );
        r.cases[0].replacement.as_mut().unwrap().arguments[0] = Some(requested + 4);
        assert_eq!(run(&f, r.clone()).0.verdict, Some(ComparisonVerdict::Diff));
        let relation = r.cases[0].relation.as_mut().unwrap();
        relation.events.timeline.writes = false;
        relation.returns.low = true;
        let (m, rows) = run(&f, r);
        assert_eq!(m.verdict, Some(ComparisonVerdict::Match));
        assert_eq!(
            rows.iter()
                .filter(|row| matches!(
                    row,
                    ExecutionEvidence::Event {
                        event: ExecutionEvent::Allocation { .. },
                        ..
                    }
                ))
                .count(),
            2
        );
    }
}
