use super::run;
use super::*;

/// `beq a0, zero, +8`, `li a0, 1`, `ret`, then an unreachable `li a0, 2`.
const PROGRAM: [u32; 4] = [0x00050463, 0x00100513, 0x00008067, 0x00200513];

fn coverage(rows: &[ExecutionEvidence], side: bool) -> ExecutionCoverage {
    let mut found = rows.iter().filter_map(|r| match r {
        ExecutionEvidence::Coverage {
            replacement,
            coverage,
        } if *replacement == side => Some(coverage.clone()),
        _ => None,
    });
    let coverage = found.next().expect("side coverage");
    assert!(found.next().is_none(), "one coverage record per side");
    coverage
}

#[test]
fn coverage_accumulates_each_side_across_cold_sessions() {
    let f = Fixture::new(&PROGRAM);
    let mut r = f.request();
    // Vendor: taken, then (after a cold reset) fallthrough. Replacement: taken twice.
    let mut second = r.cases[0].clone();
    second.name = "fallthrough".into();
    second.reset = SessionReset::Cold;
    second.vendor.arguments[0] = Some(5);
    r.cases.push(second);
    let (_, rows) = run(&f, r);
    assert!(matches!(
        rows.last(),
        Some(ExecutionEvidence::Coverage {
            replacement: true,
            ..
        })
    ));
    assert_eq!(
        coverage(&rows, false),
        ExecutionCoverage {
            instructions: vec![0x1000, 0x1004, 0x1008],
            branches: vec![BranchCoverage {
                site: 0x1000,
                taken: true,
                fallthrough: true,
            }],
            transfers: vec![],
        }
    );
    assert_eq!(
        coverage(&rows, true),
        ExecutionCoverage {
            instructions: vec![0x1000, 0x1008],
            branches: vec![BranchCoverage {
                site: 0x1000,
                taken: true,
                fallthrough: false,
            }],
            transfers: vec![],
        }
    );
}

#[test]
fn coverage_does_not_depend_on_selected_timeline() {
    let f = Fixture::new(&PROGRAM);
    let r = f.request();
    let (_, plain) = run(&f, r.clone());
    let mut traced = r;
    let case = &mut traced.cases[0];
    case.vendor.observe_timeline.branches = true;
    case.replacement.as_mut().unwrap().observe_timeline.branches = true;
    case.relation.as_mut().unwrap().events.timeline.branches = true;
    let (_, traced) = run(&f, traced);
    assert_eq!(coverage(&plain, false), coverage(&traced, false));
    assert!(traced.iter().any(|r| matches!(
        r,
        ExecutionEvidence::Event {
            event: ExecutionEvent::Branch { .. },
            ..
        }
    )));
}

/// Root at 0x1000: save `ra` in `s0`, branch on `a0`. Fallthrough calls the
/// modeled 0x2000 through `lui`/`jalr`, calls the helper at 0x1028 and jumps
/// to the epilogue; the taken side calls through `a1`. 0x1024 is dead code.
const CLOSURE: [u32; 11] = [
    0x00008413, // 0x1000 mv s0, ra
    0x00050a63, // 0x1004 beq a0, zero, 0x1018
    0x000022b7, // 0x1008 lui t0, 0x2
    0x000280e7, // 0x100c jalr ra, 0(t0)
    0x018000ef, // 0x1010 jal ra, 0x1028
    0x0080006f, // 0x1014 j 0x101c
    0x000580e7, // 0x1018 jalr ra, 0(a1)
    0x00040093, // 0x101c mv ra, s0
    0x00008067, // 0x1020 ret
    0x00200513, // 0x1024 li a0, 2
    0x00008067, // 0x1028 ret
];

fn closure_request(f: &Fixture, a0: u32, a1: u32) -> ExecutionRequest {
    let mut r = f.request();
    r.replacement = None;
    r.binding = None;
    let case = &mut r.cases[0];
    case.relation = None;
    case.replacement = None;
    case.vendor.arguments[0] = Some(a0);
    case.vendor.arguments[1] = Some(a1);
    case.vendor.calls = vec![CallDeclaration {
        repetition: CallRepetition::Unbounded,
        id: "external".into(),
        applicability: "synthetic external ABI assumption".into(),
        lifetime: RegionLifetime::Phase,
        binding: CallBinding {
            address: 0x2000,
            boundary: CallBoundary::Unmapped,
            allow_tail: false,
        },
        argument_words: 0,
        responses: vec![CallResponse {
            return_words: [Some(0), None],
            outputs: vec![],
            allocation: None,
            delay_micros: None,
        }],
    }];
    r
}

/// Vendor coverage of `requests`, each executed in process over the
/// fixture's executables.
fn report(f: &Fixture, requests: &[ExecutionRequest]) -> Result<CodeCoverageReport> {
    let executed: Vec<_> = requests
        .iter()
        .map(|r| f.run(r.clone(), budget()).unwrap())
        .collect();
    let executions: Vec<_> = requests
        .iter()
        .zip(&executed)
        .enumerate()
        .map(|(i, (r, e))| {
            (
                ArtifactId::of_bytes(&i.to_le_bytes()),
                r,
                e.records.as_slice(),
            )
        })
        .collect();
    app::in_process::coverage(
        &executions,
        &f.inputs,
        &oer_riscv_lift::RiscvDecoder,
        &WorkingMemory::new(32 * 1024 * 1024).unwrap(),
        &mut || Ok(()),
    )
}

