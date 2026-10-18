use oer_hil_run_bundle::RunStore;
use oer_hil_run_bundle::run::test_support::write_run;
use oer_hil_run_bundle::store::Sidecar;

use super::*;
use crate::runs::tests::bundle;

#[test]
fn over_its_budget_a_store_loses_its_oldest_runs_only_age_kept() {
    let directory = tempfile::tempdir().unwrap();
    let store = RunStore::at(directory.path());
    for (id, started) in [("1-a", 1), ("2-b", 2), ("3-c", 3), ("4-d", 4)] {
        write_run(
            &store.run(id),
            started,
            RunState::Completed,
            Vec::new(),
            |_| {},
        );
    }
    let runs = Run::all(&store).unwrap();
    let weighed = runs
        .iter()
        .zip([10, 20, 30, 40])
        .map(|(run, bytes)| Weighed {
            id: run.id(),
            bytes,
            deletable: run.id() != "2-b",
        })
        .collect::<Vec<_>>();
    // 100 bytes: the oldest goes (90), the kept one stays, the next goes
    // (60) and the store fits.
    assert_eq!(over_budget(&weighed, 65), ["1-a", "3-c"]);
    assert!(over_budget(&weighed, 100).is_empty());
}

#[test]
fn pruning_keeps_what_is_pinned_replayed_recent_and_latest() {
    let directory = tempfile::tempdir().unwrap();
    let store = RunStore::at(directory.path());
    let runs = store.runs();
    let day = 24 * 3600 * 1000;
    bundle(&runs, "old-pass", day, true, None);
    bundle(&runs, "new-pass", 2 * day, true, None);
    for index in 0..4 {
        bundle(
            &runs,
            &format!("fail-{index}"),
            3 * day + index,
            false,
            None,
        );
    }
    bundle(&runs, "baseline", 4 * day, false, None);
    bundle(&runs, "replayer", 5 * day, true, Some("old-pass"));
    bundle(&runs, "recent", 100 * day, false, None);
    let runs = Run::all(&store).unwrap();
    let keep = retained(
        &runs,
        &Retention {
            keep_days: 30,
            keep_failed: 2,
        },
        100 * day,
        &BTreeSet::from([String::from("baseline"), String::from("fail-0")]),
    );
    assert_eq!(keep["baseline"], "pinned");
    assert_eq!(keep["fail-0"], "pinned");
    assert!(keep["old-pass"].starts_with("firmware replayed"));
    assert!(keep["replayer"].starts_with("latest pass"));
    assert!(keep["recent"].starts_with("younger"));
    let deleted = runs
        .iter()
        .filter(|run| !keep.contains_key(run.id()))
        .map(Run::id)
        .collect::<Vec<_>>();
    // The two newest failures are `recent` and `baseline`.
    assert_eq!(deleted, ["new-pass", "fail-1", "fail-2", "fail-3"]);
}

#[test]
fn a_prune_lists_then_deletes_what_no_rule_keeps_and_holds_the_budget() {
    let directory = tempfile::tempdir().unwrap();
    let store = RunStore::at(directory.path());
    let day = 24 * 3600 * 1000;
    bundle(&store.runs(), "old-pass", day, true, None);
    bundle(&store.runs(), "new-pass", 2 * day, true, None);
    store
        .note(
            Sidecar::Pins,
            "old-pass",
            Some(oer_hil_run_bundle::store::Note {
                by: "wifi".into(),
                reason: "baseline".into(),
                unix_millis: 0,
            }),
        )
        .unwrap();
    let rule = Retention {
        keep_days: 0,
        keep_failed: 0,
    };
    // `old-pass` is pinned and `new-pass` the latest pass: nothing goes.
    let listed = prune(&store, &rule, None, false, false, &Default::default()).unwrap();
    assert!(listed.removed.is_empty());
    assert_eq!((listed.kept, listed.total), (2, 2));
    // Over a budget of nothing, a run only its age keeps still stays: the
    // budget never overrides another rule.
    let pruned = prune(&store, &rule, Some(0), true, false, &Default::default()).unwrap();
    assert!(pruned.removed.is_empty(), "{:?}", pruned.removed.len());
    store.note(Sidecar::Pins, "old-pass", None).unwrap();
    let pruned = prune(&store, &rule, None, true, false, &Default::default()).unwrap();
    assert_eq!(
        pruned
            .removed
            .iter()
            .map(|(run, _)| run.id())
            .collect::<Vec<_>>(),
        ["old-pass"]
    );
    assert!(!store.run("old-pass").exists());
    assert!(store.run("new-pass").is_dir());
}

