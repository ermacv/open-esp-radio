use super::*;

#[test]
fn image_classes_are_stable_and_do_not_use_workload_environment() {
    assert_eq!(oer_hil_model::schema::image::ImageClass::ALL.len(), 9);
    for (class, features) in [
        (ImageClass::AesKeyUnwrap, "aes-key-unwrap"),
        (ImageClass::WifiStation, "wifi-station"),
        (ImageClass::SystemWatchdog, "system-watchdog"),
        (
            ImageClass::DiagnosticUsbJtagOff,
            "system-watchdog,usb-jtag-off",
        ),
        (ImageClass::SystemPanicReset, "system-panic-reset"),
        (ImageClass::BootSmoke, "boot-smoke"),
        (ImageClass::DiagnosticPhy, "diagnostic-phy"),
        (ImageClass::DiagnosticRadioHealth, "diagnostic-radio-health"),
        (ImageClass::DiagnosticLowerMac, "diagnostic-lower-mac"),
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
    let tree = "oer-chip-a-hil-agent v0.1.0 (/repo/firmware/hil/chip-a)\n\
                critical-section v1.2.0\n\
                oer-ieee802154-mac v0.1.0 (/repo/protocols/ieee802154/mac) (*)\n";
    assert_eq!(
        tree_packages(tree),
        [
            "critical-section",
            "oer-chip-a-hil-agent",
            "oer-ieee802154-mac"
        ]
        .map(str::to_owned)
        .into()
    );
}
