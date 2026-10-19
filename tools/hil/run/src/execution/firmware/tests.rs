use std::{cell::RefCell, ffi::OsString, fs, path::PathBuf};

use super::*;

fn write(path: &Path, bytes: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

fn build_inputs(root: &Path) -> Artifacts {
    for relative in [
        "Cargo.lock",
        "Cargo.lock",
        "firmware/hil/chip-a/Cargo.toml",
        "firmware/hil/chip-a/stack.toml",
        "portable/chip-a/platform/stack.toml",
        "Cargo.lock",
        "portable/chip-a/platform/partitions/applications.csv",
    ] {
        write(&root.join(relative), relative.as_bytes());
    }
    // The fixture chip's profile and the HIL policy's base, which the build
    // provenance reads.
    write(
        &root.join("portable/chip-a/platform/chip.toml"),
        oer_hil_runs::run::test_support::TEST_CHIP_PROFILE.as_bytes(),
    );
    write(
        &root.join("firmware/hil/chip-a/stack.toml"),
        b"extends = \"../../../portable/chip-a/platform/stack.toml\"\n",
    );
    let build = root.join("build");
    for (name, bytes) in [
        ("application.bin", b"exact application".as_slice()),
        ("runtime.elf", b"runtime elf".as_slice()),
        ("runtime.bin", b"runtime bin".as_slice()),
        ("bootstrap.elf", b"bootstrap elf".as_slice()),
        ("bootloader.bin", b"bootloader".as_slice()),
        ("partitions.bin", b"partitions".as_slice()),
        ("otadata.bin", b"otadata".as_slice()),
    ] {
        write(&build.join(name), bytes);
    }
    let profile: oer_chip_profile::Profile =
        toml::from_str(oer_hil_runs::run::test_support::TEST_CHIP_PROFILE).unwrap();
    let mut bundle =
        oer_image_build::bundle::ImageBundle::new(&build, &profile, profile.flash.clone().unwrap());
    bundle.staged = Some(oer_image_build::bundle::Staged {
        bootstrap_package: "oer-chip-a-platform-bootstrap".into(),
    });
    bundle.otadata = true;
    Artifacts {
        bundle,
        features: oer_hil_model::schema::image::FeatureDelta::default(),
        repository: oer_hil_runs::format::run::RepositoryProvenance {
            commit: String::from("abc"),
            dirty: false,
        },
    }
}

fn session(root: &Path) -> RunSession {
    crate::tests::register();
    RunSession::create(
        root,
        "chip-a",
        "test-cell",
        "test-dut",
        Path::new("/test/no-device"),
        vec![OsString::from("test")],
    )
    .unwrap()
}

#[test]
fn built_firmware_is_archived_before_the_exact_run_local_path_is_flashed() {
    let root = tempfile::tempdir().unwrap();
    let artifacts = build_inputs(root.path());
    let build_path = artifacts.bundle.application();
    let mut session = session(root.path());
    let run_directory = session.directory().to_owned();
    let flashed = RefCell::new(None::<PathBuf>);

    let failure = archive_and_flash_built(
        ImageClass::SystemWatchdog,
        artifacts,
        &mut session,
        |artifacts| {
            flashed.replace(Some(artifacts.bundle.application()));
            assert_eq!(
                fs::read(artifacts.bundle.application()).unwrap(),
                b"exact application"
            );
            assert_eq!(
                fs::read(artifacts.bundle.bootloader()).unwrap(),
                b"bootloader"
            );
            Ok(())
        },
    )
    .unwrap();

    assert!(failure.is_none());
    let flashed = flashed.into_inner().expect("flash call");
    assert_ne!(flashed, build_path);
    assert!(
        flashed.ends_with("system-watchdog/application.bin"),
        "{}",
        flashed.display()
    );
    assert_eq!(
        fs::read(run_directory.join("firmware/system-watchdog/application.bin")).unwrap(),
        b"exact application"
    );
}

#[test]
fn flash_failure_is_typed_after_archival() {
    let root = tempfile::tempdir().unwrap();
    let artifacts = build_inputs(root.path());
    let mut session = session(root.path());
    let failure =
        archive_and_flash_built(ImageClass::SystemWatchdog, artifacts, &mut session, |_| {
            Err("injected flash failure".into())
        })
        .unwrap()
        .expect("typed failure");
    assert_eq!(failure.kind, FailureKind::ImageFlash);
    assert_eq!(failure.message, "injected flash failure");
    assert!(
        session
            .directory()
            .join("firmware/system-watchdog/application.bin")
            .is_file()
    );
}

#[test]
fn current_build_has_the_canonical_firmware_plan_identity() {
    assert!(matches!(
        RunFirmware::BuildCurrent(crate::image::CurrentBuild {
            source: crate::image::ImageSource::Tree(std::path::PathBuf::from("/checkout")),
            features: oer_hil_model::schema::image::FeatureDelta::default(),
            layout_seed: None,
        })
        .plan(),
        PlannedFirmware::BuildCurrent
    ));
}

#[test]
fn replay_import_is_archived_before_flashing_the_new_run_local_path() {
    let root = tempfile::tempdir().unwrap();
    let artifacts = build_inputs(root.path());
    let mut source = session(root.path());
    let source_id = source.id().to_owned();
    archive_and_flash_built(ImageClass::SystemWatchdog, artifacts, &mut source, |_| {
        Ok(())
    })
    .unwrap();
    source
        .finish(Vec::new(), oer_hil_analysis::report::views)
        .unwrap();

    let archived = oer_hil_runs::replay::archived_firmware(
        root.path(),
        "chip-a",
        &source_id,
        ImageClass::SystemWatchdog,
    )
    .unwrap();
    let mut replay = session(root.path());
    let replay_directory = replay.directory().to_owned();
    let flashed = RefCell::new(None::<PathBuf>);
    let failure = import_and_flash_replay(&archived, &mut replay, |application| {
        flashed.replace(Some(application.to_owned()));
        assert_eq!(fs::read(application).unwrap(), b"exact application");
        Ok(())
    })
    .unwrap();

    assert!(failure.is_none());
    assert_eq!(
        flashed.into_inner().expect("flash call"),
        replay_directory.join("firmware/system-watchdog/application.bin")
    );
}

#[test]
fn corrupt_replay_is_rejected_before_flash_without_a_build_fallback() {
    let root = tempfile::tempdir().unwrap();
    let artifacts = build_inputs(root.path());
    let mut source = session(root.path());
    let source_id = source.id().to_owned();
    archive_and_flash_built(ImageClass::SystemWatchdog, artifacts, &mut source, |_| {
        Ok(())
    })
    .unwrap();
    source
        .finish(Vec::new(), oer_hil_analysis::report::views)
        .unwrap();

    let archived = oer_hil_runs::replay::archived_firmware(
        root.path(),
        "chip-a",
        &source_id,
        ImageClass::SystemWatchdog,
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;

        fs::set_permissions(
            &archived.application_path,
            fs::Permissions::from_mode(0o600),
        )
        .unwrap();
    }
    #[cfg(not(unix))]
    {
        let mut permissions = fs::metadata(&archived.application_path)
            .unwrap()
            .permissions();
        permissions.set_readonly(false);
        fs::set_permissions(&archived.application_path, permissions).unwrap();
    }
    fs::write(&archived.application_path, b"corrupt application").unwrap();
    let mut replay = session(root.path());
    let flash_called = std::cell::Cell::new(false);
    let error = import_and_flash_replay(&archived, &mut replay, |_| {
        flash_called.set(true);
        Ok(())
    })
    .unwrap_err();

    assert!(!flash_called.get());
    assert!(
        error
            .to_string()
            .contains("changed after bundle verification"),
        "{error}"
    );
    assert_eq!(
        fs::read(
            replay
                .directory()
                .join("firmware/system-watchdog/application.bin")
        )
        .unwrap(),
        b"corrupt application"
    );
}
