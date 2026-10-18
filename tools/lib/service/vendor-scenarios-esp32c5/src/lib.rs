//! Typed ESP32-C5 vendor-comparison scenarios.
//!
//! Each scenario authenticates the pinned vendor inputs, drives Blobray
//! through the chip-neutral scenario engine and checks expectations
//! independent of both executions. Chip addresses and source identities
//! belong to this verification owner.
pub mod decisions;
pub mod phy_i2c;
pub mod run;

use oer_vendor_scenario_engine::{Chip, Isa, phy::PhyLayout};

/// Session input of the authenticated ROM ELF.
pub const ROM_INPUT: u64 = 1;
/// Manifest ids of the pinned PHY archive and of the ROM of the stand's
/// ESP32-C5 v1.0 (`esp32c5-eco2-20250121`).
pub const LIBRARY: &str = "libphy";
pub const ROM: &str = "rom-rev100";

/// Caller-owned scratch RAM for probe inputs and outputs, and the execution
/// stack. The ESP32-C5 maps nothing at the scratch addresses; the stack lies
/// in HP SRAM (0x4080_0000..0x4086_0000) below the ROM data at 0x4085_xxxx.
const INPUT: u32 = 0x3fff_0000;
const OUTPUT: u32 = 0x3fff_1000;
pub const PARAMETER_COPY: u32 = INPUT + 0x800;
const STACK: (u32, u32) = (0x4084_0000, 0x8000);

/// Placement of linked vendor images, outside every ESP32-C5 mapping (flash
/// is at 0x4200_0000).
const IMAGE_CODE_START: u32 = 0x1100_0000;
const IMAGE_DATA_START: u32 = 0x2000_0000;
const IMAGE_REGION_BYTES: u64 = 0x100_0000;

/// ROM interface-table pointer `rom_phyFuns`, ROM `phy_param_rom` and the
/// ROM callback table `g_phyFuns_instance` of the rev100 ROM.
const ROM_INTERFACE_POINTER: u32 = 0x4085_fc6c;
const ROM_PARAMETER_POINTER: u32 = 0x4085_fc70;
const ROM_CALLBACK_TABLE: u32 = 0x4085_faac;
/// Size of `phy_param` in the pinned `libphy.a`.
pub const PHY_PARAM_BYTES: u32 = 0x438;

/// `I2C_ANA_MST` of ESP-IDF `soc/esp32c5/include/modem/i2c_ana_mst_reg.h`:
/// the two host command ports, the complemented read mask `ANA_CONF1` and
/// the host map `ANA_CONF2`.
pub const I2C_PORTS: [u32; 2] = [0x600a_f800, 0x600a_f804];
pub const I2C_READ_MASK: u32 = 0x600a_f81c;
pub const I2C_HOST_MAP: u32 = 0x600a_f820;
/// The modem window, which holds `I2C_ANA_MST`.
const RADIO_MMIO: u32 = 0x600a_0000;
const RADIO_MMIO_BYTES: u32 = 0x1_0000;

/// The ESP32-C5 PHY session layout.
pub static PHY_LAYOUT: PhyLayout = PhyLayout {
    library: LIBRARY,
    rom: ROM,
    phy_sdk: None,
    input: INPUT,
    output: OUTPUT,
    parameter_copy: PARAMETER_COPY,
    phy_param_bytes: PHY_PARAM_BYTES,
    rom_interface_pointer: ROM_INTERFACE_POINTER,
    rom_parameter_pointer: ROM_PARAMETER_POINTER,
    rom_callback_table: ROM_CALLBACK_TABLE,
    image_code_start: IMAGE_CODE_START,
    image_data_start: IMAGE_DATA_START,
    image_region_bytes: IMAGE_REGION_BYTES,
    i2c_ports: I2C_PORTS,
    i2c_read_mask: I2C_READ_MASK,
    i2c_host_map: I2C_HOST_MAP,
    radio_mmio: RADIO_MMIO,
    radio_mmio_bytes: RADIO_MMIO_BYTES,
};

/// The ESP32-C5 facts the scenario engine runs with.
pub static CHIP: Chip = Chip {
    name: "esp32c5",
    manifest: "verification/esp32c5/artifacts.toml",
    manifest_text: include_str!("../../artifacts.toml"),
    hardware_scope: "crates/hardware/esp32c5",
    shared_scopes: &["crates/hardware/ieee80211"],
    rom_input: ROM_INPUT,
    rom_symbols: &[],
    stack: STACK,
    coverage: "verification/esp32c5/decisions/coverage.toml",
    observation: decisions::observation::DECISIONS,
    state: decisions::state::DECISIONS,
    phy: Some(&PHY_LAYOUT),
    isa: Isa::Rv32imac,
    registers: "registers/esp32c5/published/radio.bindings.toml",
};

/// Run the scenario engine for the ESP32-C5.
pub fn install() {
    oer_vendor_scenario_engine::install(&CHIP);
}

/// Probe workspace and the probe ELF package whose path dependencies are
/// the compiled production sources, relative to the repository root.
pub const PROBES_MANIFEST: &str = "verification/esp32c5/probes/Cargo.toml";
pub const PROBES_PACKAGE: &str = "oer-esp32c5-probe-radio-elf";
