use super::*;
const PREFIX: [u32; 3] = [0x00008413, 0x000022b7, 0x000280e7]; // save ra in s0; absolute call 0x2000
fn code(tail: &[u32]) -> Vec<u32> {
    PREFIX
        .into_iter()
        .chain(tail.iter().copied())
        .chain([0x00040067])
        .collect()
}
fn response(low: Option<u32>) -> CallResponse {
    CallResponse {
        return_words: [low, None],
        outputs: vec![],
        allocation: None,
        delay_micros: None,
    }
}
fn request(f: &Fixture, responses: Vec<CallResponse>) -> ExecutionRequest {
    let mut r = f.request();
    r.max_events = 128;
    r.cases[0].vendor.calls = vec![CallDeclaration {
        repetition: blobray_domain::CallRepetition::Finite,
        id: "external".into(),
        applicability: "synthetic external ABI assumption".into(),
        lifetime: RegionLifetime::Phase,
        binding: CallBinding {
            address: 0x2000,
            boundary: CallBoundary::Unmapped,
            allow_tail: false,
        },
        argument_words: 8,
        responses,
    }];
    r.cases[0].replacement = Some(r.cases[0].vendor.clone());
    r
}
fn stop(rows: &[ExecutionEvidence], case: u32) -> &ExecutionStop {
    rows.iter()
        .find_map(|r| match r {
            ExecutionEvidence::Outcome {
                case: i,
                replacement: false,
                stop,
                ..
            } if *i == case => Some(stop),
            _ => None,
        })
        .unwrap()
}
fn model(rows: &[ExecutionEvidence], case: u32) -> &CallObservation {
    rows.iter()
        .find_map(|r| match r {
            ExecutionEvidence::CallModel {
                case: i,
                replacement: false,
                observation,
            } if *i == case => Some(observation),
            _ => None,
        })
        .unwrap()
}
#[test]
fn explicit_return_words_clobbers_and_captured_boundaries_are_distinct() {
    let f = Fixture::new(&code(&[0x00b50533])); // low+high
    let mut r = request(&f, vec![response(Some(7))]);
    r.cases[0].vendor.calls[0].responses[0].return_words[1] = Some(9);
    r.cases[0].replacement = Some(r.cases[0].vendor.clone());
    let (m, rows) = run(&f, r);
    assert!(m.complete);
    assert_eq!(m.verdict, Some(ComparisonVerdict::Match));
    assert!(matches!(
        stop(&rows, 0),
        ExecutionStop::Returned { low: Some(16), .. }
    ));
    assert_eq!(model(&rows, 0).calls, 1);
    let f = Fixture::new(&code(&[0x00060513])); // consume caller-saved a2 after model
    let (m, rows) = run(&f, request(&f, vec![response(Some(7))]));
    // The clobbered register stays unknown through the move into the return.
    assert_eq!(m.verdict, Some(ComparisonVerdict::Incomplete));
    assert!(matches!(
        stop(&rows, 0),
        ExecutionStop::Returned { low: None, .. }
    ));
    assert_eq!(model(&rows, 0).status, ModelStatus::Complete);
    let f = Fixture::new(&[0x00008413, 0x00c000ef, 0x00040067, 0x00000013, 0x00000073]);
    let mut r = request(&f, vec![response(Some(42))]);
    r.cases[0].vendor.calls[0].binding = CallBinding {
        address: 0x1010,
        boundary: CallBoundary::CapturedCode,
        allow_tail: false,
    };
    r.cases[0].replacement = Some(r.cases[0].vendor.clone());
    let (m, rows) = run(&f, r.clone());
    assert!(m.complete);
    assert!(matches!(
        stop(&rows, 0),
        ExecutionStop::Returned { low: Some(42), .. }
    ));
    r.cases[0].vendor.calls.clear();
    r.cases[0].replacement = Some(r.cases[0].vendor.clone());
    let (m, rows) = run(&f, r);
    assert!(!m.complete);
    assert!(matches!(
        stop(&rows, 0),
        ExecutionStop::Incomplete {
            reason: ExecutionGap::UnsupportedInstruction,
            ..
        }
    ));
}
#[test]
fn output_owners_and_stack_abi_words_are_checked_before_writes() {
    for (address, scope) in [
        (0x3000, CallOutputScope::NormalMemory),
        (0x8ff0, CallOutputScope::PrivateStack),
    ] {
        let f = Fixture::new(&code(&[0x00052503])); // read through explicit returned pointer
        let mut reply = response(Some(address));
        reply.outputs.push(CallOutput {
            pointer_argument: 8,
            byte_offset: 0,
            width: 4,
            value: 47,
            scope,
        });
        reply.delay_micros = Some(CallValue::Argument { word: 1 });
        let mut r = request(&f, vec![reply]);
        r.cases[0].vendor.arguments = vec![
            Some(0),
            Some(13),
            None,
            None,
            None,
            None,
            None,
            None,
            Some(address),
        ];
        r.cases[0].vendor.calls[0].argument_words = 9;
        if scope == CallOutputScope::NormalMemory {
            r.cases[0].vendor.memory = vec![ExecutionRegion {
                lifetime: RegionLifetime::Phase,
                seed: MemorySeed {
                    address,
                    length: 4,
                    fill: None,
                    bytes: vec![],
                },
            }];
        }
        r.cases[0].replacement = Some(r.cases[0].vendor.clone());
        let (m, rows) = run(&f, r.clone());
        assert!(m.complete);
        assert!(matches!(
            stop(&rows, 0),
            ExecutionStop::Returned { low: Some(47), .. }
        ));
        assert!(rows.iter().any(|r|matches!(r,ExecutionEvidence::Event{event:ExecutionEvent::CallArgument{word:8,value:Some(v)},..} if *v==address)));
        assert!(rows.iter().any(|r| matches!(
            r,
            ExecutionEvidence::Event {
                event: ExecutionEvent::DelayMicros { value: 13 },
                ..
            }
        )));
        r.cases[0].vendor.arguments[8] = None;
        r.cases[0].replacement = Some(r.cases[0].vendor.clone());
        let (m, rows) = run(&f, r);
        assert!(!m.complete);
        assert_eq!(
            model(&rows, 0).issue,
            Some(CallIssue::UnknownArgument { word: 8 })
        );
        assert!(!rows.iter().any(|r| matches!(
            r,
            ExecutionEvidence::Event {
                event: ExecutionEvent::CallOutput { .. },
                ..
            }
        )));
    }
}
#[test]
fn bounded_allocations_zero_only_accessible_prefix_and_follow_owned_lifetime() {
    let f = Fixture::new(&code(&[0x00052503]));
    let mut reply = response(Some(0x4000));
    reply.allocation = Some(CallAllocation {
        address: 0x4000,
        size_argument: 0,
        capacity: 16,
        lifetime: RegionLifetime::Session,
    });
    let mut r = request(&f, vec![reply]);
    r.cases[0].vendor.arguments[0] = Some(8);
    r.cases[0].replacement = Some(r.cases[0].vendor.clone());
    let (m, rows) = run(&f, r.clone());
    assert!(m.complete);
    assert!(matches!(
        stop(&rows, 0),
        ExecutionStop::Returned { low: Some(0), .. }
    ));
    let mut second = r.cases[0].clone();
    second.name = "warm allocation read".into();
    second.reset = SessionReset::Warm;
    second.vendor.entry = 0x100c;
    // Enter lw then jump via s0 is unknown; use the root return goal at a separate plain ret fixture below.
    second.vendor.calls.clear();
    second.vendor.arguments[0] = Some(0x4008);
    second.replacement = Some(second.vendor.clone());
    r.cases.push(second);
    let (m, rows) = run(&f, r.clone());
    assert!(!m.complete);
    assert!(matches!(
        stop(&rows, 1),
        ExecutionStop::Incomplete {
            reason: ExecutionGap::Memory {
                address: 0x4008,
                access: MemoryAccess::Read
            },
            ..
        }
    ));
    r.cases.truncate(1);
    r.cases[0].vendor.arguments[0] = Some(17);
    r.cases[0].replacement = Some(r.cases[0].vendor.clone());
    let (m, rows) = run(&f, r);
    assert!(!m.complete);
    assert_eq!(
        model(&rows, 0).issue,
        Some(CallIssue::AllocationSize {
            requested: 17,
            capacity: 16
        })
    );
}
#[test]
fn unconsumed_and_exhausted_calls_cannot_match_successful_code() {
    let f = Fixture::new(&code(&[]));
    for (replies, issue) in [
        (vec![], Some(CallIssue::ExhaustedResponses)),
        (vec![response(Some(1)), response(Some(2))], None),
    ] {
        let (m, rows) = run(&f, request(&f, replies));
        assert!(!m.complete);
        assert_eq!(m.verdict, Some(ComparisonVerdict::Incomplete));
        assert_eq!(model(&rows, 0).issue, issue);
    }
    let mut differing = request(&f, vec![response(Some(7)), response(Some(9))]);
    differing.cases[0].replacement.as_mut().unwrap().calls[0].responses[0].return_words[0] =
        Some(8);
    let (manifest, _) = run(&f, differing);
    assert!(!manifest.complete);
    assert_eq!(manifest.verdict, Some(ComparisonVerdict::Diff));
    let mut r = request(&f, vec![response(Some(7)), response(Some(9))]);
    r.cases[0].vendor.calls[0].lifetime = RegionLifetime::Session;
    r.cases[0].replacement = Some(r.cases[0].vendor.clone());
    let mut second = r.cases[0].clone();
    second.name = "warm".into();
    second.reset = SessionReset::Warm;
    second.vendor.calls.clear();
    second.replacement = Some(second.vendor.clone());
    r.cases.push(second);
    let (m, rows) = run(&f, r);
    assert!(m.complete);
    assert_eq!(model(&rows, 0).status, ModelStatus::Open);
    assert_eq!(model(&rows, 1).status, ModelStatus::Complete);
    assert_eq!(model(&rows, 1).calls, 2);
}

