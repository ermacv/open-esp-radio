//! A linked call graph: root -> dequeue -> task -> PHY, with a helper
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
fn task_poll_cut_keeps_the_dequeue_owner_and_independent_strict_paths() {
    let analysis = fixture();
    let mut resolutions = Resolutions::new();
    resolutions.add(TEXT + 12, Fact::TaskPolls, [TEXT + 20]);
    resolutions.add(TEXT + 24, Fact::FieldType, [TEXT + 36]);
    let cut = task_poll_sites(&resolutions);
    assert_eq!(cut, BTreeSet::from([TEXT + 12]));
    let full = analysis.bound_with(TEXT, &resolutions).unwrap().reached;
    assert_eq!(
        full,
        BTreeSet::from([TEXT, TEXT + 12, TEXT + 20, TEXT + 32, TEXT + 36])
    );
    assert_eq!(
        analysis
            .reached_cut(TEXT, &resolutions, &BTreeSet::new())
            .unwrap(),
        full
    );
    let strict = analysis.reached_cut(TEXT, &resolutions, &cut).unwrap();
    assert_eq!(strict, BTreeSet::from([TEXT, TEXT + 12, TEXT + 32]));
    assert_eq!(
        full.difference(&strict).copied().collect::<BTreeSet<_>>(),
        BTreeSet::from([TEXT + 20, TEXT + 36])
    );
    // A supplied direct edge still wins over supplemental resolutions;
    // the generic cut API can also stop that direct transfer.
    assert_eq!(
        analysis
            .reached_cut(TEXT, &resolutions, &BTreeSet::from([TEXT]))
            .unwrap(),
        BTreeSet::from([TEXT, TEXT + 32])
    );
}

#[test]
fn no_tasks_is_a_cut_without_a_task_root_and_unrelated_facts_are_not_cuts() {
    let analysis = fixture();
    let mut resolutions = Resolutions::new();
    resolutions.add(TEXT + 12, Fact::NoTasks, []);
    resolutions.add(TEXT + 24, Fact::FieldType, [TEXT + 36]);
    resolutions.add(TEXT + 32, Fact::WakerVtables, [TEXT + 36]);
    assert_eq!(task_poll_sites(&resolutions), BTreeSet::from([TEXT + 12]));
    assert!(resolutions.get(TEXT + 12).unwrap().targets.is_empty());
    assert_eq!(
        analysis
            .reached_cut(TEXT, &resolutions, &task_poll_sites(&resolutions))
            .unwrap(),
        analysis.bound_with(TEXT, &resolutions).unwrap().reached
    );
    assert!(
        analysis
            .reached_cut(0, &resolutions, &BTreeSet::new())
            .is_err()
    );
}
