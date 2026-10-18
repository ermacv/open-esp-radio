use super::*;
pub(super) fn symbol_elf(code: &[u32], base: u32, goal: u32) -> (Vec<u8>, SymbolId) {
    let mut bytes = elf(code);
    for offset in [24, 60, 64] {
        bytes[offset..offset + 4].copy_from_slice(&base.to_le_bytes());
    }
    let strings = b"\0entry\0goal\0alias\0data\0";
    let string_offset = bytes.len() as u32;
    bytes.extend_from_slice(strings);
    while !bytes.len().is_multiple_of(4) {
        bytes.push(0);
    }
    let symbols = bytes.len() as u32;
    bytes.extend_from_slice(&[0; 16]);
    for (name, address, info) in [
        (1u32, base, 0x12u8),
        (7, goal, 0x10),
        (12, goal, 0x12),
        (18, goal, 0x11),
    ] {
        bytes.extend_from_slice(&name.to_le_bytes());
        bytes.extend_from_slice(&address.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes()); // zero-sized exact boundary
        bytes.extend_from_slice(&[info, 0, 1, 0]);
    }
    let sections = bytes.len() as u32;
    for fields in [
        [0; 10],
        [0, 1, 6, base, 256, code.len() as u32 * 4, 0, 0, 2, 0],
        [0, 3, 0, 0, string_offset, strings.len() as u32, 0, 0, 1, 0],
        [0, 2, 0, 0, symbols, 80, 2, 1, 4, 16],
    ] {
        for word in fields {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
    }
    bytes[32..36].copy_from_slice(&sections.to_le_bytes());
    bytes[46..48].copy_from_slice(&40u16.to_le_bytes());
    bytes[48..50].copy_from_slice(&4u16.to_le_bytes());
    bytes[50..52].copy_from_slice(&2u16.to_le_bytes());
    let point = SymbolId {
        object: ObjectId {
            artifact: ArtifactId::of_bytes(&bytes),
            location: ObjectLocation::Standalone,
        },
        table: SymbolTableKind::Static,
        table_section: 3,
        index: 2,
    };
    (bytes, point)
}
pub(super) fn fixture(code: &[u32], goal: u32) -> (Fixture, SymbolId) {
    let (bytes, point) = symbol_elf(code, 0x1000, goal);
    (Fixture::from_inputs(vec![bytes]), point)
}
fn request(f: &Fixture, goal: ExecutionGoal) -> ExecutionRequest {
    let mut request = f.request();
    for phase in &mut request.cases {
        phase.relation.as_mut().unwrap().returns.low = matches!(goal, ExecutionGoal::Return);
    }
    request.cases[0].vendor.goal = goal;
    request.cases[0].replacement = Some(request.cases[0].vendor.clone());
    request
}
#[test]
fn exact_goal_boundaries_stop_before_callee_body() {
    // Save return link; call a NOTYPE, zero-sized physical boundary whose body is unsupported.
    let (f, point) = fixture(
        &[0x00008413, 0x00c000ef, 0x00040067, 0x00000013, 0x00000073],
        0x1010,
    );
    for (goal, kind, pc) in [
        (
            ExecutionGoal::ReachSymbol {
                target: point.clone(),
            },
            "reached-symbol",
            0x1010,
        ),
        (
            ExecutionGoal::ObserveCall {
                target: point.clone(),
                include_tail: false,
            },
            "observed-call",
            0x1004,
        ),
    ] {
        let r = request(&f, goal);
        let run = f.run(r, budget()).unwrap();
        let result = run.facts();
        assert_eq!(result["complete"], true);
        assert_eq!(result["verdict"], "MATCH");
        assert_eq!(result["records"][0]["stop"]["kind"], kind);
        assert_eq!(result["records"][0]["stop"]["pc"], pc);
        assert_eq!(result["records"][0]["steps"], 2);
    }
    let run = f.run(f.request(), budget()).unwrap();
    assert_eq!(run.facts()["verdict"], "INCOMPLETE");
}
#[test]
fn known_indirect_calls_tail_policy_and_returns_are_distinct() {
    for (code, tail) in [
        (
            vec![0x000012b7, 0x01028293, 0x000280e7, 0x00008067, 0x00000073],
            false,
        ),
        (
            vec![0x010002ef, 0x00008067, 0x00000013, 0x00000013, 0x00000073],
            false,
        ),
        (
            vec![0x0100006f, 0x00008067, 0x00000013, 0x00000013, 0x00000073],
            true,
        ),
    ] {
        let (f, point) = fixture(&code, 0x1010);
        let run = f
            .run(
                request(
                    &f,
                    ExecutionGoal::ObserveCall {
                        target: point.clone(),
                        include_tail: true,
                    },
                ),
                budget(),
            )
            .unwrap();
        let result = run.facts();
        assert_eq!(result["records"][0]["stop"]["kind"], "observed-call");
        assert_eq!(result["records"][0]["stop"]["tail"], tail);
        if tail {
            let run = f
                .run(
                    request(
                        &f,
                        ExecutionGoal::ObserveCall {
                            target: point,
                            include_tail: false,
                        },
                    ),
                    budget(),
                )
                .unwrap();
            assert_eq!(run.facts()["verdict"], "INCOMPLETE");
        }
    }
    // A canonical return to the boundary is not an observed call, even with tails enabled.
    let (f, point) = fixture(
        &[0x000010b7, 0x01008093, 0x00008067, 0x00000013, 0x00000073],
        0x1010,
    );
    let run = f
        .run(
            request(
                &f,
                ExecutionGoal::ObserveCall {
                    target: point,
                    include_tail: true,
                },
            ),
            budget(),
        )
        .unwrap();
    assert_eq!(run.facts()["records"][0]["stop"]["kind"], "incomplete");
    // `jalr a1`: an unsupplied argument register is an unknown target.
    let (f, point) = fixture(&[0x000580e7, 0x00000073], 0x1004);
    let mut unsupplied = request(
        &f,
        ExecutionGoal::ObserveCall {
            target: point,
            include_tail: true,
        },
    );
    unsupplied.cases[0].vendor.arguments = vec![Some(0)];
    unsupplied.cases[0].replacement = Some(unsupplied.cases[0].vendor.clone());
    let run = f.run(unsupplied, budget()).unwrap();
    assert_eq!(
        run.facts()["records"][0]["stop"]["reason"]["kind"],
        "unknown-register"
    );
}
#[test]
fn premature_return_blocks_warm_phase_and_equal_prefixes_do_not_match() {
    let (f, point) = fixture(&[0x00008067, 0x00000073], 0x1004);
    let mut r = request(&f, ExecutionGoal::ReachSymbol { target: point });
    let mut warm = r.cases[0].clone();
    warm.reset = SessionReset::Warm;
    warm.name = "dependent".into();
    r.cases.push(warm);
    let run = f.run(r, budget()).unwrap();
    let result = run.facts();
    assert_eq!(result["verdict"], "INCOMPLETE");
    assert_eq!(result["records"][0]["stop"]["kind"], "goal-not-reached");
    assert_eq!(
        result["records"][3]["stop"]["kind"],
        "blocked-by-prior-phase"
    );
}
#[test]
fn invalid_physical_goals_and_relations_are_rejected() {
    let (f, point) = fixture(&[0x00008067, 0x00008067], 0x1004);
    for which in 0..5 {
        let mut point = point.clone();
        match which {
            0 => point.index = 900,
            1 => point.table_section = 2,
            2 => point.table = SymbolTableKind::Dynamic,
            3 => point.index = 4, // data symbol in executable section
            _ => point.object.artifact = ArtifactId::of_bytes(b"different"),
        }
        let run = f.run(
            request(&f, ExecutionGoal::ReachSymbol { target: point }),
            budget(),
        );
        assert!(run.is_err(), "{run:?}");
    }
    for which in 0..4 {
        let mut r = request(
            &f,
            ExecutionGoal::ReachSymbol {
                target: point.clone(),
            },
        );
        match which {
            0 => r.cases[0].relation.as_mut().unwrap().returns.low = true,
            // A vendor prefix compares no return word.
            1 => {
                r.cases[0].replacement.as_mut().unwrap().goal = ExecutionGoal::Return;
                r.cases[0].relation.as_mut().unwrap().returns.low = true;
            }
            // Only the vendor side may stop at a symbol boundary.
            2 => {
                let symbol = r.cases[0].vendor.goal.clone();
                r.cases[0].vendor.goal = ExecutionGoal::Return;
                r.cases[0].replacement.as_mut().unwrap().goal = symbol;
            }
            _ => {
                if let ExecutionGoal::ReachSymbol { target } = &mut r.cases[0].vendor.goal {
                    target.object.artifact = ArtifactId::of_bytes(b"an unmapped executable");
                }
            }
        }
        match f.run(r, budget()) {
            Ok(_) => panic!("invalid goal relation admitted"),
            Err(error) => assert_eq!(error.code, ErrorCode::InvalidRequest),
        }
    }
}
#[test]
fn comparison_of_observed_call_prefixes_keeps_known_differences() {
    let (f, point) = fixture(
        &[0x00b52023, 0x00c000ef, 0x00008067, 0x00000013, 0x00000073],
        0x1010,
    );
    let mut r = request(
        &f,
        ExecutionGoal::ObserveCall {
            target: point,
            include_tail: false,
        },
    );
    r.cases[0].vendor.arguments = vec![Some(0x3000), Some(7)];
    r.cases[0]
        .vendor
        .models
        .push(register_bank(vec![RegisterCell {
            address: 0x3000,
            width: 4,
            value: 0,
        }]));
    r.cases[0].replacement = Some(r.cases[0].vendor.clone());
    let run = f.run(r.clone(), budget()).unwrap();
    assert_eq!(run.facts()["verdict"], "MATCH");
    r.cases[0].replacement.as_mut().unwrap().arguments[1] = Some(8);
    let run = f.run(r, budget()).unwrap();
    assert_eq!(run.facts()["verdict"], "DIFF");
}

#[test]
fn multiple_physical_goals_in_one_request_share_its_failure_budgets() {
    let (f, point) = fixture(
        &[0x00008413, 0x00c000ef, 0x00040067, 0x00000013, 0x00000073],
        0x1010,
    );
    let mut r = request(
        &f,
        ExecutionGoal::ReachSymbol {
            target: point.clone(),
        },
    );
    let mut second = r.cases[0].clone();
    second.reset = SessionReset::Warm;
    second.name = "different-physical-alias".into();
    let mut alias = point;
    alias.index = 3;
    second.vendor.goal = ExecutionGoal::ObserveCall {
        target: alias,
        include_tail: false,
    };
    second.replacement = Some(second.vendor.clone());
    r.cases.push(second);
    let mut third = r.cases[0].clone();
    third.reset = SessionReset::Warm;
    third.name = "already-at-boundary".into();
    third.vendor.entry = 0x1010;
    third.replacement = Some(third.vendor.clone());
    r.cases.push(third);
    let result = f.run(r, budget()).unwrap().facts();
    assert_eq!(result["verdict"], "MATCH");
    assert_eq!(result["records"][6]["steps"], 0);
    let (f, point) = fixture(&[0x0000006f, 0x00000073], 0x1004);
    let r = request(&f, ExecutionGoal::ReachSymbol { target: point });
    let mut b = budget();
    b.max_work_units = Some(10000);
    let run = f.run(r.clone(), b);
    assert_eq!(run.unwrap_err().code, ErrorCode::ResourceLimited);
    let mut b = budget();
    b.working_memory_bytes = Some(1024 * 1024);
    let run = f.run(r.clone(), b);
    assert_eq!(run.unwrap_err().code, ErrorCode::ResourceLimited);
}

#[test]
fn symbol_goals_require_and_use_the_explicit_companion_mapping() {
    let (companion, point) = symbol_elf(&[0x00000073], 0x2000, 0x2000);
    let f = Fixture::from_inputs(vec![elf(&[0x000022b7, 0x00028067]), companion]);
    let mut r = request(&f, ExecutionGoal::ReachSymbol { target: point });
    assert_eq!(r.validate().unwrap_err().code, ErrorCode::InvalidRequest);
    r.vendor.executables.push(f.id(1));
    r.replacement.as_mut().unwrap().executables.push(f.id(1));
    let run = f.run(r, budget()).unwrap();
    let result = run.facts();
    assert_eq!(result["verdict"], "MATCH");
    assert_eq!(result["records"][0]["stop"]["pc"], 0x2000);
}
#[test]
fn in_process_symbol_goals_resolve_in_the_executables_and_compare_vendor_prefixes() {
    // Save the return link, call a returning leaf, then return through it.
    let code = [0x00008413, 0x008000ef, 0x00040067, 0x00008067];
    let (bytes, point) = symbol_elf(&code, 0x1000, 0x100c);
    let f = Fixture::from_inputs(vec![bytes.clone()]);
    let sources: &[app::in_process::Executable] =
        &[app::in_process::Executable::new(bytes.clone())];
    let memory = WorkingMemory::new(32 * 1024 * 1024).unwrap();
    let verify = |request: &ExecutionRequest| {
        app::in_process::verify(
            &app::in_process::InProcessComparison {
                request,
                executables: sources,
                effects: &[],
                projections: &[],
                vendor_results: None,
                dependence: None,
                patches: &[],
            },
            &blobray_backend_riscv::RiscvExecutor,
            &memory,
            &mut || Ok(()),
        )
    };
    let observe = ExecutionGoal::ObserveCall {
        target: point,
        include_tail: false,
    };
    let mut request = f.request();
    request.cases[0].relation = Some(fixture_relation(false));
    request.cases[0].vendor.goal = observe.clone();
    // Both sides stop before the call.
    let mut both = request.clone();
    both.cases[0].replacement = Some(both.cases[0].vendor.clone());
    let result = verify(&both).unwrap();
    assert_eq!(result.verdict, Some(ComparisonVerdict::Match));
    assert!(result.records.iter().any(|r| matches!(
        r,
        ExecutionEvidence::Outcome {
            replacement: false,
            stop: ExecutionStop::ObservedCall { pc: 0x1004, .. },
            ..
        }
    )));
    // The vendor prefix before the call against the complete replacement,
    // in process and through the project.
    let result = verify(&request).unwrap();
    assert_eq!(result.verdict, Some(ComparisonVerdict::Match));
    let run = f.run(request.clone(), budget()).unwrap();
    assert_eq!(run.facts()["verdict"], "MATCH");
    // A prefix compares no return word.
    let mut returns = request;
    returns.cases[0].relation = Some(fixture_relation(true));
    assert_eq!(
        verify(&returns).err().unwrap().code,
        ErrorCode::InvalidRequest
    );
}
