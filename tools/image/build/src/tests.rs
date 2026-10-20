use super::*;

#[test]
fn the_repository_directory_is_this_package() {
    assert!(env!("CARGO_MANIFEST_DIR").ends_with("tools/image/build"));
}

#[test]
fn an_application_selects_its_features_alone() {
    let mut application = Application {
        package: "station".into(),
        binary: "station".into(),
        features: vec!["a".into(), "b".into()],
        default_features: false,
    };
    assert_eq!(
        application.feature_arguments(),
        ["--no-default-features", "--features", "a,b"]
    );
    application.features.clear();
    application.default_features = true;
    assert!(application.feature_arguments().is_empty());
}

#[test]
fn every_chip_with_images_names_its_flash_map() {
    let repository = repository();
    let root = repository.path();
    for chip in oer_chip_profile::supported(&root).unwrap() {
        let profile = profile(&root, &chip).unwrap();
        let flash = profile.flash.unwrap();
        assert!(
            flash.bootloader < flash.partition_table && flash.partition_table < flash.application
        );
    }
}

#[test]
fn an_unchanged_embedded_runtime_keeps_its_timestamp() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("runtime.bin");
    let target = directory.path().join("bootstrap/stage-two-runtime.bin");
    std::fs::write(&source, b"runtime").unwrap();
    replace_if_changed(&source, &target).unwrap();
    let old = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1);
    std::fs::File::options()
        .write(true)
        .open(&target)
        .unwrap()
        .set_modified(old)
        .unwrap();
    replace_if_changed(&source, &target).unwrap();
    assert_eq!(std::fs::metadata(&target).unwrap().modified().unwrap(), old);
    std::fs::write(&source, b"changed").unwrap();
    replace_if_changed(&source, &target).unwrap();
    assert_eq!(std::fs::read(&target).unwrap(), b"changed");
    assert_ne!(std::fs::metadata(&target).unwrap().modified().unwrap(), old);
}

/// Enforced checks of the runtime's stack and its interrupt stacks, and
/// with `placement` its placement too.
fn requested(placement: bool) -> Gates {
    Gates {
        enforced: true,
        stack: true,
        placement,
        interrupts: Some(oer_image_checks::interrupts::Required::Proven),
        audit: None,
    }
}

/// An empty bundle in `directory` of the first chip with a flash map.
fn empty_bundle(directory: &Path) -> crate::bundle::ImageBundle {
    let repository = repository();
    let root = repository.path();
    let chip = oer_chip_profile::supported(&root).unwrap().remove(0);
    let profile = profile(&root, &chip).unwrap();
    let flash = profile.flash.clone().unwrap();
    crate::bundle::ImageBundle::new(directory, &profile, flash)
}

fn outcome(results: &[(&str, CheckResult)]) -> CheckOutcome {
    CheckOutcome {
        results: results
            .iter()
            .map(|(name, result)| ((*name).to_owned(), result.clone()))
            .collect(),
        ..CheckOutcome::default()
    }
}

#[test]
fn passed_and_not_applicable_checks_are_recorded_and_pass() {
    let directory = tempfile::tempdir().unwrap();
    let mut bundle = empty_bundle(directory.path());
    let checks = requested(false);
    let results = [
        ("stack", CheckResult::Passed),
        (
            "interrupts",
            CheckResult::NotApplicable("the image has no static interrupt table".into()),
        ),
    ];
    apply_outcome(&checks, CheckedElf::Runtime, outcome(&results), &mut bundle).unwrap();
    assert_eq!(bundle.checks.len(), 2);
    assert_eq!(bundle.checks[1].check, "interrupts");
    // The reason a property does not apply is kept, in the record and as
    // a warning.
    assert_eq!(bundle.checks[1].result, results[1].1);
    assert!(
        bundle
            .warnings
            .iter()
            .any(|warning| warning.contains("no static interrupt table")),
        "{:?}",
        bundle.warnings
    );
    assert!(!bundle.path(crate::bundle::files::CHECKS).exists());
}

#[test]
fn a_requested_check_with_missing_data_or_no_result_fails_the_build() {
    let directory = tempfile::tempdir().unwrap();
    let mut bundle = empty_bundle(directory.path());
    let checks = requested(true);
    let results = [
        ("stack", CheckResult::Passed),
        (
            "placement",
            CheckResult::MissingContract("the chip data has no [placement]".into()),
        ),
    ];
    let error = apply_outcome(&checks, CheckedElf::Runtime, outcome(&results), &mut bundle)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("placement") && error.contains("missing contract"),
        "{error}"
    );
    assert!(
        error.contains("interrupts") && error.contains("no result"),
        "{error}"
    );
    assert!(!error.contains("stack"), "{error}");
    // Every requested result is recorded, machine-readable, beside the
    // failed build.
    let written: Vec<CheckRecord> =
        serde_json::from_slice(&std::fs::read(bundle.path(crate::bundle::files::CHECKS)).unwrap())
            .unwrap();
    assert_eq!(written, bundle.checks);
    assert_eq!(written.len(), 3);
    let interrupts = written.iter().find(|record| record.check == "interrupts");
    assert!(matches!(
        interrupts.map(|record| &record.result),
        Some(CheckResult::Failed(_))
    ));
}

