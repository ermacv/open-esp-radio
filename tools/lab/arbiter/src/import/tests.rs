use super::*;
use crate::scheduler::Scheduler;

const A: &str = "02:00:00:00:00:0A";
const B: &str = "02:00:00:00:00:0B";

fn previous(directory: &Path, holders: &str) {
    std::fs::create_dir_all(directory).unwrap();
    std::fs::write(
        directory.join("state.json"),
        format!(
            r#"{{"schema": 3, "next_id": 9, "queue": [], "holders": [{holders}],
            "balances": {{"ann": {{"balance_ms": -2397, "last_active_unix_ms": 1}}}},
            "settled_unix_ms": 1}}"#
        ),
    )
    .unwrap();
    std::fs::write(
        directory.join("maintenance.json"),
        format!(
            r#"{{"schema": 1, "boards": [
            {{"mac": "{A}", "owner": "bob", "reason": "rewiring", "since_unix": 5}},
            {{"mac": "{B}", "owner": "quarantine", "reason": "its ROM did not answer",
              "since_unix": 6, "kind": "quarantine", "trigger": "unreachable"}}]}}"#
        ),
    )
    .unwrap();
    std::fs::write(directory.join("history.jsonl"), "{}\n").unwrap();
}

#[test]
fn balances_maintenance_and_quarantine_carry_over() {
    let root = tempfile::tempdir().unwrap();
    let old = root.path().join("cache/arbiter");
    previous(&old, "");
    let arbiter = Arbiter::at(root.path().join("state/arbiter")).unwrap();
    let imported = super::previous(&old, &arbiter).unwrap().unwrap();
    assert_eq!(
        imported,
        Imported {
            balances: 1,
            maintenance: 1,
            quarantined: 1,
            records: vec!["history.jsonl".to_owned()],
        }
    );
    assert!(!old.exists() && root.path().join("cache").join(IMPORTED).is_dir());
    assert!(arbiter.directory().join("history.jsonl").exists());
    let (scheduler, _) = Scheduler::open(
        &arbiter.state_path(),
        vec![format!("board:{A}"), format!("board:{B}")],
        1,
    )
    .unwrap();
    assert_eq!(scheduler.maintenance()[0].reason, "rewiring");
    assert!(matches!(
        scheduler.readiness(&format!("board:{B}")),
        Some(Readiness::Unavailable { emission: Emission::Unknown, reason, .. })
            if reason.contains("its ROM did not answer")
    ));
    // Once imported, nothing is imported again.
    previous(&old, "");
    assert_eq!(super::previous(&old, &arbiter).unwrap(), None);
}

#[test]
fn a_queue_with_leases_held_is_not_imported() {
    let root = tempfile::tempdir().unwrap();
    let old = root.path().join("arbiter");
    previous(&old, r#"{"held": true}"#);
    let arbiter = Arbiter::at(root.path().join("state")).unwrap();
    let refused = super::previous(&old, &arbiter).unwrap_err().to_string();
    assert!(refused.contains("still holds 1 lease"), "{refused}");
    assert!(old.join("state.json").exists() && !arbiter.state_path().exists());
}
