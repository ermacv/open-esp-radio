use std::{
    fs,
    path::{Path, PathBuf},
};

use oer_hil_run_bundle::run::test_support::TEST_CHIP;
use oer_hil_run_bundle::run::test_support::session;
use oer_hil_run_bundle::run::test_support::session_for;
use oer_hil_run_bundle::run::test_support::temporary_directory;
use oer_hil_run_bundle::run::test_support::write_test_build_materials;
use oer_hil_run_bundle_format::run::inventory;
use oer_hil_schema::image::ImageClass;

use super::firmware;
use crate::Artifacts;
use oer_image_build::bundle::{Staged, files};

/// The Rust target of the fixture images.
const TARGET: &str = "riscv32imafc-unknown-none-elf";

/// Bluetooth GATT artifacts: a staged bundle beside `application`
/// holding the given files and a host-independent environment.
fn test_artifacts(
    application: &Path,
    runtime_elf: &Path,
    runtime_bin: &Path,
    bootstrap_elf: &Path,
) -> Artifacts {
    let directory = application.parent().unwrap().join("bundle");
    fs::create_dir_all(&directory).unwrap();
    let profile: oer_image_profile::Profile =
        toml::from_str(oer_hil_run_bundle::run::test_support::TEST_CHIP_PROFILE).unwrap();
    let mut bundle = oer_image_build::bundle::ImageBundle::new(
        &directory,
        &profile,
        profile.flash.clone().unwrap(),
    );
    for (source, name) in [
        (application, files::APPLICATION),
        (runtime_elf, files::RUNTIME_ELF),
        (runtime_bin, files::RUNTIME_BIN),
        (bootstrap_elf, files::BOOTSTRAP_ELF),
    ] {
        fs::copy(source, directory.join(name)).unwrap();
    }
    fs::write(directory.join(files::BOOTLOADER), b"bootloader").unwrap();
    fs::write(directory.join(files::PARTITIONS), b"partitions").unwrap();
    fs::write(
        directory.join(files::SOURCE_INPUTS),
        r#"{"schema":2,"files":[]}"#,
    )
    .unwrap();
    bundle.staged = Some(Staged {
        bootstrap_package: "oer-chip-a-platform-bootstrap".into(),
    });
    assert_eq!(bundle.rust_target, TARGET);
    Artifacts {
        bundle,
        features: oer_hil_schema::image::FeatureDelta::default(),
        environment: oer_hil_run_bundle::build::synthetic_environment(),
    }
}

