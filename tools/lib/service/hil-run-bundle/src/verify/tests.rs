use super::*;
use crate::run::test_support::TestRecipe;
use oer_tool_base_durable::atomic_json;
use oer_tool_data_hil_run_bundle_format::lab::AccessPointDefinition;
use oer_tool_data_hil_run_bundle_format::lab::FixtureObservation;
use oer_tool_data_hil_run_bundle_format::lab::HostInterfaceObservation;
use oer_tool_data_hil_run_bundle_format::lab::HostObservation;
use oer_tool_data_hil_run_bundle_format::lab::LabDefinition;
use oer_tool_data_hil_run_bundle_format::lab::LabProvenance;
use oer_tool_data_hil_run_bundle_format::lab::SensitiveValueDisposition;
use oer_tool_data_hil_run_bundle_format::lab::StationFixtureDefinition;
use oer_tool_data_hil_run_bundle_format::lab::StationIpv4Definition;
use oer_tool_data_hil_run_bundle_format::run::Attachment;
use oer_tool_data_hil_run_bundle_format::run::Measurement;
use oer_tool_data_hil_run_bundle_format::run::MeasurementUnit;
use oer_tool_data_hil_run_bundle_format::run::Outcome;
use oer_tool_data_hil_run_bundle_format::run::RepetitionResult;
use oer_tool_data_hil_run_bundle_format::run::ScenarioResult;
use oer_tool_data_hil_run_bundle_format::run::SuiteCounts;
use oer_tool_data_hil_run_bundle_format::run::write_integrity_index;
use oer_tool_data_hil_schema::image::ImageClass;
use std::sync::atomic::{AtomicU64, Ordering};

static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

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
                "dirty": false,
                "workspace_sha256": "00"
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
    write_integrity_index(&run, "run-1").unwrap();
    (root, run)
}

/// Archive `bytes` at `path` the way a bundle keeps a compressed file.
fn write_compressed(path: &Path, bytes: &[u8]) {
    let mut encoder =
        flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
    std::io::Write::write_all(&mut encoder, bytes).unwrap();
    fs::write(
        oer_tool_data_hil_run_bundle_format::archived::stored_path(path),
        encoder.finish().unwrap(),
    )
    .unwrap();
}