/// A run this build cannot read goes when its schema is older and it is
/// older than the rule's days, or, with `unreadable`, when its schema is
/// this build's; a pinned one, a recent one and one of a newer schema stay.
#[test]
fn unreadable_runs_go_by_schema_and_age() {
    use oer_hil_run_bundle_format::run::RUN_SCHEMA;
    let directory = tempfile::tempdir().unwrap();
    let store = RunStore::at(directory.path());
    let now = oer_durable::unix_millis();
    bundle(&store.runs(), "readable", now, true, None);
    let raw = |id: &str, schema: u64, started: u64| {
        let run = store.run(id);
        fs::create_dir_all(&run).unwrap();
        fs::write(
            run.join("manifest.json"),
            serde_json::to_vec(&serde_json::json!({
                "schema": schema, "run_id": id, "target": "chip-a",
                "started_unix_millis": started,
            }))
            .unwrap(),
        )
        .unwrap();
    };
    let older = u64::from(RUN_SCHEMA) - 1;
    raw("older-old", older, 1);
    raw("older-recent", older, now);
    raw("older-pinned", older, 1);
    raw("current-broken", u64::from(RUN_SCHEMA), 1);
    raw("newer", u64::from(RUN_SCHEMA) + 1, 1);
    store
        .note(
            Sidecar::Pins,
            "older-pinned",
            Some(oer_hil_run_bundle::store::Note {
                by: "wifi".into(),
                reason: "baseline".into(),
                unix_millis: 0,
            }),
        )
        .unwrap();
    let rule = Retention {
        keep_days: 30,
        keep_failed: 0,
    };
    let ids = |runs: &[(Unreadable, u64)]| {
        let mut ids = runs
            .iter()
            .map(|(run, _)| run.id.clone())
            .collect::<Vec<_>>();
        ids.sort();
        ids
    };
    let listed = prune(&store, &rule, None, false, false, &Default::default()).unwrap();
    assert_eq!(ids(&listed.removed_unreadable), ["older-old"]);
    assert_eq!(listed.unreadable_kept.len(), 4);
    assert_eq!((listed.kept, listed.total), (5, 6), "kept and total agree");
    assert_eq!(listed.total, 6);
    let listed = prune(&store, &rule, None, false, true, &Default::default()).unwrap();
    assert_eq!(
        ids(&listed.removed_unreadable),
        ["current-broken", "older-old"]
    );
    assert!(store.run("older-old").is_dir(), "listing deletes nothing");
    prune(&store, &rule, None, true, true, &Default::default()).unwrap();
    for gone in ["older-old", "current-broken"] {
        assert!(!store.run(gone).exists(), "{gone}");
    }
    for kept in ["readable", "older-recent", "older-pinned", "newer"] {
        assert!(store.run(kept).is_dir(), "{kept}");
    }
    // Over a budget of nothing, the older-schema run only its age keeps
    // goes too; the pinned one and the newer-schema one stay.
    let pruned = prune(&store, &rule, Some(0), true, false, &Default::default()).unwrap();
    assert_eq!(ids(&pruned.removed_unreadable), ["older-recent"]);
    for kept in ["readable", "older-pinned", "newer"] {
        assert!(store.run(kept).is_dir(), "{kept}");
    }
}

/// Pruning reads the pins and deletes under their lock: while a pin change
/// holds it, pruning waits, and it deletes only once that change is done.
#[test]
fn pruning_waits_for_a_pin_change_holding_the_lock() {
    let directory = tempfile::tempdir().unwrap();
    let store = RunStore::at(directory.path());
    let day = 24 * 3600 * 1000;
    bundle(&store.runs(), "old-fail", day, false, None);
    let held = store
        .lock_notes(Sidecar::Pins, oer_process::lock::Mode::Exclusive)
        .unwrap();
    let rule = Retention {
        keep_days: 0,
        keep_failed: 0,
    };
    let (done, finished) = std::sync::mpsc::channel();
    std::thread::scope(|scope| {
        let (store, rule) = (&store, &rule);
        scope.spawn(move || {
            done.send(prune(store, rule, None, true, false, &Default::default()))
                .unwrap();
        });
        assert!(
            finished
                .recv_timeout(std::time::Duration::from_millis(300))
                .is_err(),
            "pruning ran while a pin change held the lock"
        );
        assert!(store.run("old-fail").is_dir());
        drop(held);
        let pruned = finished
            .recv_timeout(std::time::Duration::from_secs(30))
            .unwrap()
            .unwrap();
        assert_eq!(pruned.removed.len(), 1);
    });
    assert!(!store.run("old-fail").exists());
}