#[test]
fn code_coverage_reports_root_closures_boundaries_and_unions() {
    let f = Fixture::new(&CLOSURE);
    let fallthrough = closure_request(&f, 5, 0);
    let one = report(&f, std::slice::from_ref(&fallthrough)).unwrap();
    assert_eq!(one.executions.len(), 1);
    assert_eq!(one.outside, 0);
    let [root, helper] = &one.functions[..] else {
        panic!("{one:?}")
    };
    assert_eq!(root.entry, 0x1000);
    assert_eq!(root.name, None);
    assert_eq!(root.modeled, vec![0x100c]);
    assert_eq!(root.unresolved, vec![0x1018]);
    assert!(root.followed.is_empty());
    assert!(root.gaps.is_empty());
    assert_eq!(
        root.blocks,
        CoverageCount {
            reached: 3,
            total: 4
        }
    );
    assert_eq!(root.uncovered_blocks, vec![0x1018]);
    assert_eq!(
        root.uncovered_directions,
        vec![BranchDirection {
            site: 0x1004,
            taken: true
        }]
    );
    assert_eq!(helper.entry, 0x1028);
    assert_eq!(
        helper.blocks,
        CoverageCount {
            reached: 1,
            total: 1
        }
    );
    let [root_report] = &one.roots[..] else {
        panic!("{one:?}")
    };
    assert_eq!(root_report.functions, vec![0x1000, 0x1028]);
    assert_eq!(
        root_report.blocks,
        CoverageCount {
            reached: 4,
            total: 5
        }
    );
    assert_eq!(
        root_report.directions,
        CoverageCount {
            reached: 1,
            total: 2
        }
    );

    // The taken side calls the helper through `a1`; together both executions
    // reach every block and direction.
    let taken = closure_request(&f, 0, 0x1028);
    let (_, rows) = super::run(&f, closure_request(&f, 0, 0x1028));
    assert_eq!(
        coverage(&rows, false).transfers,
        vec![IndirectTransfer {
            site: 0x1018,
            target: 0x1028
        }]
    );
    let both = report(&f, &[fallthrough.clone(), taken]).unwrap();
    assert!(both.roots[0].blocks.complete() && both.roots[0].directions.complete());
    // The observed target is followed; the site stays visible, since other
    // targets no execution reached are outside the closure.
    assert!(both.functions[0].unresolved.is_empty(), "{both:?}");
    assert_eq!(both.functions[0].followed, vec![0x1018]);
    assert!(
        both.functions
            .iter()
            .all(|f| f.uncovered_blocks.is_empty() && f.uncovered_directions.is_empty())
    );

    // Executions of different vendor targets do not combine.
    let mut other = closure_request(&f, 5, 0);
    other.vendor.stack.fill = Some(0);
    assert!(report(&f, &[fallthrough, other]).is_err());
}

#[test]
fn a_jump_to_a_defined_function_start_is_a_named_tail_call() {
    // 0x1000 `j 0x1008`; 0x1004 dead `ret`; 0x1008 `ret`.
    let code = [0x0080006f, 0x00008067, 0x00008067];
    let symbols = [("root", 0x1000, 8), ("tail", 0x1008, 4)];
    let named = Fixture::from_inputs(vec![elf_with_symbols(&code, &symbols)]);
    let mut r = named.request();
    r.cases[0].relation = None;
    r.cases[0].replacement = None;
    r.replacement = None;
    r.binding = None;
    let named_report = report(&named, &[r]).unwrap();
    let names: Vec<_> = named_report
        .functions
        .iter()
        .map(|f| (f.entry, f.name.as_deref(), f.blocks))
        .collect();
    assert_eq!(
        names,
        vec![
            (
                0x1000,
                Some("root"),
                CoverageCount {
                    reached: 1,
                    total: 1
                }
            ),
            (
                0x1008,
                Some("tail"),
                CoverageCount {
                    reached: 1,
                    total: 1
                }
            ),
        ]
    );
    assert_eq!(named_report.roots[0].name.as_deref(), Some("root"));
    // Without symbols the same jump stays inside the root.
    let plain = Fixture::new(&code);
    let mut r = plain.request();
    r.cases[0].relation = None;
    r.cases[0].replacement = None;
    r.replacement = None;
    r.binding = None;
    let plain_report = report(&plain, &[r]).unwrap();
    assert_eq!(plain_report.functions.len(), 1);
    assert_eq!(
        plain_report.functions[0].blocks,
        CoverageCount {
            reached: 2,
            total: 2
        }
    );
}

