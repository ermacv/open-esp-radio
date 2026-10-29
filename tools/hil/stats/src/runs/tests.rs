use std::path::Path;

use oer_hil_model::schema::image::ImageClass;
use oer_hil_runs::format::run::Comparison;
use oer_hil_runs::format::run::Failure;
use oer_hil_runs::format::run::FirmwareArtifact;
use oer_hil_runs::format::run::Measurement;
use oer_hil_runs::format::run::MeasurementUnit;
use oer_hil_runs::format::run::PlannedFirmware;
use oer_hil_runs::format::run::RUN_SCHEMA;
use oer_hil_runs::format::run::RunPlan;
use oer_hil_runs::format::run::ScenarioResult;
use oer_hil_runs::run::test_support::repetition;
use oer_hil_runs::run::test_support::write_run;
use oer_hil_runs::store::Note;

use super::*;

/// A completed run `id` below `runs` of one scenario `s` that passed or
/// failed, with its console, and when it failed a USB event;
/// `replay` names the run whose firmware it replayed.
pub(crate) fn bundle(runs: &Path, id: &str, started: u64, passed: bool, replay: Option<&str>) {
    let directory = runs.join(id);
    let failed = !passed;
    let rate = Measurement::observed(
        "rx.mbps",
        if failed { 10_000_000 } else { 20_000_000 },
        MeasurementUnit::BitsPerSecond,
    )
    .evaluated(Comparison::AtLeast, 15_000_000);
    let outcome = if failed {
        Outcome::Failed
    } else {
        Outcome::Passed
    };
    let failure = failed.then(|| Failure::new(FailureKind::Scenario, "burst missing"));
    let scenario = ScenarioResult::from_repetitions(
        "s".into(),
        ImageClass::SystemWatchdog,
        1,
        vec![repetition("s", 1, outcome, failure, vec![rate])],
    );
    write_run(
        &directory,
        started,
        RunState::Completed,
        vec![scenario],
        |manifest| {
            manifest.invocation = ["~/dev/checkout-a/target/hil/runners/x/runner", "run", "s"]
                .map(String::from)
                .to_vec();
            manifest.repository.commit = "0123456789abcdef".into();
            manifest.firmware.push(
                serde_json::from_value::<FirmwareArtifact>(serde_json::json!({
                    "image": "system-watchdog",
                    "repository": { "commit": "0123456789abcdef", "dirty": false },
                    "application_path": "firmware/system-watchdog/application.bin",
                    "application_size_bytes": 1,
                    "application_sha256": "aabbccddeeff",
                    "runtime_elf_sha256": "01",
                }))
                .unwrap(),
            );
        },
    );
    if let Some(source) = replay {
        oer_os_durable::atomic_json(
            &directory.join("plan.json"),
            &RunPlan {
                schema: RUN_SCHEMA,
                run_id: id.into(),
                selection: "s".into(),
                firmware: Some(PlannedFirmware::Replay {
                    source_run_id: source.into(),
                    image: ImageClass::SystemWatchdog,
                    application_sha256: "aabbccddeeff".into(),
                }),
                entries: Vec::new(),
            },
        )
        .unwrap();
    }
    let repetition = directory.join("scenarios/s/repetition-001");
    fs::write(
        repetition.join("uart.log"),
        "boot\nready\npanic: x\u{1}\u{fffd}\n",
    )
    .unwrap();
    if failed {
        fs::write(
            repetition.join("usb-events.json"),
            serde_json::json!([{
                "realtime_micros": (started + 12_345) * 1000, "device": "3-8",
                "event": "disconnected", "device_number": 19
            }])
            .to_string(),
        )
        .unwrap();
        fs::write(
            repetition.join("cleanup.json"),
            br#"[{"operation":"restore-ap","duration_millis":3,"failure":"ssh lost"}]"#,
        )
        .unwrap();
        fs::create_dir_all(repetition.join("post-mortem")).unwrap();
        fs::write(
            repetition.join("observations.json"),
            br#"{"schema":1,"claim":{"result":"station delivers UDP","not_proven":["air"]},"observations":[{"name":"rx","value":{}},{"name":"phy-tracking","value":{}}]}"#,
        )
        .unwrap();
    }
}

