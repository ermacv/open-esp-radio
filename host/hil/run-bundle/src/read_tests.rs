use std::{fs, path::PathBuf};

use oer_hil_schema::image::ImageClass;
use oer_hil_schema::run::RunEventKind;

use crate::run::Views;
use crate::run::test_support::session;
use crate::run::test_support::temporary_directory;
use oer_hil_run_bundle_format::read::*;
use oer_hil_run_bundle_format::run::CLEANUP_FILE;
use oer_hil_run_bundle_format::run::Comparison;
use oer_hil_run_bundle_format::run::Measurement;
use oer_hil_run_bundle_format::run::MeasurementUnit;
use oer_hil_run_bundle_format::run::OBSERVATIONS_FILE;
use oer_hil_run_bundle_format::run::Outcome;
use oer_hil_run_bundle_format::run::RUN_SCHEMA;
use oer_hil_run_bundle_format::run::USB_EVENTS_FILE;
use oer_hil_run_bundle_format::run::UsbEventKind;
use oer_hil_run_bundle_format::run::{
    RepetitionResult, RunManifest, RunState, ScenarioResult, SuiteResult,
};

fn no_views(_: &SuiteResult, _: &RunManifest) -> Views {
    Views::default()
}

/// A run directory named like a run, below a fresh store.
fn run_directory(label: &str) -> PathBuf {
    let directory = temporary_directory(label).join("runs/1700000000000-00000abc");
    fs::create_dir_all(&directory).unwrap();
    directory
}

fn passed(scenario: &str, artifacts: &str) -> ScenarioResult {
    ScenarioResult::from_repetitions(
        scenario.to_owned(),
        ImageClass::BootSmoke,
        1,
        vec![RepetitionResult {
            schema: RUN_SCHEMA,
            repetition: 1,
            outcome: Outcome::Passed,
            started_unix_millis: 1,
            duration_millis: 2,
            artifact_directory: PathBuf::from(artifacts),
            attachments: Vec::new(),
            measurements: vec![
                Measurement::observed("boot.time", 900, MeasurementUnit::Microseconds)
                    .evaluated(Comparison::AtMost, 1_000),
            ],
            failure: None,
        }],
    )
}

/// A completed run of one passed scenario with a cleanup and a USB record.
fn finished(label: &str) -> RunBundle {
    let directory = run_directory(label);
    let session = session(&directory);
    let artifacts = "scenarios/boot-smoke/repetition-001";
    fs::create_dir_all(directory.join(artifacts)).unwrap();
    fs::write(
        directory.join(artifacts).join(CLEANUP_FILE),
        br#"[{"operation":"restore-ap","duration_millis":3,"failure":"ssh lost"}]"#,
    )
    .unwrap();
    fs::write(
        directory.join(artifacts).join(OBSERVATIONS_FILE),
        br#"{"schema":1,"claim":{"result":"boots","not_proven":["radio"]},"observations":[{"name":"boot-001","value":{"boot_id":"00ab"}}]}"#,
    )
    .unwrap();
    fs::write(
        directory.join(artifacts).join(USB_EVENTS_FILE),
        br#"[{"realtime_micros":5,"device":"3-8","event":"disconnected","device_number":2}]"#,
    )
    .unwrap();
    session
        .finish(vec![passed("boot-smoke", artifacts)], no_views)
        .unwrap();
    RunBundle::open(&directory).unwrap().unwrap()
}

#[test]
fn a_finished_run_reads_back_typed() {
    let bundle = finished("read-typed");
    bundle.validate().unwrap();
    assert_eq!(bundle.id(), "1700000000000-00000abc");
    assert_eq!(bundle.manifest().state, RunState::Completed);
    let suite = bundle.suite().unwrap().unwrap();
    assert_eq!(suite.outcome, Outcome::Passed);
    let repetition = &suite.scenarios[0].repetitions[0];
    assert_eq!(repetition.measurements[0].name, "boot.time");
    assert_eq!(repetition.measurements[0].value, 900);
    let cleanup = bundle.cleanup(repetition).unwrap();
    assert_eq!(cleanup[0].operation, "restore-ap");
    assert_eq!(cleanup[0].failure.as_deref(), Some("ssh lost"));
    let observations = bundle.observations(repetition).unwrap().unwrap();
    assert_eq!(observations.claim.unwrap().not_proven, ["radio"]);
    assert_eq!(observations.observations[0].name, "boot-001");
    assert_eq!(observations.observations[0].value["boot_id"], "00ab");
    let usb = bundle.usb_events(repetition).unwrap();
    assert_eq!(usb[0].kind, UsbEventKind::Disconnected { device_number: 2 });
    let events = bundle.events().unwrap();
    assert_eq!(events.last().unwrap().kind, RunEventKind::RunFinished);
    assert!(bundle.plan().unwrap().is_none());
    assert!(bundle.replayed_runs().unwrap().is_empty());
    // A finished run carries no seal.
    assert!(!bundle.directory().join("integrity.json").exists());
    assert!(!bundle.directory().join("attempts").exists());
}

#[test]
fn a_run_without_a_manifest_is_unpublished_and_a_foreign_one_is_an_error() {
    let directory = run_directory("read-unpublished");
    assert!(RunBundle::open(&directory).unwrap().is_none());
    fs::write(directory.join(MANIFEST), br#"{"schema":3}"#).unwrap();
    assert!(RunBundle::open(&directory).is_err());
}

#[test]
fn a_run_of_an_older_schema_is_refused_by_name_and_kept() {
    let bundle = finished("read-older-schema");
    let path = bundle.directory().join(MANIFEST);
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    manifest["schema"] = serde_json::json!(RUN_SCHEMA - 1);
    let bytes = serde_json::to_vec(&manifest).unwrap();
    fs::write(&path, &bytes).unwrap();
    let error = RunBundle::open(bundle.directory()).unwrap_err().to_string();
    assert!(
        error.contains(&format!(
            "has schema {}, expected {RUN_SCHEMA}",
            RUN_SCHEMA - 1
        )),
        "{error}"
    );
    // Reading it changes nothing.
    assert_eq!(fs::read(&path).unwrap(), bytes);
}