#[test]
fn a_reported_check_that_does_not_pass_is_a_warning() {
    let directory = tempfile::tempdir().unwrap();
    let mut bundle = empty_bundle(directory.path());
    let checks = Gates {
        enforced: false,
        ..requested(false)
    };
    let results = [("stack", CheckResult::Failed("no bound".into()))];
    apply_outcome(&checks, CheckedElf::Runtime, outcome(&results), &mut bundle).unwrap();
    assert_eq!(bundle.checks.len(), 2);
    assert!(
        bundle
            .warnings
            .iter()
            .any(|warning| warning.contains("stack") && warning.contains("no bound")),
        "{:?}",
        bundle.warnings
    );
    assert!(
        bundle
            .warnings
            .iter()
            .any(|warning| warning.contains("interrupts") && warning.contains("no result")),
        "{:?}",
        bundle.warnings
    );
    assert!(!bundle.path(crate::bundle::files::CHECKS).exists());
}

#[test]
fn runtime_checks_refuse_requested_link_time_gates() {
    let directory = tempfile::tempdir().unwrap();
    let repository = repository();
    let root = repository.path();
    let chip = oer_chip_profile::supported(&root).unwrap().remove(0);
    let spec = ImageSpec {
        root: root.to_owned(),
        chip,
        application: Application {
            package: "station".into(),
            binary: "station".into(),
            features: Vec::new(),
            default_features: true,
        },
        stack_policy: "no-such-stack.toml".into(),
        layout_seed: None,
        runtime_environment: Vec::new(),
        output: directory.path().to_owned(),
        checks: Some(requested(false)),
    };
    for check in [RuntimeCheck::Types, RuntimeCheck::Clippy] {
        let error = super::check(&spec, check).unwrap_err().to_string();
        assert!(error.contains("runtime check"), "{error}");
    }
}

pub(crate) fn repository() -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    for (path, text) in [
        (
            "chips/esp32c5/platform/chip.toml",
            include_str!("../../../../chips/esp32c5/platform/chip.toml"),
        ),
        (
            "chips/esp32s31/platform/chip.toml",
            include_str!("../../../../chips/esp32s31/platform/chip.toml"),
        ),
        (
            "chips/esp32c5/platform/partitions/applications.csv",
            include_str!("../../../../chips/esp32c5/platform/partitions/applications.csv"),
        ),
        (
            "chips/esp32s31/platform/partitions/applications.csv",
            include_str!("../../../../chips/esp32s31/platform/partitions/applications.csv"),
        ),
        (
            "firmware/peers/esp32c5-ble-dtm/firmware.toml",
            include_str!("../../../../firmware/peers/esp32c5-ble-dtm/firmware.toml"),
        ),
        (
            "firmware/peers/esp32c5-ieee802154/firmware.toml",
            include_str!("../../../../firmware/peers/esp32c5-ieee802154/firmware.toml"),
        ),
        (
            "firmware/peers/esp32c5-openthread/firmware.toml",
            include_str!("../../../../firmware/peers/esp32c5-openthread/firmware.toml"),
        ),
        (
            "firmware/vendor/esp32c5/ble-scan-request-counter/firmware.toml",
            include_str!(
                "../../../../firmware/vendor/esp32c5/ble-scan-request-counter/firmware.toml"
            ),
        ),
        (
            "firmware/vendor/esp32s31/ble-scan-reference/firmware.toml",
            include_str!("../../../../firmware/vendor/esp32s31/ble-scan-reference/firmware.toml"),
        ),
        (
            "firmware/vendor/esp32s31/calibration/firmware.toml",
            include_str!("../../../../firmware/vendor/esp32s31/calibration/firmware.toml"),
        ),
        (
            "firmware/vendor/esp32s31/ieee802154-reference/firmware.toml",
            include_str!("../../../../firmware/vendor/esp32s31/ieee802154-reference/firmware.toml"),
        ),
        (
            "firmware/vendor/esp32s31/phy-tracking-reference/firmware.toml",
            include_str!(
                "../../../../firmware/vendor/esp32s31/phy-tracking-reference/firmware.toml"
            ),
        ),
    ] {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    directory
}
