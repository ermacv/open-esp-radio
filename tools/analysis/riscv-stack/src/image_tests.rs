use super::*;

fn observation(records: &[FunctionRecord]) -> Observation {
    let memory = WorkingMemory::new(1024 * 1024).unwrap();
    let mut ledger = Ledger::new(&memory, &mut || Ok(())).unwrap();
    let mut observer = Observer {
        ledger: &mut ledger,
        deepest: 0,
        dynamic: false,
        expression_limited: false,
        unexpanded: Vec::new(),
        site_depths: BTreeMap::new(),
        site_registers: BTreeMap::new(),
        transfers: Vec::new(),
        predecessors: BTreeMap::new(),
        open_graph: OpenGraph::default(),
    };
    for record in records {
        observer.record(record, &mut || Ok(())).unwrap();
    }
    Observation {
        depth: Some(0),
        complete: false,
        expression_limited: observer.expression_limited,
        limit_events: [0; 2],
        resolved: observer.transfers,
        local_jumps: observer.unexpanded,
        site_depths: observer.site_depths,
        site_registers: observer.site_registers,
        predecessors: observer.predecessors,
        open_graph: observer.open_graph,
    }
}

fn edge(from: u64, target: Option<u64>, relation: EdgeKind, external: bool) -> FunctionRecord {
    FunctionRecord::Edge {
        from,
        target,
        relation,
        external,
    }
}

#[test]
fn expression_limit_is_retained_by_the_stack_observer() {
    let records = [FunctionRecord::SemanticGap {
        offset: 0x1000,
        reason: SemanticGapReason::ExpressionLimit,
    }];
    let observed = observation(&records);
    assert!(observed.expression_limited);
    assert!(!observation(&[]).expression_limited);
}

#[test]
fn closed_graph_keeps_the_shared_contract_for_every_edge_and_gap() {
    for relation in [
        EdgeKind::Taken,
        EdgeKind::Fallthrough,
        EdgeKind::Jump,
        EdgeKind::Call,
        EdgeKind::PossibleContinuation,
        EdgeKind::Indirect,
        EdgeKind::Return,
        EdgeKind::Trap,
        EdgeKind::Stop,
        EdgeKind::Conflict,
    ] {
        for target in [None, Some(0x1010)] {
            for external in [false, true] {
                let record = edge(0x1000, target, relation, external);
                assert_eq!(
                    observation(std::slice::from_ref(&record)).closed_graph(),
                    oer_analysis_riscv_analysis::writes::closes_local_graph(&record),
                    "{record:?}",
                );
            }
        }
    }
    for reason in [
        GapReason::Unvisited,
        GapReason::UnsupportedInstruction,
        GapReason::ConflictingBoundary,
        GapReason::DeclaredData,
    ] {
        let record = FunctionRecord::Gap {
            start: 0x1000,
            length: 4,
            reason,
        };
        assert_eq!(
            observation(std::slice::from_ref(&record)).closed_graph(),
            oer_analysis_riscv_analysis::writes::closes_local_graph(&record),
        );
    }
    assert!(observation(&[]).closed_graph());
    assert!(!observation(&[]).closed_except(0x1000));
}

#[test]
fn continuations_must_be_local_and_conflicts_and_stops_always_stay_open() {
    for relation in [EdgeKind::Fallthrough, EdgeKind::PossibleContinuation] {
        assert!(observation(&[edge(0x1000, Some(0x1010), relation, false)]).closed_graph());
        assert!(!observation(&[edge(0x1000, Some(0x1010), relation, true)]).closed_graph());
    }
    for relation in [EdgeKind::Conflict, EdgeKind::Stop] {
        for target in [None, Some(0x1010)] {
            for external in [false, true] {
                assert!(!observation(&[edge(0x1000, target, relation, external)]).closed_graph());
            }
        }
    }
    assert!(observation(&[edge(0x1000, None, EdgeKind::Call, true)]).closed_graph());
}

#[test]
fn closed_except_requires_only_the_external_unresolved_indirect_exit() {
    let exit = edge(0x1000, None, EdgeKind::Indirect, true);
    let observed = observation(&[exit.clone(), exit.clone()]);
    assert!(!observed.closed_graph());
    assert!(observed.closed_except(0x1000));
    assert!(!observed.closed_except(0x1004));
    for other in [
        edge(0x1004, None, EdgeKind::Indirect, true),
        edge(0x1000, None, EdgeKind::Indirect, false),
        edge(0x1000, None, EdgeKind::Conflict, true),
        edge(0x1000, None, EdgeKind::Stop, true),
        edge(0x1000, Some(0x1010), EdgeKind::Indirect, false),
        FunctionRecord::Gap {
            start: 0x1000,
            length: 4,
            reason: GapReason::UnsupportedInstruction,
        },
    ] {
        for records in [[exit.clone(), other.clone()], [other.clone(), exit.clone()]] {
            let observed = observation(&records);
            assert!(!observed.closed_graph());
            assert!(!observed.closed_except(0x1000), "{records:?}");
        }
    }
    let mut observed = observed;
    observed.open_graph.whole = true;
    assert!(!observed.closed_graph());
    assert!(!observed.closed_except(0x1000));
}
