//! A run bundle of the current schema stays readable.
//!
//! `schema/run-schema-<RUN_SCHEMA>/` holds every document a run bundle of
//! that schema carries, the fixture records of a repetition included (all
//! but `helper.json`, which wraps a family's own type), with its optional
//! parts present (asserted below, so
//! a sample cannot lose one unnoticed). Each document must
//! read as its type and write back to the same JSON, and carry the current
//! version of its own schema where the format publishes one as a constant
//! (a fixture record's version is its writer's literal; the samples hold
//! what those writers produce). A change that a stored bundle of this schema
//! would not survive (a new required field, a renamed, removed or retyped
//! one) fails here: it needs a new `RUN_SCHEMA` (or the document's own
//! schema constant), with these documents moved to the new directory and
//! brought to the new shape. A new optional field that defaults when absent
//! reads every stored bundle and passes.

use std::{
    fs,
    path::{Path, PathBuf},
};

use oer_hil_model::schema::run::RunEvent;
use oer_hil_runs::format::lab::{FixtureObservation, LAB_PROVENANCE_SCHEMA, LabProvenance};
use oer_hil_runs::format::run::fixtures::{AirMonitor, Applied, Protection, Reception};
use oer_hil_runs::format::run::{
    CleanupRecord, OBSERVATIONS_SCHEMA, Observations, PlannedFirmware, RUN_SCHEMA, RunManifest,
    RunPlan, ScenarioResult, SuiteResult, UsbEvent,
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;

fn samples() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/schema")
}

/// The directory of the current schema's documents.
fn current() -> PathBuf {
    let directories = fs::read_dir(samples())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect::<Vec<_>>();
    let expected = format!("run-schema-{RUN_SCHEMA}");
    assert_eq!(
        directories,
        std::slice::from_ref(&expected),
        "tests/schema holds the documents of the current RUN_SCHEMA only: after a \
         schema bump, move them to {expected} and bring them to the new shape"
    );
    samples().join(expected)
}

/// Reads `name` as `T` and checks that it writes back to the same JSON.
fn read<T: DeserializeOwned + Serialize>(name: &str) -> T {
    let path = current().join(name);
    let text = fs::read_to_string(&path).unwrap();
    let document: T = serde_json::from_str(&text).unwrap_or_else(|error| {
        panic!(
            "{} no longer reads ({error}); a stored bundle of schema {RUN_SCHEMA} would \
             not either: bump the schema",
            path.display()
        )
    });
    let original: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        serde_json::to_value(&document).unwrap(),
        original,
        "{} does not write back unchanged: a field was renamed, removed or retyped; \
         bump the schema",
        path.display()
    );
    document
}

#[test]
fn every_document_of_the_current_schema_reads_and_writes_back() {
    let manifest: RunManifest = read("manifest.json");
    assert_eq!(manifest.schema, RUN_SCHEMA);
    assert!(
        manifest.experiment.is_some(),
        "the sample covers an A/B run"
    );
    assert!(manifest.firmware[0].replayed_from.is_some());
    let variant = &manifest.experiment.as_ref().unwrap().variant;
    assert!(!variant.features.is_empty());
    let plan: RunPlan = read("plan.json");
    assert_eq!(plan.schema, RUN_SCHEMA);
    assert!(matches!(
        plan.firmware,
        Some(PlannedFirmware::Replay { .. })
    ));
    assert!(
        plan.entries
            .iter()
            .any(|entry| entry.requirements.is_some())
    );
    let suite: SuiteResult = read("suite.json");
    assert_eq!(suite.schema, RUN_SCHEMA);
    let scenario: ScenarioResult = read("scenario-result.json");
    assert_eq!(scenario.schema, RUN_SCHEMA);
    let measurement = &scenario.repetitions[0].measurements[0];
    assert!(measurement.threshold.is_some() && measurement.verdict.is_some());
    assert!(scenario.failure.is_some() && scenario.repetitions[0].failure.is_some());
    assert!(!scenario.repetitions[0].attachments.is_empty());
    let observations: Observations = read("observations.json");
    assert_eq!(observations.schema, OBSERVATIONS_SCHEMA);
    assert!(
        observations
            .claim
            .is_some_and(|claim| !claim.not_proven.is_empty())
    );
    let cleanup: Vec<CleanupRecord> = read("cleanup.json");
    assert!(cleanup[0].failure.is_some());
    let usb: Vec<UsbEvent> = read("usb-events.json");
    assert!(!usb.is_empty());
    let lab: LabProvenance = read("lab-provenance.json");
    assert_eq!(lab.schema, LAB_PROVENANCE_SCHEMA);
    assert!(matches!(lab.fixture, FixtureObservation::OpenWrt(_)));
    assert!(lab.host.interfaces[0].wireless_link.is_some() && !lab.host.ipv4_routes.is_empty());
    // The fixture records a repetition carries; `helper.json` wraps a
    // family's own type and has no shape of the bundle's.
    let openwrt: Applied = read("fixture-applied-openwrt.json");
    assert!(matches!(openwrt, Applied::OpenWrt(applied) if applied.requested.beacon.is_some()));
    let local: Applied = read("fixture-applied-local.json");
    assert!(matches!(local, Applied::Local(_)));
    let protection: Protection = read("fixture-protection.json");
    assert!(protection.established.is_some() && !protection.windows.is_empty());
    let monitor: AirMonitor = read("fixture-monitor.json");
    assert!(monitor.target_egress.data_to_block_ack.is_some());
    let reception: Reception = read("udp-reception.json");
    assert!(reception.error.is_some() && !reception.bursts.is_empty());
    let events = fs::read_to_string(current().join("events.jsonl")).unwrap();
    for line in events.lines() {
        let event: RunEvent = serde_json::from_str(line).unwrap();
        assert_eq!(
            serde_json::to_value(&event).unwrap(),
            serde_json::from_str::<Value>(line).unwrap()
        );
    }
}