#[test]
fn runs_are_found_explained_and_compared() {
    let directory = tempfile::tempdir().unwrap();
    let store = RunStore::at(directory.path());
    bundle(&store.runs(), "1000-r1", 1000, true, None);
    bundle(&store.runs(), "2000-r2", 2000, false, None);
    let runs = Run::all(&store).unwrap();
    assert_eq!(runs.len(), 2);
    assert_eq!(runs[0].bundle.checkout().as_deref(), Some("checkout-a"));
    let filter = Filter {
        scenario: Some("s".into()),
        outcome: Some("failed".into()),
        ..Filter::default()
    };
    assert_eq!(runs.iter().filter(|run| filter.matches(run)).count(), 1);
    assert!(
        !Filter {
            image: Some("system-panic-reset".into()),
            ..Filter::default()
        }
        .matches(&runs[0])
    );
    assert!(
        Filter {
            image: Some("aabb".into()),
            ..Filter::default()
        }
        .matches(&runs[0])
    );
    let why = why(&runs[1], 2);
    assert!(why.contains("criterion"), "{why}");
    assert!(
        why.contains("missed rx.mbps: 10000000 bit/s (criterion >= 15000000"),
        "{why}"
    );
    assert!(why.contains("cleanup failed: ssh lost"), "{why}");
    assert!(
        why.contains("claim: station delivers UDP (not proven: air)"),
        "{why}"
    );
    assert!(why.contains("observations: rx, phy-tracking"), "{why}");
    assert!(why.contains("| panic: x··"), "{why}");
    assert!(
        why.contains("host usb 3-8 disconnected (device number 19) at +12.345 s of the run"),
        "{why}"
    );
    assert!(!why.contains("| boot"), "{why}");
    let shown = show(&runs[1]);
    let run = fs::canonicalize(store.run("2000-r2")).unwrap();
    assert!(
        shown.contains(&format!("  directory: {}\n", run.display())),
        "{shown}"
    );
    assert!(shown.contains("  s [system-watchdog]: failed\n"), "{shown}");
    assert!(
        shown.contains(&format!(
            "    repetition 1: failed {}\n",
            run.join("scenarios/s/repetition-001").display()
        )),
        "{shown}"
    );
    assert!(
        shown.contains("post-mortem/") && shown.contains("uart.log"),
        "{shown}"
    );
    let compared = compare(&runs[0], &runs[1], Some("rx")).unwrap();
    // One repetition a side is too few to judge, whatever the difference.
    assert!(
        compared.contains("-50.0% insufficient repetitions"),
        "{compared}"
    );
    let history = history(&runs, "s", Some("rx")).unwrap();
    assert_eq!(
        history
            .lines()
            .filter(|l| l.starts_with(char::is_numeric))
            .count(),
        2
    );
    // Without a filter every measurement is a column; a filter that
    // matches nothing leaves none.
    let columns = |text: &str| text.lines().filter(|l| l.contains("column ")).count();
    assert!(columns(&history) > 0, "{history}");
    let all = crate::runs::history(&runs, "s", None).unwrap();
    assert!(columns(&all) >= columns(&history), "{all}");
    assert_eq!(
        columns(&crate::runs::history(&runs, "s", Some("no-such-metric")).unwrap()),
        0
    );
    assert_eq!(
        stability(&runs, "s"),
        "s: 1 of 2 runs passed (50%), 1 newest in a row did not pass\n"
    );
}

#[test]
fn runs_compare_judges_repetitions_with_the_one_noise_aware_comparison() {
    let directory = tempfile::tempdir().unwrap();
    let runs = directory.path().join("runs");
    let run = |id: &str, values: &[u64]| {
        let repetitions = values
            .iter()
            .enumerate()
            .map(|(index, value)| {
                repetition(
                    "s",
                    index as u8 + 1,
                    Outcome::Passed,
                    None,
                    vec![crate::samples::tests::rate(*value)],
                )
            })
            .collect();
        write_run(
            &runs.join(id),
            1,
            RunState::Completed,
            vec![ScenarioResult::from_repetitions(
                "s".into(),
                ImageClass::SystemPanicReset,
                values.len() as u8,
                repetitions,
            )],
            |_| {},
        );
        Run::load(&runs.join(id)).unwrap()
    };
    let a = [100_000_000, 101_000_000, 99_000_000, 100_500_000];
    let b = [110_000_000, 111_000_000, 109_500_000, 110_500_000];
    let (left, right) = (run("1-a", &a), run("2-b", &b));
    let text = compare(&left, &right, None).unwrap();
    let expected = crate::samples::compare(
        Some(crate::samples::Better::Higher),
        &a.map(|value| value as f64),
        &b.map(|value| value as f64),
    )
    .unwrap();
    assert!(text.contains(&expected.verdict.to_string()), "{text}");
    assert!(text.contains("significant, B better"), "{text}");
}

