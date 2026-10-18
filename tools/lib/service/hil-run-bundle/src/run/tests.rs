use super::test_support::{integrated_session, manifest, session, temporary_directory};
use super::*;
use crate::build::capture_source_material;
use oer_tool_data_hil_run_bundle_format::build::SourceLimitation;
use oer_tool_data_hil_run_bundle_format::build::SourceRebuildStatus;

#[test]
fn messages_used_collects_both_directions_from_every_capture() {
    let directory = temporary_directory("messages-used");
    let capture = directory.join("scenarios/a/repetition-001/image-preflight");
    fs::create_dir_all(&capture).unwrap();
    fs::write(
        directory.join("scenarios/a/repetition-001/protocol.jsonl"),
        concat!(
            r#"{"record":"host-command","command":{"path":"wifi/initialize","request_id":1}}"#,
            "\n",
            r#"{"record":"target-event","message":{"path":"base/hello"}}"#,
            "\n",
            r#"{"record":"link-health","health":null}"#,
            "\n",
        ),
    )
    .unwrap();
    fs::write(
        capture.join("protocol.jsonl"),
        r#"{"record":"target-event","message":{"path":"base/hello"}}"#,
    )
    .unwrap();
    assert_eq!(
        messages_used(&directory).unwrap(),
        ["base/hello", "wifi/initialize"]
    );
}

fn no_views(_: &SuiteResult, _: &RunManifest) -> Views {
    Views::default()
}

fn failed_suite() -> SuiteResult {
    let failure = Failure::new(FailureKind::Scenario, "bad <frame> & timeout");
    let scenarios = vec![ScenarioResult::from_repetitions(
        String::from("udp-rx"),
        ImageClass::SystemWatchdog,
        1,
        vec![RepetitionResult {
            schema: RUN_SCHEMA,
            repetition: 1,
            outcome: Outcome::Failed,
            started_unix_millis: 1,
            duration_millis: 250,
            artifact_directory: PathBuf::from("scenarios/udp-rx/repetition-001"),
            attachments: Vec::new(),
            measurements: vec![
                Measurement::observed("udp.rx.loss", 2, MeasurementUnit::Count)
                    .evaluated(Comparison::AtMost, 0),
            ],
            failure: Some(failure),
        }],
    )];
    SuiteResult {
        schema: RUN_SCHEMA,
        run_id: String::from("run<&>"),
        target: String::from("chip-a"),
        outcome: Outcome::Failed,
        started_unix_millis: 1,
        finished_unix_millis: 251,
        duration_millis: 250,
        counts: SuiteCounts::from_results(&scenarios),
        scenarios,
    }
}

#[test]
fn evaluated_measurement_binds_threshold_and_verdict() {
    let passed = Measurement::observed("icmp.rtt.p95", 900, MeasurementUnit::Microseconds)
        .evaluated(Comparison::AtMost, 1_000);
    let failed = Measurement::observed("icmp.rtt.p95", 1_001, MeasurementUnit::Microseconds)
        .evaluated(Comparison::AtMost, 1_000);
    assert_eq!(passed.verdict, Some(MeasurementVerdict::Passed));
    assert_eq!(failed.verdict, Some(MeasurementVerdict::Failed));
    assert!(passed.is_consistent());
    assert!(failed.is_consistent());
}

#[test]
fn current_measurement_contract_conformance() {
    let cases: serde_json::Value = serde_json::from_str(include_str!(
        "../../../hil-runner/tests-fixtures/fixtures/evidence/measurements.json"
    ))
    .unwrap();
    for case in cases.as_array().unwrap() {
        let outcome: Outcome = serde_json::from_value(case["outcome"].clone()).unwrap();
        let accepted = serde_json::from_value::<Vec<Measurement>>(case["measurements"].clone())
            .is_ok_and(|measurements| {
                let mut suite = failed_suite();
                let repetition = &mut suite.scenarios[0].repetitions[0];
                repetition.measurements = measurements;
                repetition.outcome = outcome;
                repetition.failure =
                    (!outcome.is_passed()).then(|| Failure::new(FailureKind::Scenario, "fixture"));
                suite.scenarios[0].outcome = outcome;
                suite.outcome = if outcome.is_passed() {
                    Outcome::Passed
                } else {
                    Outcome::Failed
                };
                suite.counts = SuiteCounts::from_results(&suite.scenarios);
                let mut manifest = manifest();
                manifest.finished_unix_millis = Some(suite.finished_unix_millis);
                manifest.duration_millis = Some(suite.duration_millis);
                validation::validate_suite(&suite, &manifest).is_ok()
            });
        assert_eq!(
            accepted,
            case["valid"].as_bool().unwrap(),
            "{}",
            case["name"]
        );
    }
}