#[test]
fn unbounded_pure_response_answers_every_call_and_completes() {
    // Three calls to 0x2000: the prefix call, then two more after reloading t0.
    let f = Fixture::new(&code(&[0x000022b7, 0x000280e7, 0x000022b7, 0x000280e7]));
    let mut r = request(&f, vec![response(Some(1))]);
    r.cases[0].vendor.calls[0].repetition = blobray_domain::CallRepetition::Unbounded;
    r.cases[0].replacement = Some(r.cases[0].vendor.clone());
    let (m, rows) = run(&f, r);
    assert!(m.complete);
    assert_eq!(m.verdict, Some(ComparisonVerdict::Match));
    let observed = model(&rows, 0);
    assert_eq!((observed.calls, observed.remaining), (3, 0));
    assert_eq!(observed.status, ModelStatus::Complete);
    // The same single response declared finitely is exhausted by the second call.
    let (m, rows) = run(&f, request(&f, vec![response(Some(1))]));
    assert!(!m.complete);
    assert_eq!(model(&rows, 0).issue, Some(CallIssue::ExhaustedResponses));
}

#[test]
fn model_outputs_never_fall_back_to_mmio_and_response_validation_is_atomic() {
    let f = Fixture::new(&code(&[]));
    for scope in [CallOutputScope::NormalMemory, CallOutputScope::PrivateStack] {
        let mut reply = response(Some(7));
        reply.outputs = vec![
            CallOutput {
                pointer_argument: 0,
                byte_offset: 0,
                width: 4,
                value: 42,
                scope: CallOutputScope::NormalMemory,
            },
            CallOutput {
                pointer_argument: 1,
                byte_offset: 0,
                width: 4,
                value: 99,
                scope,
            },
        ];
        let mut r = request(&f, vec![reply]);
        r.cases[0].vendor.arguments[0] = Some(0x4000);
        r.cases[0].vendor.arguments[1] = Some(0x3000);
        r.cases[0].vendor.memory = vec![ExecutionRegion {
            lifetime: RegionLifetime::Session,
            seed: MemorySeed {
                address: 0x4000,
                length: 4,
                fill: Some(0),
                bytes: vec![],
            },
        }];
        r.cases[0].vendor.models = vec![register_bank(vec![RegisterCell {
            address: 0x3000,
            width: 4,
            value: 1,
        }])];
        r.cases[0].replacement = Some(r.cases[0].vendor.clone());
        let (m, rows) = run(&f, r);
        assert!(!m.complete);
        assert_eq!(
            model(&rows, 0).issue,
            Some(CallIssue::OutputAccess {
                address: 0x3000,
                scope
            })
        );
        assert!(!rows.iter().any(|r| matches!(
            r,
            ExecutionEvidence::Event {
                event: ExecutionEvent::CallOutput { .. } | ExecutionEvent::Write { .. },
                ..
            }
        )));
    }
    let mut reply = response(Some(0x4000));
    reply.allocation = Some(CallAllocation {
        address: 0x4000,
        size_argument: 0,
        capacity: 16,
        lifetime: RegionLifetime::Session,
    });
    let mut r = request(&f, vec![reply]);
    r.cases[0].vendor.arguments[0] = Some(4);
    r.cases[0].vendor.memory = vec![ExecutionRegion {
        lifetime: RegionLifetime::Session,
        seed: MemorySeed {
            address: 0x4008,
            length: 4,
            fill: None,
            bytes: vec![],
        },
    }];
    r.cases[0].replacement = Some(r.cases[0].vendor.clone());
    let (_, rows) = run(&f, r);
    assert_eq!(
        model(&rows, 0).issue,
        Some(CallIssue::AllocationOverlap {
            address: 0x4000,
            capacity: 16
        })
    );
}
#[test]
fn tail_alternate_link_and_physical_goal_precede_model_dispatch() {
    let f = Fixture::new(&[0x00002337, 0x00030067]); // tail through t1, ra remains root sentinel
    let mut r = request(&f, vec![response(Some(7))]);
    let (_, rows) = run(&f, r.clone());
    assert_eq!(model(&rows, 0).issue, Some(CallIssue::TailNotAllowed));
    r.cases[0].vendor.calls[0].binding.allow_tail = true;
    r.cases[0].replacement = Some(r.cases[0].vendor.clone());
    let (m, rows) = run(&f, r);
    assert!(m.complete);
    assert!(matches!(
        stop(&rows, 0),
        ExecutionStop::Returned { low: Some(7), .. }
    ));
    let f = Fixture::new(&[0x00008413, 0x00002337, 0x000302e7, 0x00040067]); // alternate t0 link, target t1
    let (m, rows) = run(&f, request(&f, vec![response(Some(8))]));
    assert!(m.complete);
    assert!(matches!(
        stop(&rows, 0),
        ExecutionStop::Returned { low: Some(8), .. }
    ));
    let (f, point) = super::goals::fixture(
        &[0x00008413, 0x00c000ef, 0x00040067, 0x00000013, 0x00000073],
        0x1010,
    );
    let mut r = request(&f, vec![response(Some(42))]);
    for phase in &mut r.cases {
        phase.relation.as_mut().unwrap().returns.low = false;
    }
    r.cases[0].vendor.calls[0].binding = CallBinding {
        address: 0x1010,
        boundary: CallBoundary::CapturedCode,
        allow_tail: false,
    };
    r.cases[0].vendor.goal = ExecutionGoal::ObserveCall {
        target: point,
        include_tail: false,
    };
    r.cases[0].replacement = Some(r.cases[0].vendor.clone());
    let (m, rows) = run(&f, r);
    assert!(!m.complete);
    assert!(matches!(
        stop(&rows, 0),
        ExecutionStop::ObservedCall { target: 0x1010, .. }
    ));
    assert_eq!(model(&rows, 0).remaining, 1);
    assert!(!rows.iter().any(|r| matches!(
        r,
        ExecutionEvidence::Event {
            event: ExecutionEvent::ModeledCall { .. },
            ..
        }
    )));
}
#[test]
fn composed_allocation_device_delay_and_warm_calls() {
    let mut program = code(&[]);
    // Separate warm entry: read persistent allocation, then call model with that value.
    let entry = 0x1000 + program.len() as u32 * 4;
    program.extend([
        0x00008413, 0x00052503, 0x000022b7, 0x000280e7, 0x00052503, 0x00040067,
    ]);
    let f = Fixture::new(&program);
    let mut first = response(Some(0x4000));
    first.allocation = Some(CallAllocation {
        address: 0x4000,
        size_argument: 0,
        capacity: 16,
        lifetime: RegionLifetime::Session,
    });
    first.delay_micros = Some(CallValue::Constant { value: 5 });
    let mut second = response(Some(0x3000));
    second.delay_micros = Some(CallValue::Argument { word: 0 });
    let mut r = request(&f, vec![first, second]);
    r.cases[0].vendor.calls[0].lifetime = RegionLifetime::Session;
    r.cases[0].vendor.arguments[0] = Some(4);
    r.cases[0].vendor.models = vec![DeviceDeclaration {
        id: "ready".into(),
        applicability: "one warm read".into(),
        lifetime: RegionLifetime::Session,
        behavior: DeviceBehavior::SequenceRead {
            address: 0x3000,
            width: 4,
            runs: vec![77].into_iter().map(ReadRun::once).collect(),
        },
    }];
    r.cases[0].replacement = Some(r.cases[0].vendor.clone());
    let mut warm = r.cases[0].clone();
    warm.name = "warm-read".into();
    warm.reset = SessionReset::Warm;
    warm.vendor.entry = entry;
    warm.vendor.calls.clear();
    warm.vendor.models.clear();
    warm.vendor.arguments[0] = Some(0x4000);
    warm.replacement = Some(warm.vendor.clone());
    r.cases.push(warm);
    let (m, rows) = run(&f, r.clone());
    assert!(m.complete);
    assert_eq!(m.verdict, Some(ComparisonVerdict::Match));
    assert!(matches!(
        stop(&rows, 1),
        ExecutionStop::Returned { low: Some(77), .. }
    ));
    assert_eq!(model(&rows, 1).calls, 2);
    r.cases[0].replacement.as_mut().unwrap().calls[0].responses[0].delay_micros =
        Some(CallValue::Constant { value: 6 });
    assert_eq!(run(&f, r).0.verdict, Some(ComparisonVerdict::Diff));
}

