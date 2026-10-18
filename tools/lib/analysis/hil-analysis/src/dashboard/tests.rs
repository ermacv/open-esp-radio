use oer_hil_run_bundle::run::test_support::write_run;
use oer_hil_run_bundle_format::run::RunState;

use super::*;

#[test]
fn a_running_run_reports_its_step_scenario_and_finished_count() {
    let directory = tempfile::tempdir().unwrap();
    let run = directory.path().join("1-a");
    write_run(&run, 1, RunState::Running, Vec::new(), |_| {});
    let event = |millis: u64, kind: &str, scenario: Option<&str>| {
        let outcome = (kind == "scenario-finished").then_some("passed");
        json!({"timestamp_unix_millis": millis, "kind": kind, "scenario": scenario,
               "image": null, "outcome": outcome})
        .to_string()
    };
    std::fs::write(
        run.join("events.jsonl"),
        [
            event(1, "run-started", None),
            event(2, "scenario-started", Some("a")),
            event(3, "scenario-finished", Some("a")),
            event(4, "scenario-started", Some("b")),
        ]
        .join("\n")
            + "\n",
    )
    .unwrap();
    let entry = |scenario: &str, disposition: &str| {
        json!({"scenario": scenario, "image": "system-watchdog", "repetitions": 1,
               "disposition": disposition, "reason": null})
    };
    std::fs::write(
        run.join("plan.json"),
        json!({"schema": 3, "run_id": "1-a", "selection": "s", "entries": [
            entry("a", "selected"), entry("b", "selected"), entry("c", "selected"),
            entry("d", "filtered"),
        ]})
        .to_string(),
    )
    .unwrap();
    let bundle = RunBundle::open(&run).unwrap().unwrap();
    assert_eq!(
        progress(&bundle).unwrap(),
        json!({"step": "scenario-started", "step_since_millis": 4, "scenario": "b",
               "finished": 1, "planned": 3, "scenarios": ["a", "b", "c"],
               "outcomes": {"a": "passed"}})
    );
    // An event this build does not know is not guessed at.
    let mut events = std::fs::read_to_string(run.join("events.jsonl")).unwrap();
    events.push_str(&format!("{}\n", event(5, "future-step", None)));
    std::fs::write(run.join("events.jsonl"), events).unwrap();
    assert_eq!(progress(&bundle), None);
}

#[test]
fn the_newest_runs_are_read_newest_first() {
    let directory = tempfile::tempdir().unwrap();
    let store = RunStore::at(directory.path());
    for (id, started) in [("1000-a", 1000), ("2000-b", 2000), ("3000-c", 3000)] {
        write_run(
            &store.run(id),
            started,
            RunState::Completed,
            Vec::new(),
            |_| {},
        );
    }
    // A directory that is no run is passed over.
    std::fs::create_dir_all(store.runs().join("runs.before-shared-store")).unwrap();
    let newest = newest_runs(&store, 2);
    assert_eq!(
        newest
            .iter()
            .map(|run| run["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["3000-c", "2000-b"]
    );
    assert_eq!(newest[0]["state"], "completed");
}
