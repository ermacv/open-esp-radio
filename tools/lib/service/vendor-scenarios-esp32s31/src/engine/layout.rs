//! Guest addresses and peripheral encodings shared by the ESP32-S31 scenarios.
//!
//! These are chip and ROM facts owned by this verification package and scratch
//! placements chosen for its requests. Expected values never derive from the
//! compared outputs.
/// Input index of the authenticated ROM ELF in every PHY session.
pub const ROM_INPUT: u64 = 1;
/// Input index of the PHY SDK firmware after archive, ROM and production.
pub const PHY_SDK_INPUT: u64 = 3;

/// Caller-owned scratch RAM for probe inputs and copied parameter images.
pub const INPUT: u32 = 0x3fff_0000;
/// Caller-owned scratch RAM for selected outputs.
pub const OUTPUT: u32 = 0x3fff_1000;
/// Explicit callback table referenced through the ROM interface pointer.
pub const CALLBACK_TABLE: u32 = 0x3fff_3000;
/// Source of a 492-byte parameter image copied by captured `memcpy`.
pub const PARAMETER_SOURCE: u32 = 0x3fff_5000;
/// Production-side destination of a copied parameter image.
pub const PARAMETER_DESTINATION: u32 = 0x3fff_6000;
/// Production-side copy of a parameter image in scratch input RAM.
pub const PARAMETER_COPY: u32 = INPUT + 0x800;
/// Size of the captured `phy_param` object.
pub const PHY_PARAM_BYTES: u32 = 492;

/// ROM storage used in `match` patterns, so it is not resolved at run time.
/// Each constant names its pinned-ROM symbol; every session that captures
/// the ROM checks them against its inventory (`verify_rom_symbols`).
///
/// ROM interface-table pointer `rom_phyFuns`, in storage without a PT_LOAD
/// mapping.
pub const ROM_INTERFACE_POINTER: u32 = 0x2f07_fc3c;
/// ROM pointer `phy_param_rom` to the active parameter object.
pub const ROM_PARAMETER_POINTER: u32 = 0x2f07_fc40;
/// ROM callback table `g_phyFuns_instance` installed by `phy_get_romfunc_addr`.
pub const ROM_CALLBACK_TABLE: u32 = 0x2f07_f944;
/// Word slots of `g_phyFuns_instance`.
pub const ROM_CALLBACK_SLOTS: u32 = 13;
pub const ROM_SYMBOLS: [(&str, u32, u64); 3] = [
    ("rom_phyFuns", ROM_INTERFACE_POINTER, 4),
    ("phy_param_rom", ROM_PARAMETER_POINTER, 4),
    (
        "g_phyFuns_instance",
        ROM_CALLBACK_TABLE,
        4 * ROM_CALLBACK_SLOTS as u64,
    ),
];

/// Address of one word slot of the ROM callback table.
pub const fn callback_slot(index: u32) -> u32 {
    assert!(index < ROM_CALLBACK_SLOTS);
    ROM_CALLBACK_TABLE + 4 * index
}
/// Callback-table slot observed after the captured installer runs.
pub const INSTALLED_CALLBACK_SLOT: u32 = callback_slot(10);

/// Packed-command ports of the two analog I2C hosts.
pub const I2C_PORT_0: u32 = 0x2010_f800;
pub const I2C_PORT_1: u32 = 0x2010_f804;
pub const I2C_PORTS: [u32; 2] = [I2C_PORT_0, I2C_PORT_1];
/// Complemented read-mask control of the analog I2C transport.
pub const I2C_READ_MASK: u32 = 0x2010_f81c;
/// Host-map control of the analog I2C transport.
pub const I2C_HOST_MAP: u32 = 0x2010_f820;
/// Frequency-control word of the channel switch.
pub const FREQUENCY_CONTROL: u32 = 0x2010_001c;
/// Gain-bank base read that starts TX gain publication.
pub const GAIN_BASE: u32 = 0x2010_0408;
/// Baseband work mode; 2 selects the settle branch.
pub const WORK_MODE: u32 = 0x2010_9c18;
/// Low-power temperature-sensor code.
pub const TEMPERATURE_CODE: u32 = 0x2081_8000;
/// Channel readiness status; bit 8 reports a completed frequency switch.
pub const CHANNEL_STATUS: u32 = 0x2010_0028;
pub const CHANNEL_READY: u32 = 0x100;
/// PBus transaction status polled after each PBus command.
pub const PBUS_STATUS: u32 = 0x2010_0890;
/// Gain-memory index/control word and its three data words.
pub const GAIN_INDEX: u32 = 0x2010_0844;
pub const GAIN_DATA: [u32; 3] = [0x2010_0848, 0x2010_084c, 0x2010_0850];

/// Analog I2C command RAM: 45 aligned command words written by the PHY.
pub const COMMAND_RAM: u32 = 0x2010_fc00;

/// Placement of linked vendor images.
pub const IMAGE_CODE_START: u32 = 0x1100_0000;
pub const IMAGE_DATA_START: u32 = 0x2000_0000;
pub const IMAGE_REGION_BYTES: u64 = 0x100_0000;

/// Execution stack of both implementations.
pub const STACK_ADDRESS: u32 = 0x3ffe_0000;
pub const STACK_BYTES: u32 = 0x8000;
/// Radio MMIO block of the PHY, modem and analog I2C master registers.
pub const RADIO_MMIO: u32 = 0x2010_0000;
pub const RADIO_MMIO_BYTES: u32 = 0x1_0000;

/// The ESP32-S31 PHY session layout.
pub static PHY_LAYOUT: oer_vendor_scenario_engine::phy::PhyLayout =
    oer_vendor_scenario_engine::phy::PhyLayout {
        library: "libphy",
        rom: "rom",
        phy_sdk: Some("phy-sdk"),
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

pub use oer_vendor_scenario_engine::phy::layout::{
    COMMAND_BUSY, COMMAND_DATA_MASK, COMMAND_READ, COMMAND_SELECTOR_MASK, COMMAND_WRITE, FILLS,
    MAX_EVENTS, analog_bank, constant_read, fill_word, radio_aperture, register_bank,
    sequence_read,
};
