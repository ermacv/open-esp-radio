use super::*;
fn declaration(behavior: DeviceBehavior, lifetime: RegionLifetime) -> DeviceDeclaration {
    DeviceDeclaration {
        id: "fixture".into(),
        applicability: "explicit synthetic device assumption".into(),
        lifetime,
        behavior,
    }
}
fn request(f: &Fixture, behavior: DeviceBehavior) -> ExecutionRequest {
    let mut r = f.request();
    r.cases[0].vendor.arguments = vec![Some(0x3000), Some(5), Some(99)];
    r.cases[0].vendor.models = vec![declaration(behavior, RegionLifetime::Phase)];
    r.cases[0].replacement = Some(r.cases[0].vendor.clone());
    r
}
fn model(rows: &[ExecutionEvidence], phase: u32) -> &ModelObservation {
    rows.iter()
        .find_map(|r| match r {
            ExecutionEvidence::Model {
                case,
                replacement: false,
                observation,
            } if *case == phase => Some(observation),
            _ => None,
        })
        .unwrap()
}
fn stop(rows: &[ExecutionEvidence], phase: u32) -> &ExecutionStop {
    rows.iter()
        .find_map(|r| match r {
            ExecutionEvidence::Outcome {
                case,
                replacement: false,
                stop,
                ..
            } if *case == phase => Some(stop),
            _ => None,
        })
        .unwrap()
}
#[test]
fn all_standard_devices_have_independent_values_and_complete_participation() {
    let cases = [
        (
            vec![0x00b51023, 0x00055503, 0x00008067],
            DeviceBehavior::RegisterBank {
                cells: vec![RegisterCell {
                    address: 0x3000,
                    width: 2,
                    value: 0,
                }],
            },
            0xffffabcd,
            0xabcd,
            1,
            1,
        ),
        (
            vec![0x00052503, 0x00008067],
            DeviceBehavior::ConstantRead {
                address: 0x3000,
                width: 4,
                value: 77,
            },
            5,
            77,
            1,
            0,
        ),
        (
            vec![0x00052283, 0x00052503, 0x00550533, 0x00008067],
            DeviceBehavior::SequenceRead {
                address: 0x3000,
                width: 4,
                runs: vec![7, 9].into_iter().map(ReadRun::once).collect(),
            },
            5,
            16,
            2,
            0,
        ),
        (
            vec![0x00b52023, 0x00052283, 0x00052503, 0x00008067],
            DeviceBehavior::W1c {
                address: 0x3000,
                width: 4,
                initial: 15,
                clear_mask: 3,
                read_clear_mask: 4,
            },
            1,
            10,
            2,
            1,
        ),
        (
            vec![0x00052283, 0x00052503, 0x00008067],
            DeviceBehavior::ReadClear {
                address: 0x3000,
                width: 4,
                initial: 15,
                clear_mask: 3,
            },
            5,
            12,
            2,
            0,
        ),
        (
            vec![0x00b52023, 0x00052503, 0x00008067],
            DeviceBehavior::SelfClearing {
                address: 0x3000,
                width: 4,
                initial: 128,
                store_mask: 3,
                command_mask: 4,
            },
            5,
            129,
            1,
            1,
        ),
        (
            vec![
                0x00b52023, 0x00052283, 0x00900593, 0x00b52023, 0x00052503, 0x00008067,
            ],
            DeviceBehavior::Fifo {
                address: 0x3000,
                width: 4,
                reads: vec![7, 8],
                writes: vec![5, 9],
            },
            5,
            8,
            2,
            2,
        ),
        (
            vec![0x00b52023, 0x00c52223, 0x00452503, 0x00008067],
            DeviceBehavior::IndexedBank {
                index_address: 0x3000,
                data_address: 0x3004,
                width: 4,
                index: None,
                values: vec![10, 20],
            },
            1,
            99,
            1,
            2,
        ),
    ];
    for (code, behavior, arg, expected, reads, writes) in cases {
        let f = Fixture::new(&code);
        let mut r = request(&f, behavior);
        r.cases[0].vendor.arguments[1] = Some(arg);
        r.cases[0].replacement = Some(r.cases[0].vendor.clone());
        let identity = r.cases[0].vendor.models[0]
            .identity(&mut || Ok(()))
            .unwrap();
        let (manifest, rows) = run(&f, r);
        assert!(manifest.complete);
        assert_eq!(manifest.verdict, Some(ComparisonVerdict::Match));
        assert!(
            matches!(stop(&rows, 0), ExecutionStop::Returned { low: Some(value), .. } if *value == expected)
        );
        let observed = model(&rows, 0);
        assert_eq!(observed.definition, identity);
        assert_eq!((observed.reads, observed.writes), (reads, writes));
        assert_eq!(
            (observed.remaining_reads, observed.remaining_writes),
            (0, 0)
        );
        assert!(observed.closed);
        assert_eq!(observed.status, ModelStatus::Complete);
    }
}
#[test]
fn cyclic_read_repeats_its_values_without_a_read_obligation() {
    // lw t0,0(a0); lw t1,0(a0); lw t2,0(a0); add a0,t0,t2; add a0,a0,t1; ret
    let f = Fixture::new(&[
        0x00052283, 0x00052303, 0x00052383, 0x00728533, 0x00650533, 0x00008067,
    ]);
    let (manifest, rows) = run(
        &f,
        request(
            &f,
            DeviceBehavior::CyclicRead {
                address: 0x3000,
                width: 4,
                values: vec![7, 100],
            },
        ),
    );
    assert!(manifest.complete);
    // Reads 7, 100, 7.
    assert!(matches!(
        stop(&rows, 0),
        ExecutionStop::Returned { low: Some(114), .. }
    ));
    let observed = model(&rows, 0);
    assert_eq!((observed.reads, observed.remaining_reads), (3, 0));
    assert_eq!(observed.status, ModelStatus::Complete);
    assert!(
        declaration(
            DeviceBehavior::CyclicRead {
                address: 0x3000,
                width: 4,
                values: vec![],
            },
            RegionLifetime::Phase
        )
        .validate()
        .is_err()
    );
}
#[test]
fn retained_aperture_merges_subword_writes_and_yields_to_exact_ports() {
    // sb a1,1(a0); lw t0,0(a0); lw t1,8(a0); add a0,t0,t1; ret
    let f = Fixture::new(&[0x00b500a3, 0x00052283, 0x00852303, 0x00628533, 0x00008067]);
    let aperture = DeviceBehavior::RetainedAperture {
        start: 0x3000,
        length: 0x100,
        initial: 0x1122_3344,
    };
    let mut r = request(&f, aperture.clone());
    r.cases[0].vendor.arguments[1] = Some(0xab);
    r.cases[0].replacement = Some(r.cases[0].vendor.clone());
    let (manifest, rows) = run(&f, r.clone());
    assert!(manifest.complete);
    // Byte 1 of the first word is written; the untouched word keeps `initial`.
    let expected = 0x1122_ab44u32.wrapping_add(0x1122_3344);
    assert!(matches!(
        stop(&rows, 0),
        ExecutionStop::Returned { low: Some(value), .. } if *value == expected
    ));
    let observed = model(&rows, 0);
    assert_eq!((observed.reads, observed.writes), (2, 1));
    assert_eq!(observed.status, ModelStatus::Complete);
    // Every aperture access is an ordinary MMIO event with its value.
    let reads: Vec<_> = rows
        .iter()
        .filter_map(|r| match r {
            ExecutionEvidence::Event {
                replacement: false,
                event: ExecutionEvent::Read { address, value, .. },
                ..
            } => Some((*address, *value)),
            _ => None,
        })
        .collect();
    assert_eq!(reads, [(0x3000, 0x1122_ab44), (0x3008, 0x1122_3344)]);
    // An exact port inside the aperture takes precedence.
    let mut with_port = r.clone();
    with_port.cases[0].vendor.models.push(DeviceDeclaration {
        id: "status".into(),
        ..declaration(
            DeviceBehavior::ConstantRead {
                address: 0x3008,
                width: 4,
                value: 77,
            },
            RegionLifetime::Phase,
        )
    });
    with_port.cases[0].replacement = Some(with_port.cases[0].vendor.clone());
    let (manifest, rows) = run(&f, with_port);
    assert!(manifest.complete);
    assert!(matches!(
        stop(&rows, 0),
        ExecutionStop::Returned { low: Some(value), .. } if *value == 0x1122_ab44 + 77
    ));
    // Misaligned access is a model gap, never a decomposed device access.
    let f = Fixture::new(&[0x00252283, 0x00008067]);
    let (manifest, rows) = run(&f, request(&f, aperture.clone()));
    assert!(!manifest.complete);
    assert!(matches!(stop(&rows, 0), ExecutionStop::Incomplete { .. }));
    // Invalid geometry is rejected before execution.
    for bad in [
        DeviceBehavior::RetainedAperture {
            start: 0x3002,
            length: 0x100,
            initial: 0,
        },
        DeviceBehavior::RetainedAperture {
            start: 0x3000,
            length: 0,
            initial: 0,
        },
    ] {
        assert!(declaration(bad, RegionLifetime::Phase).validate().is_err());
    }
}
#[test]
fn unconsumed_obligations_cannot_match_a_successful_return() {
    for behavior in [
        DeviceBehavior::SequenceRead {
            address: 0x3000,
            width: 4,
            runs: vec![7, 9].into_iter().map(ReadRun::once).collect(),
        },
        DeviceBehavior::Fifo {
            address: 0x3000,
            width: 4,
            reads: vec![7],
            writes: vec![5],
        },
    ] {
        let f = Fixture::new(&[0x00052503, 0x00008067]);
        let (manifest, rows) = run(&f, request(&f, behavior));
        assert!(matches!(
            stop(&rows, 0),
            ExecutionStop::Returned { low: Some(7), .. }
        ));
        assert!(!manifest.complete);
        assert_eq!(manifest.verdict, Some(ComparisonVerdict::Incomplete));
        assert_eq!(model(&rows, 0).status, ModelStatus::Incomplete);
        assert!(model(&rows, 0).remaining_reads + model(&rows, 0).remaining_writes > 0);
    }
}
#[test]
fn mismatched_and_exhausted_accesses_preserve_explicit_model_issues() {
    for (code, behavior, issue) in [
        (
            vec![0x00052283, 0x00052503, 0x00008067],
            DeviceBehavior::SequenceRead {
                address: 0x3000,
                width: 4,
                runs: vec![7].into_iter().map(ReadRun::once).collect(),
            },
            DeviceIssue::ExhaustedReads,
        ),
        (
            vec![0x00b52023, 0x00008067],
            DeviceBehavior::ConstantRead {
                address: 0x3000,
                width: 4,
                value: 7,
            },
            DeviceIssue::ReadOnly,
        ),
        (
            vec![0x00b52023, 0x00008067],
            DeviceBehavior::ReadClear {
                address: 0x3000,
                width: 4,
                initial: 7,
                clear_mask: 3,
            },
            DeviceIssue::ReadOnly,
        ),
        (
            vec![0x00b52023, 0x00008067],
            DeviceBehavior::Fifo {
                address: 0x3000,
                width: 4,
                reads: vec![],
                writes: vec![6],
            },
            DeviceIssue::WriteMismatch {
                expected: 6,
                actual: 5,
            },
        ),
        (
            vec![0x00b52023, 0x00008067],
            DeviceBehavior::Fifo {
                address: 0x3000,
                width: 4,
                reads: vec![],
                writes: vec![],
            },
            DeviceIssue::UnexpectedWrite,
        ),
        (
            vec![0x00452503, 0x00008067],
            DeviceBehavior::IndexedBank {
                index_address: 0x3000,
                data_address: 0x3004,
                width: 4,
                index: None,
                values: vec![7],
            },
            DeviceIssue::UnknownIndex,
        ),
        (
            vec![0x00b52023, 0x00008067],
            DeviceBehavior::IndexedBank {
                index_address: 0x3000,
                data_address: 0x3004,
                width: 4,
                index: Some(0),
                values: vec![7],
            },
            DeviceIssue::IndexOutOfRange { index: 5 },
        ),
        (
            vec![0x00054503, 0x00008067],
            DeviceBehavior::W1c {
                address: 0x3000,
                width: 4,
                initial: 7,
                clear_mask: 3,
                read_clear_mask: 0,
            },
            DeviceIssue::AccessWidth,
        ),
    ] {
        let f = Fixture::new(&code);
        let (manifest, rows) = run(&f, request(&f, behavior));
        assert_eq!(manifest.verdict, Some(ComparisonVerdict::Incomplete));
        assert!(matches!(
            stop(&rows, 0),
            ExecutionStop::Incomplete {
                reason: ExecutionGap::Memory { .. },
                ..
            }
        ));
        assert_eq!(model(&rows, 0).issue, Some(issue));
        assert_eq!(model(&rows, 0).status, ModelStatus::Incomplete);
    }
}
#[test]
fn session_sequences_stay_open_until_chain_closure_and_phase_sequences_block() {
    for lifetime in [RegionLifetime::Session, RegionLifetime::Phase] {
        let f = Fixture::new(&[0x00052503, 0x00008067]);
        let mut r = request(
            &f,
            DeviceBehavior::SequenceRead {
                address: 0x3000,
                width: 4,
                runs: vec![7, 9].into_iter().map(ReadRun::once).collect(),
            },
        );
        r.cases[0].vendor.models[0].lifetime = lifetime;
        r.cases[0].replacement = Some(r.cases[0].vendor.clone());
        let mut next = r.cases[0].clone();
        next.name = "continue".into();
        next.reset = SessionReset::Warm;
        next.vendor.models.clear();
        next.replacement = Some(next.vendor.clone());
        r.cases.push(next);
        let (manifest, rows) = run(&f, r);
        if lifetime == RegionLifetime::Session {
            assert!(manifest.complete);
            assert_eq!(manifest.verdict, Some(ComparisonVerdict::Match));
            assert_eq!(model(&rows, 0).status, ModelStatus::Open);
            assert!(!model(&rows, 0).closed);
            assert_eq!(model(&rows, 1).status, ModelStatus::Complete);
            assert_eq!(model(&rows, 1).reads, 2);
            assert!(matches!(
                stop(&rows, 1),
                ExecutionStop::Returned { low: Some(9), .. }
            ));
        } else {
            assert!(!manifest.complete);
            assert_eq!(manifest.verdict, Some(ComparisonVerdict::Incomplete));
            assert_eq!(model(&rows, 0).status, ModelStatus::Incomplete);
            assert!(matches!(stop(&rows, 1), ExecutionStop::BlockedByPriorPhase));
        }
    }
}
#[test]
fn indexed_ports_do_not_claim_the_gap_between_them() {
    let f = Fixture::new(&[0x00452503, 0x00008067]);
    let mut r = request(
        &f,
        DeviceBehavior::IndexedBank {
            index_address: 0x3000,
            data_address: 0x3010,
            width: 4,
            index: None,
            values: vec![7],
        },
    );
    r.cases[0].vendor.memory.push(ram(MemorySeed {
        address: 0x3004,
        length: 4,
        fill: None,
        bytes: 123u32.to_le_bytes().to_vec(),
    }));
    r.cases[0].replacement = Some(r.cases[0].vendor.clone());
    let (manifest, rows) = run(&f, r);
    assert_eq!(manifest.verdict, Some(ComparisonVerdict::Match));
    assert!(matches!(
        stop(&rows, 0),
        ExecutionStop::Returned { low: Some(123), .. }
    ));
    assert_eq!((model(&rows, 0).reads, model(&rows, 0).writes), (0, 0));
    assert!(
        !rows
            .iter()
            .any(|r| matches!(r, ExecutionEvidence::Event { .. }))
    );
}