fn add_build_provenance(run: &Path) {
    let runtime_elf_path = PathBuf::from("firmware/system-watchdog/runtime.elf");
    let runtime_bin_path = PathBuf::from("firmware/system-watchdog/runtime.bin");
    let bootstrap_elf_path = PathBuf::from("firmware/system-watchdog/bootstrap.elf");
    let effective_lock_path = PathBuf::from("firmware/system-watchdog/effective-Cargo.lock");
    write_compressed(&run.join(&runtime_elf_path), b"runtime elf");
    fs::write(run.join(&runtime_bin_path), b"runtime bin").unwrap();
    fs::write(run.join(&bootstrap_elf_path), b"bootstrap elf").unwrap();
    fs::write(run.join(&effective_lock_path), b"effective lock").unwrap();
    let mut manifest: RunManifest = read_json(&run.join("manifest.json")).unwrap();
    manifest.repository.workspace_sha256 = "00".repeat(32);
    let artifact = &mut manifest.firmware[0];
    artifact.runtime_elf_path = Some(runtime_elf_path.clone());
    artifact.runtime_elf_size_bytes = Some(11);
    artifact.runtime_elf_sha256 = oer_tool_base_durable::sha256_bytes(b"runtime elf");
    artifact.runtime_bin_path = Some(runtime_bin_path.clone());
    artifact.runtime_bin_size_bytes = Some(11);
    artifact.runtime_bin_sha256 = Some(sha256_file(&run.join(&runtime_bin_path)).unwrap());
    artifact.bootstrap_elf_path = Some(bootstrap_elf_path.clone());
    artifact.bootstrap_elf_size_bytes = Some(13);
    artifact.bootstrap_elf_sha256 = Some(sha256_file(&run.join(&bootstrap_elf_path)).unwrap());
    let subjects = vec![
        BuildSubject {
            role: BuildSubjectRole::Application,
            path: artifact.application_path.clone(),
            size_bytes: artifact.application_size_bytes,
            sha256: artifact.application_sha256.clone(),
        },
        BuildSubject {
            role: BuildSubjectRole::BootstrapElf,
            path: bootstrap_elf_path,
            size_bytes: 13,
            sha256: artifact.bootstrap_elf_sha256.clone().unwrap(),
        },
        BuildSubject {
            role: BuildSubjectRole::RuntimeBin,
            path: runtime_bin_path,
            size_bytes: 11,
            sha256: artifact.runtime_bin_sha256.clone().unwrap(),
        },
        BuildSubject {
            role: BuildSubjectRole::RuntimeElf,
            path: runtime_elf_path,
            size_bytes: 11,
            sha256: artifact.runtime_elf_sha256.clone(),
        },
    ];
    let build_id = crate::build::build_id(&subjects);
    let provenance_path = PathBuf::from("firmware/system-watchdog/build-provenance.json");
    artifact.build_id = Some(build_id.clone());
    artifact.build_provenance_path = Some(provenance_path.clone());
    atomic_json(&run.join("manifest.json"), &manifest).unwrap();
    atomic_json(
        &run.join(&provenance_path),
        &BuildProvenance {
            schema: BUILD_PROVENANCE_SCHEMA,
            build_id,
            build_type: String::from("open-esp-radio-hil-firmware/v1"),
            parameters: oer_tool_data_hil_run_bundle_format::build::BuildParameters {
                network: None,
                image: ImageClass::SystemWatchdog,
                runtime_profile: ImageClass::SystemWatchdog.runtime_profile().to_owned(),
                target: String::from("riscv32imafc-unknown-none-elf"),
                runtime_features: ImageClass::SystemWatchdog.runtime_features().to_owned(),
                layout_seed: None,
                features: oer_tool_data_hil_schema::image::FeatureDelta::default(),
            },
            sources: vec![SourceMaterial {
                name: String::from("repository"),
                checkout_path: PathBuf::from("/build/source"),
                remote: Some(String::from("https://example.invalid/repository.git")),
                commit: manifest.repository.commit.clone(),
                dirty: manifest.repository.dirty,
                workspace_sha256: manifest.repository.workspace_sha256.clone(),
                rebuild_status: SourceRebuildStatus::CleanCommit,
                tracked_patch_path: None,
                tracked_patch_size_bytes: None,
                tracked_patch_sha256: None,
                untracked_files: Vec::new(),
                limitations: Vec::new(),
            }],
            files: vec![
                oer_tool_data_hil_run_bundle_format::build::BuildFileMaterial {
                    name: String::from("embedded-lock"),
                    path: PathBuf::from("Cargo.lock"),
                    archive_path: Some(effective_lock_path.clone()),
                    size_bytes: 14,
                    sha256: sha256_file(&run.join(&effective_lock_path)).unwrap(),
                },
            ],
            environment: oer_tool_data_hil_run_bundle_format::build::BuildEnvironment {
                tools: Vec::new(),
                inherited_rustflags: None,
                inherited_encoded_rustflags: None,
                cargo_incremental: String::from("0"),
                source_date_epoch: None,
            },
            subjects,
            source_reconstructable: true,
            reproducibility:
                oer_tool_data_hil_run_bundle_format::build::BuildReproducibility::Unverified,
        },
    )
    .unwrap();
    write_integrity_index(run, "run-1").unwrap();
}

