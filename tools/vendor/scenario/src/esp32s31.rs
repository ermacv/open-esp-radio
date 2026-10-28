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
pub mod session_entry;
pub mod wifi;

// Domain modules keep their crate-root paths.
pub use bluetooth::ble;
pub use coexistence::coex_hw;
pub use engine::{
    artifacts, contracts, evidence, harness, harness_edges, layout, observation, session, state,
};
pub use phy::{calibration_leaves, gain, gain_state, i2c};
pub use wifi::mac;

/// The ESP32-S31 facts the scenario engine runs with.
pub static CHIP: crate::Chip = crate::Chip {
    name: "esp32s31",
    manifest_text: include_str!("../../../../chips/esp32s31/vendor/artifacts.toml"),
    hardware_scope: "chips/esp32s31",
    shared_scopes: &["protocols/ieee80211", "chips/espressif/coex"],
    rom_input: layout::ROM_INPUT,
    rom_symbols: &layout::ROM_SYMBOLS,
    stack: (layout::STACK_ADDRESS, layout::STACK_BYTES),
    observation: decisions::observation::DECISIONS,
    phy: Some(&layout::PHY_LAYOUT),
    isa: crate::Isa::Rv32imafcZbaZbbZbsZcbZcmp,
    registers: "esp32s31",
};

/// Run the scenario engine for the ESP32-S31.
pub fn install() {
    crate::install(&CHIP);
}
