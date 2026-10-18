//! Typed ESP32-S31 vendor-comparison scenarios.
//!
//! Each scenario authenticates caller-supplied private inputs, drives the
//! `blobray` CLI with requests built from Blobray's own types, and checks
//! independent expectations. Chip addresses and source identities belong to
//! this verification owner; generic Blobray has no chip dependency.
pub mod bluetooth;
pub mod coexistence;
pub mod decisions;
pub mod engine;
pub mod phy;
pub mod run;
pub mod wifi;

// Domain modules keep their crate-root paths.
pub use bluetooth::ble;
pub use coexistence::{coex, coex_hw};
pub use engine::{
    artifacts, contracts, coverage, evidence, harness, harness_edges, layout, observation, session,
    state,
};
pub use phy::{
    calibration_leaves, calibration_prefix, channel, gain, gain_state, i2c, i2c_transport, rfpll,
    rx_gain, tracking, tracking_graph, tx_dc,
};
pub use wifi::{ampdu_resort, low_power_clock, mac, retry, rx_append};

/// The ESP32-S31 facts the scenario engine runs with.
pub static CHIP: oer_vendor_scenario_engine::Chip = oer_vendor_scenario_engine::Chip {
    name: "esp32s31",
    manifest: "verification/esp32s31/artifacts.toml",
    manifest_text: include_str!("../../artifacts.toml"),
    hardware_scope: "crates/hardware/esp32s31",
    shared_scopes: &[
        "crates/hardware/ieee80211",
        "crates/hardware/espressif/coex",
    ],
    rom_input: layout::ROM_INPUT,
    rom_symbols: &layout::ROM_SYMBOLS,
    stack: (layout::STACK_ADDRESS, layout::STACK_BYTES),
    coverage: "verification/esp32s31/decisions/coverage.toml",
    observation: decisions::observation::DECISIONS,
    state: decisions::state::DECISIONS,
    phy: Some(&layout::PHY_LAYOUT),
    isa: oer_vendor_scenario_engine::Isa::Rv32imafcZbaZbbZbsZcbZcmp,
    registers: "registers/esp32s31/published/radio.bindings.toml",
};

/// Run the scenario engine for the ESP32-S31.
pub fn install() {
    oer_vendor_scenario_engine::install(&CHIP);
}

/// Probe workspace and the probe ELF package whose path dependencies are
/// the compiled production sources, relative to the repository root.
pub const PROBES_MANIFEST: &str = "verification/esp32s31/probes/Cargo.toml";
pub const PROBES_PACKAGE: &str = "oer-esp32s31-probe-radio-elf";
/// Package of the compiled Bluetooth probe image.
pub const BLUETOOTH_PROBES_PACKAGE: &str = "oer-esp32s31-probe-bluetooth-elf";
pub const PROBES_TARGET: &str = "riscv32imafc-unknown-none-elf";

#[cfg(test)]
mod tests {
    use crate::artifacts::{manifest, path, sha256};
    use oer_vendor_pins::SourceKind;
    use std::path::Path;

    #[test]
    fn only_fetched_pins_are_shard_input_identities() {
        super::install();
        assert!(crate::artifacts::is_fetched_pin(sha256("libphy").unwrap()));
        assert!(crate::artifacts::is_fetched_pin(sha256("rom").unwrap()));
        // A host build's digest, such as a probe's, is no pin.
        assert!(!crate::artifacts::is_fetched_pin(&"0".repeat(64)));
    }

    #[test]
    fn a_firmware_input_has_the_digest_of_its_current_build_or_none() {
        super::install();
        // Any input path, explicit or default, must have this digest: the
        // current build's, or the input is refused with the build command.
        match sha256("phy-sdk") {
            Ok(digest) => assert_eq!(digest.len(), 64),
            Err(error) => assert!(
                error
                    .to_string()
                    .contains("run `cargo hil firmware build phy-tracking-reference`"),
                "{error}"
            ),
        }
    }

    #[test]
    fn tracked_manifest_pins_every_scenario_input_once() {
        super::install();
        let manifest = manifest();
        for id in ["libphy", "librftest", "libpp", "rom"] {
            assert_eq!(sha256(id).unwrap().len(), 64, "{id}");
        }
        for id in ["sdk", "phy-sdk"] {
            assert_eq!(manifest.artifact(id).unwrap().sha256, None, "{id}");
        }
        let mut ids: Vec<_> = manifest.artifact.iter().map(|a| a.id.as_str()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), manifest.artifact.len());
        for source in &manifest.source {
            if source.kind != SourceKind::Firmware {
                assert!(
                    source.repository.is_some() && source.revision.is_some(),
                    "{}",
                    source.id
                );
            }
            if source.kind == SourceKind::Release {
                assert!(
                    source.asset.is_some() && source.sha256.is_some(),
                    "{}",
                    source.id
                );
            }
        }
    }

    #[test]
    fn firmware_outputs_lie_in_their_catalog_image_build() {
        super::install();
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let entries = oer_image::esp_idf::catalog::entries(&root).unwrap();
        let manifest = manifest();
        for artifact in &manifest.artifact {
            let source = manifest.source_of(artifact);
            if source.kind != SourceKind::Firmware {
                continue;
            }
            let image = source.image.as_deref().unwrap();
            let entry = oer_image::esp_idf::catalog::entry(&entries, image).unwrap();
            let output = oer_esp_idf::output(&root, &entry.project(&root)).join("build");
            assert!(
                path(&root, &artifact.id).starts_with(&output),
                "{}",
                artifact.id
            );
        }
        // The recipes are shard sources: every firmware project.
        assert_eq!(
            crate::artifacts::firmware_projects(&root).unwrap(),
            [Path::new(
                "verification/esp32s31/hil-vendor/phy-tracking-reference"
            )]
        );
    }

    #[test]
    fn fetched_artifacts_live_under_their_source_revision() {
        super::install();
        let root = Path::new("/repo");
        let pins = oer_vendor_pins::Manifest::parse(
            &std::fs::read_to_string(
                Path::new(env!("CARGO_MANIFEST_DIR")).join("../artifacts.toml"),
            )
            .unwrap(),
        )
        .unwrap();
        let libphy = pins.artifact("libphy").unwrap();
        assert_eq!(path(root, "libphy"), pins.location(root, libphy).unwrap());
    }
}
