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
        let flash = profile.facts.flash.unwrap();
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
        enforced: Enforced::All,
        frames: false,
        stack: true,
        placement,
        interrupts: Some(oer_image_checks::interrupts::Required::Product),
        panics: false,
        audit: None,
    }
}

/// An empty bundle in `directory` of the first chip with a flash map.
fn empty_bundle(directory: &Path) -> crate::bundle::ImageBundle {
    let repository = repository();
    let root = repository.path();
    let chip = oer_chip_profile::supported(&root).unwrap().remove(0);
    let profile = profile(&root, &chip).unwrap();
    let flash = profile.facts.flash.clone().unwrap();
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
    apply_outcome(
        &checks,
        CheckedElf::Runtime,
        outcome(&results),
        &mut bundle,
        false,
    )
    .unwrap();
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
    let error = apply_outcome(
        &checks,
        CheckedElf::Runtime,
        outcome(&results),
        &mut bundle,
        false,
    )
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
        enforced: Enforced::Reported,
        ..requested(false)
    };
    let results = [("stack", CheckResult::Failed("no bound".into()))];
    apply_outcome(
        &checks,
        CheckedElf::Runtime,
        outcome(&results),
        &mut bundle,
        false,
    )
    .unwrap();
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
        purpose: crate::bundle::Purpose::Flash,
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
    for (path, text) in oer_espressif_facts::tracked::FILES.iter().copied().chain([
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
    ]) {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    directory
}

#[test]
fn only_interrupts_rejects_failed_missing_and_absent_results() {
    for result in [
        Some(CheckResult::Failed("no bound".into())),
        Some(CheckResult::MissingContract("no interrupt table".into())),
        None,
    ] {
        let directory = tempfile::tempdir().unwrap();
        let mut bundle = empty_bundle(directory.path());
        let checks = Gates {
            enforced: Enforced::Only([Check::Interrupts].into()),
            ..requested(false)
        };
        let mut results = vec![("stack", CheckResult::Failed("task has no bound".into()))];
        if let Some(result) = result {
            results.push(("interrupts", result));
        }
        let error = apply_outcome(
            &checks,
            CheckedElf::Runtime,
            outcome(&results),
            &mut bundle,
            false,
        )
        .unwrap_err()
        .to_string();
        assert!(
            error.starts_with("requested checks did not pass: interrupts (Runtime):"),
            "{error}"
        );
        assert!(!error.contains("task has no bound"), "{error}");
        assert!(
            bundle
                .warnings
                .iter()
                .any(|warning| warning.contains("stack")
                    && warning.contains("reported only")
                    && warning.contains("task has no bound"))
        );
        let written: Vec<CheckRecord> = serde_json::from_slice(
            &std::fs::read(bundle.path(crate::bundle::files::CHECKS)).unwrap(),
        )
        .unwrap();
        assert_eq!(written, bundle.checks);
        assert_eq!(written.len(), 2);
    }
}

#[test]
fn enforced_frame_configuration_errors_and_missing_results_still_reject_both_elfs() {
    for elf in [CheckedElf::Runtime, CheckedElf::Bootstrap] {
        for frame_result in [
            Some(CheckResult::Failed("invalid frame exception".into())),
            None,
        ] {
            let directory = tempfile::tempdir().unwrap();
            let mut bundle = empty_bundle(directory.path());
            let checks = Gates {
                enforced: Enforced::Only([Check::Frames, Check::Interrupts].into()),
                ..Gates::all(oer_image_checks::interrupts::Required::Product)
            };
            let mut results = vec![
                ("stack", CheckResult::Passed),
                ("interrupts", CheckResult::Passed),
                ("placement", CheckResult::Passed),
            ];
            if let Some(result) = frame_result {
                results.push(("frames", result));
            }
            let error = apply_outcome(&checks, elf, outcome(&results), &mut bundle, false)
                .unwrap_err()
                .to_string();
            assert!(error.contains(&format!("frames ({elf:?})")), "{error}");
            assert!(bundle.path(crate::bundle::files::CHECKS).is_file());
        }
    }
}