#[test]
fn invalid_bindings_stack_access_and_resource_limits_fail() {
    let f = Fixture::new(&code(&[]));
    let base = request(&f, vec![response(Some(7))]);
    for variant in 0..5 {
        let mut r = base.clone();
        let d = &mut r.cases[0].vendor.calls[0];
        match variant {
            0 => d.binding.address = 0x2001,
            1 => d.argument_words = 257,
            2 => d.responses[0].outputs.push(CallOutput {
                pointer_argument: 8,
                byte_offset: 0,
                width: 4,
                value: 0,
                scope: CallOutputScope::NormalMemory,
            }),
            3 => {
                d.responses[0].allocation = Some(CallAllocation {
                    address: 0x4000,
                    size_argument: 0,
                    capacity: 4,
                    lifetime: RegionLifetime::Phase,
                })
            } // explicit return differs
            4 => d.responses[0].delay_micros = Some(CallValue::Argument { word: 8 }),
            _ => unreachable!(),
        }
        let error = f.run(r, budget()).unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidRequest);
    }
    for (address, boundary) in [
        (0x1000, CallBoundary::Unmapped),
        (0x2000, CallBoundary::CapturedCode),
    ] {
        let mut r = base.clone();
        r.cases[0].vendor.calls[0].binding = CallBinding {
            address,
            boundary,
            allow_tail: false,
        };
        let record = f.run(r, budget());
        assert_eq!(record.unwrap_err().code, ErrorCode::InvalidRequest);
    }
    let mut r = base.clone();
    r.cases[0].vendor.calls[0].argument_words = 9;
    r.cases[0].replacement = Some(r.cases[0].vendor.clone());
    let (_, rows) = run(&f, r);
    assert_eq!(
        model(&rows, 0).issue,
        Some(CallIssue::StackArgument { word: 8 })
    );
    let mut r = base.clone();
    r.max_events = 9;
    let record = f.run(r, budget());
    assert_eq!(record.unwrap_err().code, ErrorCode::ResourceLimited);
    let mut b = budget();
    b.working_memory_bytes = Some(1024 * 1024);
    let record = f.run(base.clone(), b);
    assert_eq!(record.unwrap_err().code, ErrorCode::ResourceLimited);
    let f = Fixture::new(&[0xffc10113, 0x000022b7, 0x000280e7, 0x00008067]);
    let (_, rows) = run(&f, request(&f, vec![response(Some(7))]));
    assert_eq!(model(&rows, 0).issue, Some(CallIssue::StackAlignment));
}