#[test]
fn in_process_verification_needs_symbol_executables_and_one_executable_per_source() {
    let f = Fixture::new(&CLOSURE);
    let mut request = closure_request(&f, 5, 0);
    // Compare the program with itself.
    request.replacement = Some(request.vendor.clone());
    request.binding = Some(CompiledBinding::SharedCore);
    request.cases[0].replacement = Some(request.cases[0].vendor.clone());
    request.cases[0].relation = Some(fixture_relation(true));
    let executable = elf(&CLOSURE);
    let sources: &[app::in_process::Executable] =
        &[app::in_process::Executable::new(executable.clone())];
    let memory = WorkingMemory::new(32 * 1024 * 1024).unwrap();
    // A symbol goal names a code symbol of an executable of the target, and
    // every executable a target names must be given.
    let mut symbolic = request.clone();
    symbolic.cases[0].vendor.goal = ExecutionGoal::ReachSymbol {
        target: SymbolId {
            object: ObjectId {
                artifact: ArtifactId::of_bytes(b"object"),
                location: ObjectLocation::Standalone,
            },
            table: SymbolTableKind::Static,
            table_section: 1,
            index: 1,
        },
    };
    symbolic.cases[0].replacement = Some(symbolic.cases[0].vendor.clone());
    symbolic.cases[0].relation = Some(fixture_relation(false));
    for (request, executables, reason) in [
        (&symbolic, sources, "goal symbol"),
        (&request, &[][..], "an executable that was not given"),
    ] {
        let error = app::in_process::verify(
            &app::in_process::InProcessComparison {
                request,
                executables,
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
        .err()
        .unwrap();
        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert!(error.message.contains(reason), "{error:?}");
    }
}

#[test]
fn reused_vendor_results_yield_the_records_of_a_full_execution() {
    let executable = elf(&CLOSURE);
    let sources: &[app::in_process::Executable] =
        &[app::in_process::Executable::new(executable.clone())];
    let memory = WorkingMemory::new(32 * 1024 * 1024).unwrap();
    fn input<'a>(
        request: &'a ExecutionRequest,
        sources: &'a [app::in_process::Executable],
    ) -> app::in_process::InProcessComparison<'a> {
        app::in_process::InProcessComparison {
            request,
            executables: sources,
            effects: &[],
            projections: &[],
            vendor_results: None,
            dependence: None,
            patches: &[],
        }
    }
    let executor = &blobray_backend_riscv::RiscvExecutor;
    let verify = |request: &ExecutionRequest, vendor: Option<&app::in_process::VendorResults>| {
        app::in_process::verify(
            &app::in_process::InProcessComparison {
                vendor_results: vendor,
                ..input(request, sources)
            },
            executor,
            &memory,
            &mut || Ok(()),
        )
    };
    let f = Fixture::new(&CLOSURE);
    // A cold and a warm case comparing the program with itself; the
    // replacement's arguments select its path.
    let compared = |replacement_a0: u32, replacement_a1: u32| {
        let mut request = closure_request(&f, 5, 0);
        request.replacement = Some(request.vendor.clone());
        request.binding = Some(CompiledBinding::SharedCore);
        let mut replacement = request.cases[0].vendor.clone();
        replacement.arguments[0] = Some(replacement_a0);
        replacement.arguments[1] = Some(replacement_a1);
        request.cases[0].replacement = Some(replacement);
        request.cases[0].relation = Some(fixture_relation(true));
        let mut warm = request.cases[0].clone();
        warm.name = "warm".into();
        warm.reset = SessionReset::Warm;
        request.cases.push(warm);
        request
    };
    let request = compared(5, 0);
    let vendor =
        app::in_process::vendor(&input(&request, sources), executor, &memory, &mut || Ok(()))
            .unwrap();
    // The same vendor side under other replacements.
    for replacement in [(5, 0), (0, 0x1028)] {
        let request = compared(replacement.0, replacement.1);
        let full = verify(&request, None).unwrap();
        assert!(!full.vendor_reused);
        let reused = verify(&request, Some(&vendor)).unwrap();
        assert!(reused.vendor_reused);
        assert_eq!(reused.records, full.records);
        assert_eq!(reused.verdict, full.verdict);
        assert_eq!(reused.complete, full.complete);
    }
    // Vendor results of another vendor side are rejected.
    let mut other = compared(5, 0);
    other.cases[1].vendor.arguments[0] = Some(0);
    let error = verify(&other, Some(&vendor)).err().unwrap();
    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert!(error.message.contains("another vendor side"), "{error:?}");
    // A replacement that cannot complete blocks the vendor side of the warm
    // case, which then depends on it: the request executes fully.
    let blocking = compared(0, 0x5000);
    let blocked_full = verify(&blocking, None).unwrap();
    assert!(blocked_full.records.iter().any(|r| matches!(
        r,
        ExecutionEvidence::Outcome {
            case: 1,
            replacement: false,
            stop: ExecutionStop::BlockedByPriorPhase,
            ..
        }
    )));
    let blocked = verify(&blocking, Some(&vendor)).unwrap();
    assert!(!blocked.vendor_reused);
    assert_eq!(blocked.records, blocked_full.records);
    assert_eq!(blocked.verdict, blocked_full.verdict);
}
