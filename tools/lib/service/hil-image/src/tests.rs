use super::*;

#[test]
fn image_classes_are_stable_and_do_not_use_workload_environment() {
    assert_eq!(oer_hil_schema::image::ImageClass::ALL.len(), 4);
    for (class, features) in [
        (ImageClass::SystemWatchdog, "system-watchdog"),
        (
            ImageClass::DiagnosticUsbJtagOff,
            "system-watchdog,usb-jtag-off",
        ),
        (ImageClass::SystemPanicReset, "system-panic-reset"),
        (ImageClass::BootSmoke, "boot-smoke"),
    ] {
        assert_eq!(class.runtime_features(), features);
    }
}

#[test]
fn removed_images_are_rejected_by_both_decoders() {
    for id in [
        "diagnostic-rx-delivery-phy-steps",
        "diagnostic-rx-delivery-phy-settle",
        "bluetooth-dtm",
        "bluetooth-gatt",
        "bluetooth-secure-gatt",
        "diagnostic-bluetooth-gatt-trace",
    ] {
        assert!(serde_json::from_str::<ImageClass>(&format!("\"{id}\"")).is_err());
        assert!(id.parse::<ImageClass>().is_err());
    }
}

/// This checkout's first chip whose HIL agent builds the panic-reset image
/// (`full`), the staged agent, or does not, the application agent.
fn agent(full: bool) -> oer_chip_profile::Profile {
    oer_chip_profile::Profile::all(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.."))
        .unwrap()
        .into_iter()
        .find(|profile| oer_hil_image_class::declares(&profile.id, "system-panic-reset") == full)
        .unwrap()
}

#[test]
fn a_seeded_build_has_artifacts_of_its_own() {
    assert_eq!(super::seed_suffix(None), "");
    let seven = super::seed_suffix(std::num::NonZeroU32::new(7));
    let eleven = super::seed_suffix(std::num::NonZeroU32::new(11));
    assert_eq!(seven, "-seed7");
    assert_ne!(seven, eleven);
}

#[test]
fn cargo_tree_lines_name_their_packages() {
    let tree = "oer-chip-a-hil-agent v0.1.0 (/repo/hil/targets/chip-a/agent)\n\
                critical-section v1.2.0\n\
                oer-ieee802154 v0.1.0 (/repo/crates/protocols/ieee802154) (*)\n";
    assert_eq!(
        tree_packages(tree),
        ["critical-section", "oer-chip-a-hil-agent", "oer-ieee802154"]
            .map(str::to_owned)
            .into()
    );
}

#[test]
fn the_application_chip_builds_the_boot_smoke_and_system_watchdog_images() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let chip = agent(false).id;
    assert!(serves(&root, &chip, ImageClass::BootSmoke).unwrap());
    assert!(serves(&root, &chip, ImageClass::SystemWatchdog).unwrap());
    assert!(!serves(&root, &chip, ImageClass::SystemPanicReset).unwrap());
}

#[test]
fn each_chip_s_spec_takes_its_agent_policy() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let placement = || PathBuf::from("/out");
    let staged_chip = agent(true);
    let application_chip = agent(false);
    let staged = spec(
        &root,
        &staged_chip.id,
        ImageClass::SystemWatchdog,
        (None, &FeatureDelta::default()),
        Overrides::default(),
        placement(),
    )
    .unwrap();
    assert_eq!(staged.application.package, staged_chip.hil_agent_package());
    assert_eq!(staged.application.features, ["system-watchdog"]);
    assert_eq!(staged.stack_policy, staged_chip.hil_stack_policy());
    assert!(staged.checks.is_some());
    assert!(!staged.builder_inputs.is_empty());
    let _diagnostic = spec(
        &root,
        &staged_chip.id,
        ImageClass::DiagnosticUsbJtagOff,
        (None, &FeatureDelta::default()),
        Overrides::default(),
        placement(),
    )
    .unwrap();
    let esp_idf = spec(
        &root,
        &application_chip.id,
        ImageClass::SystemWatchdog,
        (None, &FeatureDelta::default()),
        Overrides::default(),
        placement(),
    )
    .unwrap();
    assert_eq!(
        esp_idf.application.package,
        application_chip.hil_agent_package()
    );
    assert_eq!(
        root.join(&esp_idf.application.workspace),
        application_chip.hil_agent_workspace(&root)
    );
    assert_eq!(esp_idf.application.features, ["system-watchdog"]);
    assert_eq!(esp_idf.stack_policy, application_chip.hil_stack_policy());
    assert!(esp_idf.checks.is_some());
}
