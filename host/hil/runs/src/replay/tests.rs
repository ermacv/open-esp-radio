use super::*;
use crate::format::lab::AccessPointDefinition;
use crate::format::lab::FixtureObservation;
use crate::format::lab::HostInterfaceObservation;
use crate::format::lab::HostObservation;
use crate::format::lab::LabDefinition;
use crate::format::lab::LabProvenance;
use crate::format::lab::SensitiveValueDisposition;
use crate::format::lab::StationFixtureDefinition;
use crate::format::lab::StationIpv4Definition;
use crate::format::run::Attachment;
use crate::format::run::Measurement;
use crate::format::run::MeasurementUnit;
use crate::format::run::Outcome;
use crate::format::run::RepetitionResult;
use crate::format::run::ScenarioResult;
use crate::format::run::SuiteCounts;
use crate::format::run::SuiteResult;
use crate::run::sha256_file;
use oer_hil_model::schema::image::ImageClass;
use oer_os_durable::atomic_json;
use std::sync::atomic::{AtomicU64, Ordering};

static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

/// The replay check of the fixture's run `run-1` of `chip-a`.
fn validate(root: &Path) -> Result<RunManifest> {
    validate_source(&root.join(crate::store::CHECKOUT_RUNS), "chip-a", "run-1")
}

