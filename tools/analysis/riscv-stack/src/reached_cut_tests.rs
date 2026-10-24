//! A linked call graph: root -> poll -> task -> PHY, with a helper
//! called independently by both root and task.
use super::*;

fn fixture() -> Analysis {
    analyze(
        &built(
            TEXT,
            &[
                Symbol {
                    name: "root",
                    frame: Some(0),
                    words: vec![call(TEXT, TEXT + 12), call(TEXT + 4, TEXT + 32), RET],
                },
                Symbol {
                    name: "dequeue",
                    frame: Some(0),
                    words: vec![0x0007_80e7, RET],
                },
                Symbol {
                    name: "task",
                    frame: Some(0),
                    words: vec![call(TEXT + 20, TEXT + 36), call(TEXT + 24, TEXT + 32), RET],
                },
                Symbol {
                    name: "shared",
                    frame: Some(0),
                    words: vec![RET],
                },
                Symbol {
                    name: "phy",
                    frame: Some(0),
                    words: vec![RET],
                },
            ],
            &Rodata {
                words: &[],
                relocated: &[],
                objects: &[],
            },
        ),
        &[],
        &[],
    )
    .unwrap()
}

#[test]
fn first_discovery_witnesses_use_only_traversed_edges_and_do_not_cross_a_hole() {
    let analysis = fixture();
    let memory = WorkingMemory::new(TEST_MEMORY).unwrap();
    let mut keep = Ledger::new(&memory, &mut || Ok(())).unwrap();
    let mut resolutions = Resolutions::new();
    // The direct shared edge takes precedence over this supplemental target.
    resolutions.add(TEXT + 4, Fact::FieldType, [TEXT + 20]);
    let partial = analysis
        .bound_with(TEXT, &resolutions, &mut keep, &mut || Ok(()))
        .unwrap();
    assert_eq!(partial.witness(TEXT + 20), None);
    assert_eq!(partial.witness(TEXT + 36), None);
    assert_eq!(
        partial.reached,
        BTreeSet::from([TEXT, TEXT + 12, TEXT + 32])
    );
    resolutions.add(TEXT + 12, Fact::FieldType, [TEXT + 20]);
    let complete = analysis
        .bound_with(TEXT, &resolutions, &mut keep, &mut || Ok(()))
        .unwrap();
    assert_eq!(complete.witness(TEXT + 36).unwrap().len(), 3);
    assert_eq!(complete.witness(TEXT + 32).unwrap().len(), 3); // first discovery through task
    assert_eq!(complete.path, vec![(TEXT, 0)]); // equal-depth branches do not replace the critical path
    for &target in &complete.reached {
        let mut caller = TEXT;
        for (callee, parent) in complete.witness(target).unwrap() {
            assert_eq!(parent.caller, caller);
            let transfer = analysis.functions[&caller]
                .transfers
                .iter()
                .find(|transfer| transfer.site == parent.site)
                .unwrap();
            match transfer.target {
                Some(direct) => assert_eq!(callee, direct),
                None => assert!(
                    resolutions
                        .get(parent.site)
                        .unwrap()
                        .targets
                        .contains(&callee)
                ),
            }
            caller = callee;
        }
        assert_eq!(caller, target);
    }
    let clone = complete.clone();
    assert!(std::rc::Rc::ptr_eq(
        &complete.predecessors,
        &clone.predecessors
    ));
    assert!(memory.used() > 0);
    drop(keep);
    assert_eq!(memory.used(), 0);
}

#[test]
fn predecessor_storage_is_refused_before_exceeding_working_memory() {
    let analysis = fixture();
    let memory = WorkingMemory::new(1).unwrap();
    let mut keep = Ledger::new(&memory, &mut || Ok(())).unwrap();
    let error = analysis.bound(TEXT, &mut keep, &mut || Ok(())).unwrap_err();
    assert_eq!(
        error.code,
        oer_analysis_riscv_model::ErrorCode::ResourceLimited
    );
    assert!(error.memory.is_some());
    assert_eq!(memory.peak(), 0);
}

#[test]
fn task_poll_cut_keeps_the_dequeue_owner_and_independent_strict_paths() {
    let analysis = fixture();
    let mut resolutions = Resolutions::new();
    resolutions.add(TEXT + 12, Fact::FieldType, [TEXT + 20]);
    resolutions.add_task_poll(TEXT + 12);
    resolutions.add(TEXT + 24, Fact::FieldType, [TEXT + 36]);
    let cut = task_poll_sites(&resolutions);
    assert_eq!(cut, BTreeSet::from([TEXT + 12]));
    let full = analysis
        .bound_with(TEXT, &resolutions, &mut crate::tests::ledger(), &mut || {
            Ok(())
        })
        .unwrap()
        .reached;
    assert_eq!(
        full,
        BTreeSet::from([TEXT, TEXT + 12, TEXT + 20, TEXT + 32, TEXT + 36])
    );
    assert_eq!(
        analysis
            .reached_cut(
                TEXT,
                &resolutions,
                &BTreeSet::new(),
                &mut crate::tests::ledger(),
                &mut || Ok(())
            )
            .unwrap(),
        full
    );
    let strict = analysis
        .reached_cut(
            TEXT,
            &resolutions,
            &cut,
            &mut crate::tests::ledger(),
            &mut || Ok(()),
        )
        .unwrap();
    assert_eq!(strict, BTreeSet::from([TEXT, TEXT + 12, TEXT + 32]));
    assert_eq!(
        full.difference(&strict).copied().collect::<BTreeSet<_>>(),
        BTreeSet::from([TEXT + 20, TEXT + 36])
    );
    // A supplied direct edge still wins over supplemental resolutions;
    // the generic cut API can also stop that direct transfer.
    assert_eq!(
        analysis
            .reached_cut(
                TEXT,
                &resolutions,
                &BTreeSet::from([TEXT]),
                &mut crate::tests::ledger(),
                &mut || Ok(())
            )
            .unwrap(),
        BTreeSet::from([TEXT, TEXT + 32])
    );
}

#[test]
fn unrelated_facts_are_not_cuts() {
    let analysis = fixture();
    let mut resolutions = Resolutions::new();
    resolutions.add(TEXT + 12, Fact::FieldType, [TEXT + 20]);
    resolutions.add(TEXT + 24, Fact::FieldType, [TEXT + 36]);
    resolutions.add(TEXT + 32, Fact::WakerVtables, [TEXT + 36]);
    assert!(task_poll_sites(&resolutions).is_empty());
    assert_eq!(
        analysis
            .reached_cut(
                TEXT,
                &resolutions,
                &task_poll_sites(&resolutions),
                &mut crate::tests::ledger(),
                &mut || Ok(())
            )
            .unwrap(),
        analysis
            .bound_with(TEXT, &resolutions, &mut crate::tests::ledger(), &mut || Ok(
                ()
            ))
            .unwrap()
            .reached
    );
    assert!(
        analysis
            .reached_cut(
                0,
                &resolutions,
                &BTreeSet::new(),
                &mut crate::tests::ledger(),
                &mut || Ok(())
            )
            .is_err()
    );
}