#[test]
fn unique_run_directories_never_replace_a_previous_run() {
    let root = temporary_directory("run");
    let first = create_unique_directory(&root, "123-abc").unwrap();
    fs::write(first.join("evidence"), b"retained").unwrap();
    let second = create_unique_directory(&root, "123-abc").unwrap();
    assert_ne!(first, second);
    assert_eq!(fs::read(first.join("evidence")).unwrap(), b"retained");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn repository_archive_seals_tracked_patch_and_marks_untracked_content_incomplete() {
    let base = temporary_directory("source-archive");
    let repository = base.join("repository");
    fs::create_dir(&repository).unwrap();
    for arguments in [
        &["init", "-q", "-b", "main"][..],
        &["config", "user.name", "HIL Test"],
        &["config", "user.email", "hil@example.invalid"],
        &[
            "remote",
            "add",
            "origin",
            "https://example.invalid/repository.git",
        ],
    ] {
        oer_tool_base_process::git::run(&repository, arguments).unwrap();
    }
    fs::write(repository.join("tracked.txt"), b"base\n").unwrap();
    oer_tool_base_process::git::run(&repository, ["add", "tracked.txt"]).unwrap();
    oer_tool_base_process::git::run(&repository, ["commit", "-m", "base"]).unwrap();
    fs::write(repository.join("tracked.txt"), b"changed\n").unwrap();
    let tracked_run = base.join("tracked-run");
    fs::create_dir(&tracked_run).unwrap();
    let tracked_source = capture_source_material(
        "repository",
        &repository,
        &tracked_run,
        Path::new("source/repository.patch"),
    )
    .unwrap();
    assert!(tracked_source.dirty);
    assert_eq!(
        tracked_source.rebuild_status,
        SourceRebuildStatus::TrackedPatch
    );
    assert!(tracked_source.limitations.is_empty());
    assert!(tracked_source.tracked_patch_size_bytes.unwrap() != 0);
    assert!(
        fs::read_to_string(
            tracked_run.join(tracked_source.tracked_patch_path.expect("tracked patch"))
        )
        .unwrap()
        .contains("+changed")
    );

    fs::write(repository.join("untracked.txt"), b"untracked\n").unwrap();
    let incomplete_run = base.join("incomplete-run");
    fs::create_dir(&incomplete_run).unwrap();
    let incomplete_source = capture_source_material(
        "repository",
        &repository,
        &incomplete_run,
        Path::new("source/repository.patch"),
    )
    .unwrap();
    assert_eq!(
        incomplete_source.rebuild_status,
        SourceRebuildStatus::Incomplete
    );
    assert_eq!(
        incomplete_source.limitations,
        [SourceLimitation::UntrackedContentNotArchived]
    );
    assert_eq!(incomplete_source.untracked_files.len(), 1);
    assert_eq!(
        incomplete_source.untracked_files[0].path,
        Path::new("untracked.txt")
    );
    assert_eq!(incomplete_source.untracked_files[0].size_bytes, 10);
    assert_eq!(incomplete_source.untracked_files[0].sha256.len(), 64);
    fs::remove_dir_all(base).unwrap();
}

#[test]
fn attachments_are_sorted_and_content_addressed() {
    let root = temporary_directory("attachments");
    fs::create_dir(root.join("nested")).unwrap();
    fs::write(root.join("z.log"), b"serial evidence").unwrap();
    fs::write(root.join("nested/capture.pcapng"), b"pcap evidence").unwrap();
    let attachments =
        collect_attachments(&root, Path::new("scenario/repetition-001")).expect("index artifacts");
    assert_eq!(attachments.len(), 2);
    assert_eq!(
        attachments[0].path,
        PathBuf::from("scenario/repetition-001/nested/capture.pcapng")
    );
    assert_eq!(attachments[0].media_type, "application/vnd.tcpdump.pcap");
    assert_eq!(attachments[1].media_type, "text/plain");
    assert_eq!(attachments[1].size_bytes, 15);
    assert_eq!(attachments[1].sha256.len(), 64);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn replay_import_rejects_paths_outside_the_sealed_bundle() {
    assert!(archive::validate_replayed_source_path(Path::new("../application.bin")).is_err());
    assert!(archive::validate_replayed_source_path(Path::new("/tmp/application.bin")).is_err());
    assert!(archive::validate_replayed_source_path(Path::new("firmware/runtime.elf")).is_ok());
}

#[test]
fn outcome_aggregation_is_fail_closed() {
    assert_eq!(aggregate_outcome([]), Outcome::Skipped);
    assert_eq!(aggregate_outcome([Outcome::Passed]), Outcome::Passed);
    assert_eq!(
        aggregate_outcome([Outcome::Passed, Outcome::Blocked]),
        Outcome::Blocked
    );
    assert_eq!(
        aggregate_outcome([Outcome::Failed, Outcome::Interrupted]),
        Outcome::Interrupted
    );
}

#[test]
fn finish_writes_all_views_and_completes_manifest() {
    let root = temporary_directory("finish");
    let scenarios = failed_suite().scenarios;
    let (suite, completion) = integrated_session(&root)
        .finish(scenarios, no_views)
        .unwrap();
    assert_eq!(suite.outcome, Outcome::Failed);
    assert!(completion.suite_report.is_file());
    assert!(completion.junit_report.is_file());
    assert!(completion.html_report.is_file());
    assert!(completion.integrity_report.is_file());
    let final_manifest: RunManifest =
        serde_json::from_slice(&fs::read(completion.run_directory.join("manifest.json")).unwrap())
            .unwrap();
    assert_eq!(final_manifest.state, RunState::Completed);
    assert!(final_manifest.finished_unix_millis.is_some());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn dropped_session_marks_manifest_interrupted() {
    let root = temporary_directory("interrupted");
    drop(session(&root));
    let final_manifest: RunManifest =
        serde_json::from_slice(&fs::read(root.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(final_manifest.state, RunState::Interrupted);
    assert!(final_manifest.finished_unix_millis.is_some());
    assert!(root.join("integrity.json").is_file());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn failure_before_seal_cannot_leave_a_false_completed_manifest() {
    let root = temporary_directory("finish-before-seal-failure");
    let session = integrated_session(&root);
    let run_directory = session.directory().to_owned();
    fs::create_dir(run_directory.join("junit.xml")).unwrap();

    assert!(session.finish(Vec::new(), no_views).is_err());
    let manifest: RunManifest =
        serde_json::from_slice(&fs::read(run_directory.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest.state, RunState::Interrupted);
    assert!(run_directory.join("suite.json").is_file());
    assert!(!run_directory.join("report.html").exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn seal_failure_rolls_the_manifest_back_to_interrupted() {
    let root = temporary_directory("seal-failure");
    let session = integrated_session(&root);
    let run_directory = session.directory().to_owned();
    fs::create_dir(run_directory.join("integrity.json")).unwrap();

    assert!(session.finish(Vec::new(), no_views).is_err());
    let manifest: RunManifest =
        serde_json::from_slice(&fs::read(run_directory.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest.state, RunState::Interrupted);
    assert!(run_directory.join("suite.json").is_file());
    assert!(run_directory.join("junit.xml").is_file());
    assert!(run_directory.join("report.html").is_file());
    assert!(run_directory.join("integrity.json").is_dir());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn broken_or_interrupted_repetitions_can_retain_failed_measurements() {
    for outcome in [Outcome::Broken, Outcome::Interrupted] {
        let mut suite = failed_suite();
        suite.scenarios[0].repetitions[0].outcome = outcome;
        suite.scenarios[0].outcome = outcome;
        suite.counts = SuiteCounts::from_results(&suite.scenarios);
        let mut manifest = manifest();
        manifest.finished_unix_millis = Some(suite.finished_unix_millis);
        manifest.duration_millis = Some(suite.duration_millis);
        validation::validate_suite(&suite, &manifest).unwrap();
    }
}

#[test]
fn a_staged_image_keeps_its_runtime_and_bootstrap_subjects() {
    let staged = serde_json::json!({
        "image": "boot-smoke",
        "application_path": "firmware/boot-smoke/application.bin",
        "application_size_bytes": 1,
        "application_sha256": "00",
        "runtime_elf_sha256": "01",
        "runtime_bin_sha256": "02",
        "bootstrap_elf_sha256": "03",
    });
    let artifact: FirmwareArtifact = serde_json::from_value(staged.clone()).unwrap();
    assert_eq!(serde_json::to_value(&artifact).unwrap(), staged);
    assert_eq!(
        artifact.required_subjects(),
        ["runtime.bin", "bootstrap.elf"]
    );
    let files = artifact.subjects().map(|subject| subject.file);
    assert_eq!(files, ["runtime.elf", "runtime.bin", "bootstrap.elf"]);
}