#[test]
fn a_scenario_that_passed_and_failed_is_flaky_and_its_stand_failures_count_apart() {
    let directory = tempfile::tempdir().unwrap();
    let runs = directory.path().join("runs");
    let infrastructure = |message: &str| Some(Failure::new(FailureKind::Infrastructure, message));
    let run = |id: &str, repetitions: Vec<_>| {
        write_run(
            &runs.join(id),
            1,
            RunState::Completed,
            vec![ScenarioResult::from_repetitions(
                "system-watchdog".into(),
                ImageClass::SystemWatchdog,
                repetitions.len() as u8,
                repetitions,
            )],
            |_| {},
        );
        Run::load(&runs.join(id)).unwrap()
    };
    let runs = [
        run(
            "1-a",
            vec![
                repetition("system-watchdog", 1, Outcome::Passed, None, Vec::new()),
                repetition(
                    "system-watchdog",
                    2,
                    Outcome::Blocked,
                    infrastructure("device did not publish a HIL protocol hello"),
                    Vec::new(),
                ),
            ],
        ),
        run(
            "2-b",
            vec![
                repetition("system-watchdog", 1, Outcome::Passed, None, Vec::new()),
                repetition(
                    "system-watchdog",
                    2,
                    Outcome::Failed,
                    infrastructure("unexpected reset"),
                    Vec::new(),
                ),
                repetition(
                    "system-watchdog",
                    3,
                    Outcome::Interrupted,
                    infrastructure("interrupted"),
                    Vec::new(),
                ),
            ],
        ),
    ];
    let stabilities = stabilities(&runs);
    assert_eq!(stabilities.len(), 1);
    let watchdog = &stabilities[0];
    assert_eq!(
        (watchdog.passed, watchdog.failed, watchdog.stand),
        (2, 1, 1)
    );
    assert!(watchdog.is_flaky());
    assert_eq!(watchdog.last_failure.as_deref(), Some("unexpected reset"));
    let quarantined = Notes::from([(
        String::from("system-watchdog"),
        Note {
            by: "wifi".into(),
            reason: "lost hello".into(),
            unix_millis: 1,
        },
    )]);
    let report = flaky_report(&stabilities, 3, &quarantined);
    assert!(report.starts_with(
        " 50% system-watchdog (2 of 4 passed; 1 failed, 1 stand), flaky, quarantined"
    ));
    // Too few repetitions to judge.
    assert!(flaky_report(&stabilities, 5, &quarantined).contains("passed each time"));
}

#[test]
fn waiting_ends_at_the_run_end_with_its_outcome() {
    let directory = tempfile::tempdir().unwrap();
    let event = |kind: &str, outcome: Option<&str>| {
        serde_json::json!({"timestamp_unix_millis": 1, "kind": kind, "scenario": null,
                           "image": null, "outcome": outcome})
        .to_string()
    };
    // A running run whose events say it finished, without a suite: the
    // outcome is unknown, not a pass.
    let running = directory.path().join("1-a");
    write_run(&running, 1, RunState::Running, Vec::new(), |_| {});
    fs::write(
        running.join("events.jsonl"),
        format!(
            "{}\n{}\n",
            event("run-started", None),
            event("run-finished", Some("failed"))
        ),
    )
    .unwrap();
    let mut lines = Vec::new();
    assert_eq!(wait(&running, |line| lines.push(line)).unwrap(), 2);
    assert_eq!(lines, ["0 run-started", "0 run-finished failed"]);
    // A finished run whose events stop short still ends the wait.
    let finished = directory.path().join("2-b");
    write_run(&finished, 1, RunState::Interrupted, Vec::new(), |_| {});
    fs::write(
        finished.join("events.jsonl"),
        format!("{}\n", event("run-started", None)),
    )
    .unwrap();
    assert_eq!(wait(&finished, |_| {}).unwrap(), 2);
    assert_eq!(exit_status(Some(Outcome::Passed)), 0);
    assert_eq!(exit_status(Some(Outcome::Broken)), 1);
    assert_eq!(exit_status(Some(Outcome::BoardQuarantined)), 2);
}

#[test]
fn a_running_run_without_its_runner_is_abandoned() {
    let directory = tempfile::tempdir().unwrap();
    let now = oer_os_durable::unix_millis();
    let running = |id: &str| {
        let run = directory.path().join(id);
        write_run(&run, now, RunState::Running, Vec::new(), |_| {});
        let run = Run::load(&run).unwrap();
        (run.abandoned, run.status())
    };
    // This test process started before the run.
    assert_eq!(
        running(&format!("{now}-{:08x}", std::process::id())),
        (false, "running")
    );
    // No process has PID 0xfffffffe.
    assert_eq!(running(&format!("{now}-fffffffe")), (true, "abandoned"));
}

#[test]
fn why_shows_the_failure_that_blocked_a_scenario_before_any_repetition() {
    let directory = tempfile::tempdir().unwrap();
    let run = directory.path().join("1-r");
    write_run(
        &run,
        0,
        RunState::Completed,
        vec![ScenarioResult::blocked(
            "coex".into(),
            ImageClass::BootSmoke,
            1,
            Failure::new(
                FailureKind::ImageBuild,
                "stack audit failed\nframe grew to 9248 bytes",
            ),
        )],
        |_| {},
    );
    let text = why(&Run::load(&run).unwrap(), 0);
    assert!(text.contains("failure (image-build):"), "{text}");
    assert!(text.contains("      frame grew to 9248 bytes"), "{text}");
}