#[test]
fn session_model_evidence_distinguishes_a_changed_read_run() {
    let f = Fixture::new(&[0x00052503, 0x00008067]);
    let mut r = request(
        &f,
        DeviceBehavior::SequenceRead {
            address: 0x3000,
            width: 4,
            runs: vec![7, 9].into_iter().map(ReadRun::once).collect(),
        },
    );
    r.cases[0].vendor.models[0].lifetime = RegionLifetime::Session;
    r.cases[0].replacement = Some(r.cases[0].vendor.clone());
    let mut second = r.cases[0].clone();
    second.name = "finish-sequence".into();
    second.reset = SessionReset::Warm;
    second.vendor.models.clear();
    second.replacement = Some(second.vendor.clone());
    r.cases.push(second);
    if let DeviceBehavior::SequenceRead { runs, .. } =
        &mut r.cases[0].replacement.as_mut().unwrap().models[0].behavior
    {
        runs[1].value = 10;
    }
    let (manifest, _) = run(&f, r);
    assert!(manifest.complete);
    assert_eq!(manifest.verdict, Some(ComparisonVerdict::Diff));
}

#[test]
fn finite_read_runs_cross_warm_boundaries_and_cold_restarts_without_expanding() {
    let f = Fixture::new(&[0x00052503, 0x00008067]);
    let mut r = request(
        &f,
        DeviceBehavior::SequenceRead {
            address: 0x3000,
            width: 4,
            runs: vec![ReadRun { value: 7, count: 2 }, ReadRun::once(9)],
        },
    );
    r.cases[0].vendor.models[0].lifetime = RegionLifetime::Session;
    r.cases[0].replacement = Some(r.cases[0].vendor.clone());
    for i in 1..3 {
        let mut next = r.cases[0].clone();
        next.name = format!("warm-{i}");
        next.reset = SessionReset::Warm;
        next.vendor.models.clear();
        next.replacement = Some(next.vendor.clone());
        r.cases.push(next);
    }
    let mut cold = r.cases[0].clone();
    cold.name = "cold-with-unconsumed-repeat".into();
    cold.vendor.models[0].behavior = DeviceBehavior::SequenceRead {
        address: 0x3000,
        width: 4,
        runs: vec![ReadRun {
            value: 42,
            count: 20_000,
        }],
    };
    cold.replacement = Some(cold.vendor.clone());
    r.cases.push(cold);
    let (manifest, rows) = run(&f, r);
    assert_eq!(manifest.verdict, Some(ComparisonVerdict::Incomplete));
    for (i, (value, remaining)) in [(7, 2), (7, 1), (9, 0), (42, 19_999)]
        .into_iter()
        .enumerate()
    {
        assert!(
            matches!(stop(&rows, i as u32), ExecutionStop::Returned { low: Some(v), .. } if *v == value)
        );
        assert_eq!(model(&rows, i as u32).remaining_reads, remaining);
    }
    assert_eq!(model(&rows, 2).status, ModelStatus::Complete);
    assert_eq!(model(&rows, 3).status, ModelStatus::Incomplete);
}
#[test]
fn cold_closure_is_explicit_and_expired_ports_can_become_ram() {
    let f = Fixture::new(&[0x00052503, 0x00008067]);
    let mut r = request(
        &f,
        DeviceBehavior::SequenceRead {
            address: 0x3000,
            width: 4,
            runs: vec![7].into_iter().map(ReadRun::once).collect(),
        },
    );
    r.cases[0].vendor.models[0].lifetime = RegionLifetime::Session;
    r.cases[0].replacement = Some(r.cases[0].vendor.clone());
    let mut second = r.cases[0].clone();
    second.name = "new-cold-device".into();
    second.vendor.models[0].behavior = DeviceBehavior::ConstantRead {
        address: 0x3000,
        width: 4,
        value: 42,
    };
    second.replacement = Some(second.vendor.clone());
    r.cases.push(second);
    let (manifest, rows) = run(&f, r);
    assert_eq!(manifest.verdict, Some(ComparisonVerdict::Match));
    assert!(model(&rows, 0).closed && model(&rows, 1).closed);
    assert_ne!(model(&rows, 0).definition, model(&rows, 1).definition);
    assert!(matches!(
        stop(&rows, 1),
        ExecutionStop::Returned { low: Some(42), .. }
    ));
    let mut r = request(
        &f,
        DeviceBehavior::ConstantRead {
            address: 0x3000,
            width: 4,
            value: 7,
        },
    );
    let mut second = r.cases[0].clone();
    second.reset = SessionReset::Warm;
    second.vendor.models.clear();
    second.vendor.memory.push(ram(MemorySeed {
        address: 0x3000,
        length: 4,
        fill: None,
        bytes: 42u32.to_le_bytes().to_vec(),
    }));
    second.replacement = Some(second.vendor.clone());
    r.cases.push(second);
    let (manifest, rows) = run(&f, r);
    assert_eq!(manifest.verdict, Some(ComparisonVerdict::Match));
    assert!(matches!(
        stop(&rows, 1),
        ExecutionStop::Returned { low: Some(42), .. }
    ));
}
#[test]
fn invalid_declarations_and_live_ownership_conflicts_are_rejected() {
    let f = Fixture::new(&[0x00008067]);
    let behavior = DeviceBehavior::ConstantRead {
        address: 0x3000,
        width: 4,
        value: 7,
    };
    for bad in [
        DeviceBehavior::ConstantRead {
            address: 0x3000,
            width: 3,
            value: 7,
        },
        DeviceBehavior::ConstantRead {
            address: 0x3000,
            width: 1,
            value: 256,
        },
        DeviceBehavior::ConstantRead {
            address: u32::MAX,
            width: 1,
            value: 7,
        },
        DeviceBehavior::SequenceRead {
            address: 0x3000,
            width: 4,
            runs: vec![].into_iter().map(ReadRun::once).collect(),
        },
        DeviceBehavior::SelfClearing {
            address: 0x3000,
            width: 4,
            initial: 0,
            store_mask: 3,
            command_mask: 1,
        },
        DeviceBehavior::IndexedBank {
            index_address: 0x3000,
            data_address: 0x3004,
            width: 1,
            index: None,
            values: vec![0; 257],
        },
    ] {
        match f.run(request(&f, bad), budget()) {
            Ok(_) => panic!("invalid model admitted"),
            Err(error) => assert_eq!(error.code, ErrorCode::InvalidRequest),
        }
    }
    for which in 0..4 {
        let mut r = request(&f, behavior.clone());
        match which {
            0 => {
                let mut other = r.cases[0].vendor.models[0].clone();
                other.id = "other".into();
                r.cases[0].vendor.models.push(other);
            }
            1 => {
                r.cases[0].vendor.models[0].behavior = DeviceBehavior::ConstantRead {
                    address: 0x8000,
                    width: 4,
                    value: 7,
                }
            }
            2 | 3 => {
                r.cases[0].vendor.models[0].lifetime = RegionLifetime::Session;
                r.cases[0].replacement = Some(r.cases[0].vendor.clone());
                let mut second = r.cases[0].clone();
                second.reset = SessionReset::Warm;
                if which == 3 {
                    second.vendor.models.clear();
                    second.vendor.memory.push(ram(MemorySeed {
                        address: 0x3000,
                        length: 4,
                        fill: Some(0),
                        bytes: vec![],
                    }));
                }
                second.replacement = Some(second.vendor.clone());
                r.cases.push(second);
            }
            _ => unreachable!(),
        }
        let failed = f.run(r, budget());
        assert_eq!(failed.unwrap_err().code, ErrorCode::Conflict);
    }
}