#[test]
fn frame_and_panic_warnings_pass_but_stack_bound_failures_still_reject() {
    for elf in [CheckedElf::Runtime, CheckedElf::Bootstrap] {
        for enforced in [Enforced::All, Enforced::Only([Check::Interrupts].into())] {
            let directory = tempfile::tempdir().unwrap();
            let checks = Gates {
                enforced,
                ..Gates::all(oer_image_checks::interrupts::Required::Product)
            };
            let results = [
                ("frames", CheckResult::Passed),
                ("panics", CheckResult::Passed),
                ("stack", CheckResult::Passed),
                ("interrupts", CheckResult::Passed),
                ("placement", CheckResult::Passed),
            ];
            let warnings = [
                "frame `large` is 4097 bytes, above the 4096-byte move limit",
                "runtime reaches 1 panic sites in the hal layer, over its budget of 0",
            ];
            let mut reported = outcome(&results);
            reported.warnings = warnings.map(str::to_owned).into();
            let mut bundle = empty_bundle(directory.path());
            apply_outcome(&checks, elf, reported, &mut bundle, true).unwrap();
            assert_eq!(bundle.warnings, warnings);
            assert!(bundle.checks.iter().all(|record| record.result.passes()));

            let mut rejected = outcome(&results);
            rejected
                .results
                .iter_mut()
                .find(|(name, _)| name == "stack")
                .unwrap()
                .1 = CheckResult::Failed("proven stack exceeds its allocation".into());
            let error = apply_outcome(&checks, elf, rejected, &mut bundle, true)
                .unwrap_err()
                .to_string();
            assert!(
                error.contains("proven stack exceeds its allocation"),
                "{error}"
            );
        }
    }
}

#[test]
fn only_interrupts_reports_failed_runtime_and_bootstrap_stacks() {
    for elf in [CheckedElf::Runtime, CheckedElf::Bootstrap] {
        let directory = tempfile::tempdir().unwrap();
        let mut bundle = empty_bundle(directory.path());
        let checks = Gates {
            enforced: Enforced::Only([Check::Interrupts].into()),
            ..requested(true)
        };
        let results = [
            ("stack", CheckResult::Failed("no bound".into())),
            ("interrupts", CheckResult::Passed),
            ("placement", CheckResult::Failed("outside SRAM".into())),
        ];
        apply_outcome(&checks, elf, outcome(&results), &mut bundle, false).unwrap();
        assert!(
            bundle
                .warnings
                .iter()
                .any(|warning| warning.contains("stack")
                    && warning.contains("reported only")
                    && warning.contains("no bound"))
        );
        if elf == CheckedElf::Runtime {
            assert!(
                bundle
                    .warnings
                    .iter()
                    .any(|warning| warning.contains("placement")
                        && warning.contains("reported only")
                        && warning.contains("outside SRAM"))
            );
        }
        assert!(!bundle.path(crate::bundle::files::CHECKS).exists());
    }
}

#[test]
fn only_interrupts_passes_with_the_executor_invariant_warning() {
    let directory = tempfile::tempdir().unwrap();
    let mut bundle = empty_bundle(directory.path());
    let checks = Gates {
        enforced: Enforced::Only([Check::Interrupts].into()),
        ..Gates::of(&[Check::Interrupts])
    };
    let warning = "hart 0's interrupt stack is conditional on the executor invariant";
    let mut result = outcome(&[("interrupts", CheckResult::Passed)]);
    result.warnings.push(warning.into());
    apply_outcome(&checks, CheckedElf::Runtime, result, &mut bundle, false).unwrap();
    assert_eq!(bundle.warnings, [warning]);
    assert_eq!(bundle.checks[0].result, CheckResult::Passed);
}

#[test]
fn only_all_enforces_the_callers_audit() {
    for enforced in [
        Enforced::All,
        Enforced::Only([Check::Stack, Check::Placement, Check::Interrupts].into()),
        Enforced::Reported,
    ] {
        let directory = tempfile::tempdir().unwrap();
        let mut bundle = empty_bundle(directory.path());
        let all = enforced == Enforced::All;
        let checks = Gates {
            enforced,
            audit: Some(Box::new(|_| Ok(()))),
            ..Gates::default()
        };
        let results = [("audit", CheckResult::Failed("audit failed".into()))];
        let result = apply_outcome(
            &checks,
            CheckedElf::Runtime,
            outcome(&results),
            &mut bundle,
            false,
        );
        assert_eq!(result.is_err(), all);
        assert_eq!(bundle.warnings.is_empty(), all);
        assert_eq!(bundle.checks[0].result, results[0].1);
    }
}

#[test]
fn a_failed_stack_check_fails_the_build_only_when_the_image_declares_its_task_stacks_proven() {
    let directory = tempfile::tempdir().unwrap();
    let results = [("stack", CheckResult::Failed("unresolved sites".into()))];
    let checks = Gates {
        enforced: Enforced::Only([Check::Frames].into()),
        ..requested(false)
    };
    let mut bundle = empty_bundle(directory.path());
    apply_outcome(
        &checks,
        CheckedElf::Runtime,
        outcome(&results),
        &mut bundle,
        false,
    )
    .unwrap();
    assert!(
        bundle
            .warnings
            .iter()
            .any(|warning| warning.contains("unresolved sites")),
        "{:?}",
        bundle.warnings
    );
    let mut bundle = empty_bundle(directory.path());
    let error = apply_outcome(
        &checks,
        CheckedElf::Runtime,
        outcome(&results),
        &mut bundle,
        true,
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("unresolved sites"), "{error}");
}