#[test]
fn firmware_record_archives_the_exact_application() {
    let root = temporary_directory("firmware");
    write_test_build_materials(&root);
    let run_directory = root.join("run");
    fs::create_dir(&run_directory).unwrap();
    let application = root.join("application.bin");
    let runtime_elf = root.join("runtime.elf");
    let runtime_bin = root.join("runtime.bin");
    let bootstrap_elf = root.join("bootstrap.elf");
    fs::write(&application, b"application bytes").unwrap();
    fs::write(&runtime_elf, b"runtime elf").unwrap();
    fs::write(&runtime_bin, b"runtime bin").unwrap();
    fs::write(&bootstrap_elf, b"bootstrap elf").unwrap();

    let mut first_session = session(&run_directory);
    let (_snapshot_root, snapshot) = oer_hil_source_snapshot::test_snapshot(&root);
    let slots = root.join("slots/source-build");
    first_session
        .bind_source_snapshot(
            snapshot.directory(),
            &oer_hil_source_snapshot::test_objects_of(&snapshot),
            &slots,
        )
        .unwrap();
    firmware::record(
        &mut first_session,
        ImageClass::SystemWatchdog,
        &test_artifacts(&application, &runtime_elf, &runtime_bin, &bootstrap_elf),
    )
    .unwrap();
    let artifact = &first_session.manifest().firmware[0];
    assert_eq!(
        artifact.application_path,
        PathBuf::from("firmware/system-watchdog/application.bin")
    );
    assert_eq!(artifact.application_size_bytes, 17);
    assert_eq!(
        fs::read(run_directory.join(&artifact.application_path)).unwrap(),
        b"application bytes"
    );
    assert_eq!(artifact.application_sha256.len(), 64);
    // The runtime ELF is kept compressed; its one reader returns it whole.
    let archived_elf = run_directory.join(
        artifact
            .runtime_elf_path
            .as_ref()
            .expect("runtime ELF path"),
    );
    assert!(!archived_elf.exists());
    assert_eq!(
        oer_hil_run_bundle_format::archived::read_archived(&archived_elf).unwrap(),
        b"runtime elf"
    );
    assert_eq!(artifact.runtime_elf_size_bytes, Some(11));
    assert_eq!(
        fs::read(
            run_directory.join(
                artifact
                    .runtime_bin_path
                    .as_ref()
                    .expect("runtime bin path")
            )
        )
        .unwrap(),
        b"runtime bin"
    );
    assert_eq!(
        fs::read(
            run_directory.join(
                artifact
                    .bootstrap_elf_path
                    .as_ref()
                    .expect("bootstrap ELF path")
            )
        )
        .unwrap(),
        b"bootstrap elf"
    );
    let provenance_path = artifact
        .build_provenance_path
        .as_ref()
        .expect("build provenance path");
    let provenance: oer_hil_run_bundle_format::build::BuildProvenance =
        serde_json::from_slice(&fs::read(run_directory.join(provenance_path)).unwrap()).unwrap();
    assert_eq!(provenance.build_id, artifact.build_id.clone().unwrap());
    assert_eq!(provenance.subjects.len(), 4);
    // The fixture chip has no HIL agent: its record follows the class's
    // own recipe on it, which links no network.
    assert_eq!(
        provenance.parameters.network.as_deref(),
        oer_hil_image_class::network_on(ImageClass::SystemWatchdog, TEST_CHIP)
    );
    assert_eq!(
        provenance.parameters.runtime_features,
        oer_hil_image_class::build_features_on(ImageClass::SystemWatchdog, TEST_CHIP)
    );
    // One workspace lock: the agent and the bootstrap resolve against it.
    for name in ["embedded-lock"] {
        assert!(provenance.files.iter().any(|file| file.name == name));
    }
    assert!(provenance.source_reconstructable);
    let object_root = root.join("objects/sha256");
    let first_objects = inventory(&object_root).unwrap();
    // The application, both ELFs, the runtime binary, the lock, the build
    // provenance, the source inputs and the snapshot's record and manifest
    // (its files are source objects of their own).
    assert_eq!(first_objects.len(), 8);
    assert!(
        firmware::record(
            &mut first_session,
            ImageClass::SystemWatchdog,
            &test_artifacts(&application, &runtime_elf, &runtime_bin, &bootstrap_elf,),
        )
        .is_err()
    );
    assert_eq!(inventory(&object_root).unwrap(), first_objects);
    drop(first_session);

    let second_run_directory = root.join("run-2");
    fs::create_dir(&second_run_directory).unwrap();
    let mut second = session(&second_run_directory);
    second
        .bind_source_snapshot(
            snapshot.directory(),
            &oer_hil_source_snapshot::test_objects_of(&snapshot),
            &slots,
        )
        .unwrap();
    firmware::record(
        &mut second,
        ImageClass::SystemWatchdog,
        &test_artifacts(&application, &runtime_elf, &runtime_bin, &bootstrap_elf),
    )
    .unwrap();
    assert_eq!(inventory(&object_root).unwrap(), first_objects);
    drop(second);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn replayed_firmware_bundle_is_self_contained_after_origin_removal() {
    let root = temporary_directory("firmware-replay");
    let target_directory = root.join("target/hil/chip-a");
    let runs_directory = root.join(oer_hil_run_bundle::store::CHECKOUT_RUNS);
    let repository_root = target_directory.clone();
    fs::create_dir_all(&runs_directory).unwrap();
    write_test_build_materials(&repository_root);

    let application = repository_root.join("application.bin");
    let runtime_elf = repository_root.join("runtime.elf");
    let runtime_bin = repository_root.join("runtime.bin");
    let bootstrap_elf = repository_root.join("bootstrap.elf");
    fs::write(&application, b"application bytes").unwrap();
    fs::write(&runtime_elf, b"runtime elf").unwrap();
    fs::write(&runtime_bin, b"runtime bin").unwrap();
    fs::write(&bootstrap_elf, b"bootstrap elf").unwrap();

    let source_directory = runs_directory.join("source-run");
    fs::create_dir(&source_directory).unwrap();
    let mut source = session_for(&source_directory, &target_directory, &repository_root);
    let slots = root.join("slots/source-build");
    // The runs' store holds the source objects, as the shared store does for
    // every capture and run.
    let objects = runs_directory
        .parent()
        .unwrap()
        .join(oer_hil_schema::snapshot::OBJECTS);
    let (_snapshot_root, snapshot) =
        oer_hil_source_snapshot::test_snapshot_into(&repository_root, Some(&objects));
    source
        .bind_source_snapshot(snapshot.directory(), &objects, &slots)
        .unwrap();
    firmware::record(
        &mut source,
        ImageClass::SystemWatchdog,
        &test_artifacts(&application, &runtime_elf, &runtime_bin, &bootstrap_elf),
    )
    .unwrap();
    source
        .finish(Vec::new(), |_, _| Default::default())
        .unwrap();

    let archived = oer_hil_run_bundle::replay::archived_firmware(
        &root,
        "chip-a",
        "source-run",
        ImageClass::SystemWatchdog,
        &oer_hil_run_bundle::run::test_support::TestRecipe,
    )
    .unwrap();
    let replay_directory = runs_directory.join("replay-run");
    fs::create_dir(&replay_directory).unwrap();
    let mut replay = session_for(&replay_directory, &target_directory, &repository_root);
    let replayed_application = replay.record_replayed_firmware(&archived).unwrap();
    assert_eq!(
        fs::read(&replayed_application).unwrap(),
        b"application bytes"
    );
    assert_eq!(replay.manifest().firmware.len(), 1);
    assert_eq!(
        replay.manifest().firmware[0]
            .replayed_from
            .as_ref()
            .expect("replay origin")
            .source_run_id,
        "source-run"
    );
    replay
        .finish(Vec::new(), |_, _| Default::default())
        .unwrap();

    fs::remove_dir_all(source_directory).unwrap();
    // The replay run stands on its own: its firmware replays again after
    // the source run is gone.
    let again = oer_hil_run_bundle::replay::archived_firmware(
        &root,
        "chip-a",
        "replay-run",
        ImageClass::SystemWatchdog,
        &oer_hil_run_bundle::run::test_support::TestRecipe,
    )
    .unwrap();
    assert_eq!(again.run_id, "replay-run");
    fs::remove_dir_all(root).unwrap();
}