fn fixture() -> (PathBuf, PathBuf) {
    let root = std::env::temp_dir().join(format!(
        "open-radio-hil-verification-{}-{}",
        std::process::id(),
        TEST_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let run = root.join("target/hil/runs/run-1");
    let artifact_path = PathBuf::from("scenarios/icmp/repetition-001/evidence.log");
    let application_path = PathBuf::from("firmware/system-watchdog/application.bin");
    fs::create_dir_all(run.join(artifact_path.parent().unwrap())).unwrap();
    fs::create_dir_all(run.join(application_path.parent().unwrap())).unwrap();
    fs::write(run.join(&artifact_path), b"serial evidence").unwrap();
    fs::write(run.join(&application_path), b"flashed application").unwrap();
    let attachment_sha256 = sha256_file(&run.join(&artifact_path)).unwrap();
    let application_sha256 = sha256_file(&run.join(&application_path)).unwrap();
    atomic_json(
        &run.join("manifest.json"),
        &serde_json::json!({
            "schema": RUN_SCHEMA,
            "run_id": "run-1",
            "target": "chip-a",
            "state": "completed",
            "started_unix_millis": 100,
            "finished_unix_millis": 200,
            "duration_millis": 100,
            "invocation": ["cargo", "hil", "run", "icmp"],
            "repository": {
                "commit": "0123456789abcdef",
                "dirty": false
            },
            "runner": {
                "package": "runner",
                "version": "1",
                "messages_lock_sha256": "00",
                "host_os": "linux",
                "host_arch": "x86_64",
                "tools": []
            },
            "cell": {
                "cell_id": "cell-1",
                "device_id": "dut-1",
                "serial_device": "/dev/ttyACM0"
            },
            "firmware": [{
                "image": "system-watchdog",
                "repository": { "commit": "0123456789abcdef", "dirty": false },
                "application_path": application_path,
                "application_size_bytes": 19,
                "application_sha256": application_sha256,
                "runtime_elf_sha256": "00".repeat(32),
                "runtime_bin_sha256": "11".repeat(32),
                "bootstrap_elf_sha256": "22".repeat(32)
            }],
            "messages_used": ["base/hello"]
        }),
    )
    .unwrap();
    let scenarios = vec![ScenarioResult::from_repetitions(
        String::from("icmp"),
        ImageClass::SystemWatchdog,
        1,
        vec![RepetitionResult {
            schema: RUN_SCHEMA,
            repetition: 1,
            outcome: Outcome::Passed,
            started_unix_millis: 100,
            duration_millis: 100,
            artifact_directory: PathBuf::from("scenarios/icmp/repetition-001"),
            attachments: vec![Attachment {
                path: artifact_path,
                media_type: String::from("text/plain"),
                size_bytes: 15,
                sha256: attachment_sha256,
            }],
            measurements: vec![Measurement::observed(
                "icmp.replies.received",
                1,
                MeasurementUnit::Count,
            )],
            failure: None,
        }],
    )];
    atomic_json(
        &run.join("suite.json"),
        &SuiteResult {
            schema: RUN_SCHEMA,
            run_id: String::from("run-1"),
            target: String::from("chip-a"),
            outcome: Outcome::Passed,
            started_unix_millis: 100,
            finished_unix_millis: 200,
            duration_millis: 100,
            counts: SuiteCounts::from_results(&scenarios),
            scenarios,
        },
    )
    .unwrap();
    fs::write(run.join("report.html"), b"generated report").unwrap();
    (root, run)
}

/// Archive `bytes` at `path` the way a bundle keeps a compressed file.
fn write_compressed(path: &Path, bytes: &[u8]) {
    let mut encoder =
        flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
    std::io::Write::write_all(&mut encoder, bytes).unwrap();
    fs::write(
        crate::format::archived::stored_path(path),
        encoder.finish().unwrap(),
    )
    .unwrap();
}

/// Archive the image's runtime and bootstrap files beside its application,
/// with their digests in the manifest.
fn add_firmware_subjects(run: &Path) {
    let runtime_elf_path = PathBuf::from("firmware/system-watchdog/runtime.elf");
    let runtime_bin_path = PathBuf::from("firmware/system-watchdog/runtime.bin");
    let bootstrap_elf_path = PathBuf::from("firmware/system-watchdog/bootstrap.elf");
    write_compressed(&run.join(&runtime_elf_path), b"runtime elf");
    fs::write(run.join(&runtime_bin_path), b"runtime bin").unwrap();
    fs::write(run.join(&bootstrap_elf_path), b"bootstrap elf").unwrap();
    let mut manifest: RunManifest = read_json(&run.join("manifest.json")).unwrap();
    let artifact = &mut manifest.firmware[0];
    artifact.runtime_elf_path = Some(runtime_elf_path);
    artifact.runtime_elf_size_bytes = Some(11);
    artifact.runtime_elf_sha256 = oer_os_durable::sha256_bytes(b"runtime elf");
    artifact.runtime_bin_path = Some(runtime_bin_path.clone());
    artifact.runtime_bin_size_bytes = Some(11);
    artifact.runtime_bin_sha256 = Some(sha256_file(&run.join(&runtime_bin_path)).unwrap());
    artifact.bootstrap_elf_path = Some(bootstrap_elf_path.clone());
    artifact.bootstrap_elf_size_bytes = Some(13);
    artifact.bootstrap_elf_sha256 = Some(sha256_file(&run.join(&bootstrap_elf_path)).unwrap());
    atomic_json(&run.join("manifest.json"), &manifest).unwrap();
}

fn add_lab_provenance(run: &Path, device_id: &str) {
    let mut manifest: RunManifest = read_json(&run.join("manifest.json")).unwrap();
    let path = PathBuf::from("lab-provenance.json");
    manifest.lab_provenance_path = Some(path.clone());
    atomic_json(&run.join("manifest.json"), &manifest).unwrap();
    atomic_json(
        &run.join(path),
        &LabProvenance {
            scope: crate::format::lab::ObservationScope::Network,
            schema: crate::format::lab::LAB_PROVENANCE_SCHEMA,
            captured_unix_millis: 150,
            definition: LabDefinition {
                bluetooth_adapter: None,
                peer: None,
                cell_id: String::from("cell-1"),
                device_id: device_id.to_owned(),
                station_ipv4: StationIpv4Definition::Dhcp,
                access_point: AccessPointDefinition {
                    channel: 6,
                    channel_width: crate::format::lab::ChannelWidth::Mhz40Above,
                    client_limit: 4,
                    target_address: "10.43.0.1".parse().unwrap(),
                    client_address: "10.43.0.2".parse().unwrap(),
                    secondary_client_address: None,
                },
                station_fixture: StationFixtureDefinition::External {
                    phys: vec![crate::format::lab::Phy::Ht40],
                },
                sensitive_network_values: SensitiveValueDisposition::Omitted,
            },
            host: HostObservation {
                kernel_release: String::from("6.12.0"),
                machine: String::from("x86_64"),
                os_release: Some(String::from("Test Linux")),
                boot_id: None,
                interfaces: vec![HostInterfaceObservation {
                    name: String::from("lo"),
                    operstate: String::from("unknown"),
                    mac_address: Some(String::from("00:00:00:00:00:00")),
                    master: None,
                    wireless: false,
                    ipv4_addresses: vec![String::from("127.0.0.1/8")],
                    wireless_link: None,
                }],
                ipv4_routes: Vec::new(),
            },
            fixture: FixtureObservation::External { managed: false },
        },
    )
    .unwrap();
}

#[test]
fn a_source_run_with_its_firmware_is_accepted() {
    let (root, _) = fixture();
    let manifest = validate(&root).unwrap();
    assert_eq!(manifest.run_id, "run-1");
    assert_eq!(manifest.firmware.len(), 1);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn lab_provenance_without_a_scope_or_with_the_old_peer_key_is_rejected() {
    let (root, run) = fixture();
    add_lab_provenance(&run, "dut-1");
    let path = run.join("lab-provenance.json");
    let current: serde_json::Value = read_json(&path).unwrap();
    let mut unscoped = current.clone();
    unscoped.as_object_mut().unwrap().remove("scope");
    let mut renamed = current;
    let definition = renamed["definition"].as_object_mut().unwrap();
    let peer = definition.remove("peer").unwrap();
    definition.insert("ieee802154_peer".into(), peer);
    for document in [unscoped, renamed] {
        assert!(serde_json::from_value::<LabProvenance>(document).is_err());
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn verifies_typed_lab_provenance_and_rejects_wrong_device_binding() {
    let (root, run) = fixture();
    add_lab_provenance(&run, "dut-1");
    validate(&root).unwrap();

    add_lab_provenance(&run, "another-dut");
    let error = validate(&root).expect_err("reject lab snapshot for another device");
    assert!(error.to_string().contains("not bound"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn system_provenance_requires_a_matching_scenario_snapshot() {
    let (root, run) = fixture();
    add_lab_provenance(&run, "dut-1");
    let mut provenance: LabProvenance = read_json(&run.join("lab-provenance.json")).unwrap();
    provenance.scope = crate::format::lab::ObservationScope::System;
    provenance.fixture = FixtureObservation::NotUsed;
    atomic_json(&run.join("lab-provenance.json"), &provenance).unwrap();
    let mut scenario = oer_hil_scenario::test_family::scenario(include_str!(
        "../../../scenarios/system/boot-smoke.toml"
    ));
    let snapshot = run
        .join("scenarios")
        .join(scenario.id())
        .join("scenario.json");
    fs::create_dir_all(snapshot.parent().unwrap()).unwrap();
    atomic_json(&snapshot, &scenario).unwrap();
    let plan = crate::format::run::RunPlan {
        schema: RUN_SCHEMA,
        run_id: "run-1".into(),
        selection: "boot-smoke".into(),
        firmware: None,
        entries: vec![crate::format::run::PlanEntry {
            scenario: scenario.id().to_owned(),
            image: scenario.image(),
            repetitions: scenario.repetitions(),
            disposition: crate::format::run::PlanDisposition::Selected,
            reason: None,
            requirements: Some(scenario.requirements()),
        }],
    };
    atomic_json(&run.join("plan.json"), &plan).unwrap();
    let manifest: RunManifest = read_json(&run.join("manifest.json")).unwrap();
    validate_lab_provenance(&run, &manifest).unwrap();
    scenario.header.repetitions += 1;
    atomic_json(&snapshot, &scenario).unwrap();
    assert!(
        validate_lab_provenance(&run, &manifest)
            .unwrap_err()
            .to_string()
            .contains("disagrees")
    );
    fs::remove_file(snapshot).unwrap();
    assert!(validate_lab_provenance(&run, &manifest).is_err());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_non_network_plan_accepts_system_provenance_with_a_peer_requirement() {
    let (root, run) = fixture();
    add_lab_provenance(&run, "dut-1");
    let mut provenance: LabProvenance = read_json(&run.join("lab-provenance.json")).unwrap();
    provenance.scope = crate::format::lab::ObservationScope::System;
    provenance.fixture = FixtureObservation::NotUsed;
    provenance.definition.bluetooth_adapter = Some("hci0".into());
    atomic_json(&run.join("lab-provenance.json"), &provenance).unwrap();
    let scenario = oer_hil_scenario::test_family::scenario(include_str!(
        "../../../scenarios/system/boot-smoke.toml"
    ));
    let directory = run.join("scenarios").join(scenario.id());
    fs::create_dir_all(&directory).unwrap();
    atomic_json(&directory.join("scenario.json"), &scenario).unwrap();
    let plan = crate::format::run::RunPlan {
        schema: RUN_SCHEMA,
        run_id: "run-1".into(),
        selection: scenario.id().to_owned(),
        firmware: None,
        entries: vec![crate::format::run::PlanEntry {
            scenario: scenario.id().to_owned(),
            image: scenario.image(),
            repetitions: scenario.repetitions(),
            disposition: crate::format::run::PlanDisposition::Selected,
            reason: None,
            requirements: Some(oer_hil_model::catalog::requirements::Requirements {
                peer: true,
                ..Default::default()
            }),
        }],
    };
    atomic_json(&run.join("plan.json"), &plan).unwrap();
    let manifest: RunManifest = read_json(&run.join("manifest.json")).unwrap();
    validate_lab_provenance(&run, &manifest).unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn selects_only_verified_archived_firmware_for_replay() {
    let (root, run) = fixture();
    let firmware = archived_firmware(&root, "chip-a", "run-1", ImageClass::SystemWatchdog)
        .expect("select archived firmware");
    assert_eq!(
        firmware.application_path,
        run.join("firmware/system-watchdog/application.bin")
    );
    assert_eq!(firmware.application_sha256.len(), 64);
    let error = archived_firmware(&root, "chip-a", "run-1", ImageClass::SystemPanicReset)
        .expect_err("reject absent image class");
    assert!(
        error
            .to_string()
            .contains("no archived `system-panic-reset` firmware")
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn verifies_every_archived_firmware_subject() {
    let (root, run) = fixture();
    add_firmware_subjects(&run);
    assert_eq!(validate(&root).unwrap().firmware.len(), 1);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn rejects_tampered_archived_runtime_elf() {
    let (root, run) = fixture();
    add_firmware_subjects(&run);
    write_compressed(
        &run.join("firmware/system-watchdog/runtime.elf"),
        b"runtime elF",
    );
    let error = validate(&root).unwrap_err();
    assert!(error.to_string().contains("SHA-256"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn rejects_a_replay_origin_naming_the_run_itself() {
    let (root, run) = fixture();
    let mut manifest: RunManifest = read_json(&run.join("manifest.json")).unwrap();
    manifest.firmware[0].replayed_from = Some(crate::format::run::FirmwareReplayOrigin {
        source_run_id: String::from("run-1"),
    });
    atomic_json(&run.join("manifest.json"), &manifest).unwrap();
    let error = validate(&root).unwrap_err();
    assert!(error.to_string().contains("inconsistent replay origin"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn the_runs_link_to_the_shared_store_is_followed() {
    let directory = tempfile::tempdir().unwrap();
    let store = directory.path().join("store/runs");
    fs::create_dir_all(&store).unwrap();
    let target = directory.path().join("checkout/target/hil");
    fs::create_dir_all(&target).unwrap();
    std::os::unix::fs::symlink(&store, target.join("runs")).unwrap();
    assert_eq!(
        runs_directory(&target.join("runs")).unwrap(),
        fs::canonicalize(&store).unwrap()
    );
    let plain = directory.path().join("plain");
    fs::create_dir_all(plain.join("runs")).unwrap();
    assert_eq!(
        runs_directory(&plain.join("runs")).unwrap(),
        plain.join("runs")
    );
}