fn add_lab_provenance(run: &Path, device_id: &str) {
    let mut manifest: RunManifest = read_json(&run.join("manifest.json")).unwrap();
    let path = PathBuf::from("lab-provenance.json");
    manifest.lab_provenance_path = Some(path.clone());
    atomic_json(&run.join("manifest.json"), &manifest).unwrap();
    atomic_json(
        &run.join(path),
        &LabProvenance {
            scope: oer_tool_data_hil_run_bundle_format::lab::ObservationScope::Network,
            schema: oer_tool_data_hil_run_bundle_format::lab::LAB_PROVENANCE_SCHEMA,
            captured_unix_millis: 150,
            definition: LabDefinition {
                bluetooth_adapter: None,
                peer: None,
                cell_id: String::from("cell-1"),
                device_id: device_id.to_owned(),
                station_ipv4: StationIpv4Definition::Dhcp,
                access_point: AccessPointDefinition {
                    channel: 6,
                    channel_width:
                        oer_tool_data_hil_run_bundle_format::lab::ChannelWidth::Mhz40Above,
                    client_limit: 4,
                    target_address: "10.43.0.1".parse().unwrap(),
                    client_address: "10.43.0.2".parse().unwrap(),
                    secondary_client_address: None,
                },
                station_fixture: StationFixtureDefinition::External {
                    phys: vec![oer_tool_data_hil_run_bundle_format::lab::Phy::Ht40],
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
    write_integrity_index(run, "run-1").unwrap();
}

#[test]
fn verifies_firmware_and_attachment_content() {
    let (root, _) = fixture();
    let completion = verify(&root, Some("chip-a"), None, &TestRecipe).unwrap();
    assert_eq!(completion.status, "verified");
    assert_eq!(completion.runs, 1);
    assert_eq!(completion.attachments, 1);
    assert_eq!(completion.firmware_artifacts, 1);
    assert_eq!(completion.verified_run_ids, ["run-1"]);
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
    verify(&root, Some("chip-a"), Some("run-1"), &TestRecipe).unwrap();

    add_lab_provenance(&run, "another-dut");
    let error = verify(&root, Some("chip-a"), Some("run-1"), &TestRecipe)
        .expect_err("reject lab snapshot for another device");
    assert!(error.to_string().contains("not bound"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn system_provenance_requires_a_matching_scenario_snapshot() {
    let (root, run) = fixture();
    add_lab_provenance(&run, "dut-1");
    let mut provenance: LabProvenance = read_json(&run.join("lab-provenance.json")).unwrap();
    provenance.scope = oer_tool_data_hil_run_bundle_format::lab::ObservationScope::System;
    provenance.fixture = FixtureObservation::NotUsed;
    atomic_json(&run.join("lab-provenance.json"), &provenance).unwrap();
    let mut scenario = oer_tool_service_hil_scenario::test_family::scenario(include_str!(
        "../../../../data/hil-scenarios/system/boot-smoke.toml"
    ));
    let snapshot = run
        .join("scenarios")
        .join(scenario.id())
        .join("scenario.json");
    fs::create_dir_all(snapshot.parent().unwrap()).unwrap();
    atomic_json(&snapshot, &scenario).unwrap();
    let plan = oer_tool_data_hil_run_bundle_format::run::RunPlan {
        schema: RUN_SCHEMA,
        run_id: "run-1".into(),
        selection: "boot-smoke".into(),
        firmware: None,
        entries: vec![oer_tool_data_hil_run_bundle_format::run::PlanEntry {
            scenario: scenario.id().to_owned(),
            image: scenario.image(),
            repetitions: scenario.repetitions(),
            disposition: oer_tool_data_hil_run_bundle_format::run::PlanDisposition::Selected,
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
    provenance.scope = oer_tool_data_hil_run_bundle_format::lab::ObservationScope::System;
    provenance.fixture = FixtureObservation::NotUsed;
    provenance.definition.bluetooth_adapter = Some("hci0".into());
    atomic_json(&run.join("lab-provenance.json"), &provenance).unwrap();
    let scenario = oer_tool_service_hil_scenario::test_family::scenario(include_str!(
        "../../../../data/hil-scenarios/system/boot-smoke.toml"
    ));
    let directory = run.join("scenarios").join(scenario.id());
    fs::create_dir_all(&directory).unwrap();
    atomic_json(&directory.join("scenario.json"), &scenario).unwrap();
    let plan = oer_tool_data_hil_run_bundle_format::run::RunPlan {
        schema: RUN_SCHEMA,
        run_id: "run-1".into(),
        selection: scenario.id().to_owned(),
        firmware: None,
        entries: vec![oer_tool_data_hil_run_bundle_format::run::PlanEntry {
            scenario: scenario.id().to_owned(),
            image: scenario.image(),
            repetitions: scenario.repetitions(),
            disposition: oer_tool_data_hil_run_bundle_format::run::PlanDisposition::Selected,
            reason: None,
            requirements: Some(
                oer_tool_data_hil_scenario_catalog::requirements::Requirements {
                    peer: true,
                    ..Default::default()
                },
            ),
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
    let firmware = archived_firmware(
        &root,
        "chip-a",
        "run-1",
        ImageClass::SystemWatchdog,
        &TestRecipe,
    )
    .expect("select archived firmware");
    assert_eq!(
        firmware.application_path,
        run.join("firmware/system-watchdog/application.bin")
    );
    assert_eq!(firmware.application_sha256.len(), 64);
    let error = archived_firmware(
        &root,
        "chip-a",
        "run-1",
        ImageClass::SystemPanicReset,
        &TestRecipe,
    )
    .expect_err("reject absent image class");
    assert!(
        error
            .to_string()
            .contains("no archived `system-panic-reset` firmware")
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn verifies_complete_build_provenance_and_all_firmware_subjects() {
    let (root, run) = fixture();
    add_build_provenance(&run);
    let completion = verify(&root, Some("chip-a"), Some("run-1"), &TestRecipe).unwrap();
    assert_eq!(completion.firmware_artifacts, 1);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_manifest_seed_must_be_the_build_records_seed() {
    let (root, run) = fixture();
    add_build_provenance(&run);
    let mut manifest: RunManifest = read_json(&run.join("manifest.json")).unwrap();
    manifest.firmware[0].layout_seed = std::num::NonZeroU32::new(7);
    atomic_json(&run.join("manifest.json"), &manifest).unwrap();
    write_integrity_index(&run, "run-1").unwrap();
    let error = verify(&root, Some("chip-a"), Some("run-1"), &TestRecipe).unwrap_err();
    assert!(
        error.to_string().contains("build provenance inconsistent"),
        "{error}"
    );
    // Unseeded, the manifest keeps its earlier form.
    manifest.firmware[0].layout_seed = None;
    let json = serde_json::to_value(&manifest).unwrap();
    assert!(json["firmware"][0].get("layout_seed").is_none());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn rejects_tampered_archived_runtime_elf() {
    let (root, run) = fixture();
    add_build_provenance(&run);
    write_compressed(
        &run.join("firmware/system-watchdog/runtime.elf"),
        b"runtime elF",
    );
    let error = verify(&root, Some("chip-a"), Some("run-1"), &TestRecipe).unwrap_err();
    assert!(error.to_string().contains("SHA-256"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn rejects_replay_origin_not_bound_to_the_firmware_build() {
    let (root, run) = fixture();
    add_build_provenance(&run);
    let mut manifest: RunManifest = read_json(&run.join("manifest.json")).unwrap();
    let repository = manifest.repository.clone();
    manifest.firmware[0].replayed_from = Some(
        oer_tool_data_hil_run_bundle_format::run::FirmwareReplayOrigin {
            source_run_id: String::from("source-run"),
            source_integrity_sha256: "33".repeat(32),
            firmware_repository: repository,
            source_build_id: Some(String::from("wrong-build-id")),
        },
    );
    atomic_json(&run.join("manifest.json"), &manifest).unwrap();
    write_integrity_index(&run, "run-1").unwrap();
    let error = verify(&root, Some("chip-a"), Some("run-1"), &TestRecipe).unwrap_err();
    assert!(error.to_string().contains("inconsistent replay origin"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn rejects_tampered_attachment() {
    let (root, run) = fixture();
    fs::write(
        run.join("scenarios/icmp/repetition-001/evidence.log"),
        b"serial evidencE",
    )
    .unwrap();
    let error = verify(&root, Some("chip-a"), Some("run-1"), &TestRecipe).unwrap_err();
    assert!(error.to_string().contains("SHA-256"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn rejects_paths_escaping_the_run_bundle() {
    let (root, run) = fixture();
    let mut suite: SuiteResult = read_json(&run.join("suite.json")).unwrap();
    suite.scenarios[0].repetitions[0].attachments[0].path = PathBuf::from("../outside");
    atomic_json(&run.join("suite.json"), &suite).unwrap();
    let error = verify(&root, Some("chip-a"), None, &TestRecipe).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("invalid or duplicate attachment")
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn rejects_unindexed_files() {
    let (root, run) = fixture();
    fs::write(run.join("injected.log"), b"not sealed").unwrap();
    let error = verify(&root, Some("chip-a"), None, &TestRecipe).unwrap_err();
    assert!(error.to_string().contains("sealed file inventory"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn rejects_tampered_derived_report() {
    let (root, run) = fixture();
    fs::write(run.join("report.html"), b"tampered report").unwrap();
    let error = verify(&root, Some("chip-a"), None, &TestRecipe).unwrap_err();
    assert!(error.to_string().contains("sealed file inventory"));
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

#[test]
fn a_referenced_observer_build_must_be_stored_under_its_digest() {
    use oer_tool_data_hil_run_bundle_format::observer::store as observer_store;
    let (root, run) = fixture();
    let store = root.join("target/hil");
    let build = serde_json::json!({"schema": 2, "resolved": {"nodes": []}});
    let embedded = serde_json::json!({
        "schema": observer_store::EMBEDDED,
        "executable_sha256": "e".repeat(64),
        "build_sha256": oer_tool_base_durable::sha256_bytes(&serde_json::to_vec(&build).unwrap()),
        "build": build,
    });
    let reference = observer_store::detach(&embedded, &store).unwrap();
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(run.join("manifest.json")).unwrap()).unwrap();
    manifest["runner"]["observer"] = embedded;
    atomic_json(&run.join("manifest.json"), &manifest).unwrap();
    write_integrity_index(&run, "run-1").unwrap();
    assert!(
        verify(&root, Some("chip-a"), None, &TestRecipe).is_err(),
        "a manifest embedding its observer build"
    );
    manifest["runner"]["observer"] = reference.clone();
    atomic_json(&run.join("manifest.json"), &manifest).unwrap();
    write_integrity_index(&run, "run-1").unwrap();
    verify(&root, Some("chip-a"), None, &TestRecipe).unwrap();

    let file =
        observer_store::path(&store, observer_store::build_digest(&reference).unwrap()).unwrap();
    let stored = fs::read(&file).unwrap();
    fs::remove_file(&file).unwrap();
    assert!(
        verify(&root, Some("chip-a"), None, &TestRecipe).is_err(),
        "a missing build"
    );
    fs::write(&file, stored).unwrap();
    fs::write(
        store
            .join("observers")
            .join(format!("{}.json", "0".repeat(64))),
        b"{}",
    )
    .unwrap();
    assert!(
        verify(&root, Some("chip-a"), None, &TestRecipe).is_err(),
        "a build that does not hash to its name"
    );
    fs::remove_dir_all(root).unwrap();
}
