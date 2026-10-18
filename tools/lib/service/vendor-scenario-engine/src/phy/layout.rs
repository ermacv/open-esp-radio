//! Guest placement and peripheral models of Espressif PHY sessions.
//!
//! A chip's scenario crate supplies its [`PhyLayout`]: pinned artifact ids,
//! ROM pointers, scratch placement and the radio and analog I2C transport
//! addresses. The analog command encoding, fill patterns and event capacity
//! are shared.
use blobray_domain::{
    CommandBank, CommandCell, CommandPort, DeviceBehavior, DeviceDeclaration, ReadRun,
    RegionLifetime, RegisterCell,
};

/// One chip's PHY session layout.
#[derive(Debug)]
pub struct PhyLayout {
    /// Manifest ids of the PHY archive, the ROM ELF and, where the chip's
    /// PHY images need one, the PHY SDK firmware.
    pub library: &'static str,
    pub rom: &'static str,
    pub phy_sdk: Option<&'static str>,
    /// Caller-owned scratch RAM for probe inputs and for selected outputs.
    pub input: u32,
    pub output: u32,
    /// Production-side copy of a parameter image in scratch input RAM.
    pub parameter_copy: u32,
    /// Size of the captured `phy_param` object.
    pub phy_param_bytes: u32,
    /// ROM interface-table pointer `rom_phyFuns`, ROM pointer `phy_param_rom`
    /// and ROM callback table `g_phyFuns_instance`.
    pub rom_interface_pointer: u32,
    pub rom_parameter_pointer: u32,
    pub rom_callback_table: u32,
    /// Placement of linked vendor images.
    pub image_code_start: u32,
    pub image_data_start: u32,
    pub image_region_bytes: u64,
    /// Packed-command ports of the two analog I2C hosts, the complemented
    /// read-mask control and the host-map control of the transport.
    pub i2c_ports: [u32; 2],
    pub i2c_read_mask: u32,
    pub i2c_host_map: u32,
    /// Radio MMIO block of the PHY, modem and analog I2C master registers.
    pub radio_mmio: u32,
    pub radio_mmio_bytes: u32,
}

/// The installed chip's PHY layout.
pub fn layout() -> &'static PhyLayout {
    crate::chip().phy()
}

/// Packed analog command fields.
pub const COMMAND_SELECTOR_MASK: u32 = 0xffff;
pub const COMMAND_DATA_MASK: u32 = 0x00ff_0000;
pub const COMMAND_READ: u32 = 0x0400_0000;
pub const COMMAND_WRITE: u32 = 0x0500_0000;
pub const COMMAND_BUSY: u32 = 0x0200_0000;

/// Stack and peripheral fill patterns exercised by every matrix.
pub const FILLS: [u8; 2] = [0x5a, 0xa5];

/// A fill byte replicated into every byte of a register word.
pub fn fill_word(fill: u8) -> u32 {
    u32::from(fill) * 0x0101_0101
}

/// Event capacity of one phase in the finite matrices.
pub const MAX_EVENTS: u32 = 32768;

/// Analog command bank over both I2C ports with a shared retained cell set.
pub fn analog_bank(
    id: &str,
    applicability: &str,
    busy: u32,
    initial_busy: [u32; 2],
    mut cells: Vec<CommandCell>,
) -> DeviceDeclaration {
    cells.sort_by_key(|c| c.selector);
    DeviceDeclaration {
        id: id.into(),
        applicability: applicability.into(),
        lifetime: RegionLifetime::Phase,
        behavior: DeviceBehavior::CommandBank(CommandBank {
            selector_mask: COMMAND_SELECTOR_MASK,
            data_mask: COMMAND_DATA_MASK,
            read_command: COMMAND_READ,
            write_command: COMMAND_WRITE,
            busy_mask: COMMAND_BUSY,
            reset_command: Some(COMMAND_READ),
            ports: layout()
                .i2c_ports
                .iter()
                .zip(initial_busy)
                .map(|(address, initial_busy_reads)| CommandPort {
                    address: *address,
                    initial: 0,
                    initial_busy_reads,
                    busy_reads: busy,
                })
                .collect(),
            cells,
        }),
    }
}

/// Every radio register not given an explicit model is retained storage that
/// starts with the fill pattern. Explicit models inside the block take
/// precedence; reads of never-written words stay visible as MMIO events.
pub fn radio_aperture(fill: u8) -> DeviceDeclaration {
    DeviceDeclaration {
        id: "radio-retained".into(),
        applicability: "unmodeled radio registers retain writes; fill-pattern initial words".into(),
        lifetime: RegionLifetime::Phase,
        behavior: DeviceBehavior::RetainedAperture {
            start: layout().radio_mmio,
            length: layout().radio_mmio_bytes,
            initial: fill_word(fill),
        },
    }
}

/// Retained word registers with explicit initial values.
pub fn register_bank(id: &str, applicability: &str, cells: Vec<(u32, u32)>) -> DeviceDeclaration {
    let mut cells = cells;
    cells.sort();
    DeviceDeclaration {
        id: id.into(),
        applicability: applicability.into(),
        lifetime: RegionLifetime::Phase,
        behavior: DeviceBehavior::RegisterBank {
            cells: cells
                .into_iter()
                .map(|(address, value)| RegisterCell {
                    address,
                    width: 4,
                    value,
                })
                .collect(),
        },
    }
}

/// A word register that always reads `value`.
pub fn constant_read(id: &str, address: u32, value: u32) -> DeviceDeclaration {
    DeviceDeclaration {
        id: id.into(),
        applicability: "explicit constant peripheral observation".into(),
        lifetime: RegionLifetime::Phase,
        behavior: DeviceBehavior::ConstantRead {
            address,
            width: 4,
            value,
        },
    }
}

/// A word register that returns finite runs of values, then is exhausted.
pub fn sequence_read(id: &str, address: u32, runs: Vec<ReadRun>) -> DeviceDeclaration {
    DeviceDeclaration {
        id: id.into(),
        applicability: "finite caller-supplied peripheral observations".into(),
        lifetime: RegionLifetime::Phase,
        behavior: DeviceBehavior::SequenceRead {
            address,
            width: 4,
            runs,
        },
    }
}
