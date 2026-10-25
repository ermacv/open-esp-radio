//! Shared PHY table-memory operations over the generated PHY memory and
//! clock blocks.

#![forbid(unsafe_code)]

use oer_esp32s31_pac::{phy_clock_oracle::PhyClockOracle, phy_memory::PhyMemory};
use oer_mmio::{Bits, Index, Placement};

/// Parameter-dependent first PBUS table word (`phy_set_pbus_mem`).
///
/// SOURCE: ROM_REV0_PHY_SET_PBUS_MEM. ROM ORs the parameter byte, shifted
/// to bit 9, into the fixed image without narrowing it first.
pub const fn pbus_memory_parameter_002_data(parameter: u8) -> u32 {
    0x0008_01ff | ((parameter as u32) << 9)
}

/// Parameter-dependent group-eight PBUS table word (`phy_set_pbus_mem`).
///
/// SOURCE: ROM_REV0_PHY_SET_PBUS_MEM. ROM ORs the parameter byte, shifted
/// to bit 12, into the fixed image without narrowing it first.
pub const fn pbus_memory_parameter_014_data(parameter: u8) -> u32 {
    0x0480_01ff | ((parameter as u32) << 12)
}

/// One packed PBUS-memory group boundary.
///
/// Complete rev0 ROM `phy_write_pbus_mem` packs groups 0..11 into six
/// generated `GROUP_BOUNDARY` register words.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PbusMemoryGroupBoundary {
    pub group: u8,
    pub first_entry: u8,
    pub last_entry: u8,
}

/// One complete gain-memory entry: its three data words and its index.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct PhyGainMemoryEntry {
    words: [u32; 3],
    index: u8,
}

impl core::fmt::Debug for PhyGainMemoryEntry {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("PhyGainMemoryEntry(..)")
    }
}

/// The receive-table initialization's fixed first data word.
///
/// SOURCE: ROM_REV0_PHY_WRITE_GAIN_MEM.
const RECEIVE_TABLE_DATA_0: u32 = 0x4020_0000;

impl PhyGainMemoryEntry {
    /// Encode one generated receive-gain entry from semantic calibration data.
    ///
    /// `dc` contains the I and Q calibration components in that order.
    ///
    /// SOURCE: ROM_REV0_PHY_WRITE_GAIN_MEM, the receive-gain encoder's
    /// packing of the three data words.
    pub const fn generated_receive(
        index: u8,
        dc: [u16; 2],
        index_dc: [u16; 2],
        auxiliary: u16,
        encoded_gain: u32,
        mixer_digital_gain: u8,
        parameter_002: u8,
    ) -> Self {
        let [dc_i, dc_q] = dc;
        let [index_dc_i, index_dc_q] = index_dc;
        let data_0 = ((dc_i as u32) << 31)
            | ((dc_q as u32) << 13)
            | ((index_dc_q as u32) << 22)
            | ((auxiliary as u32) & 0x1fff);
        let data_1 = (((encoded_gain >> 4) & 0x7f) << 20)
            | ((encoded_gain & 0x7) << 17)
            | ((index_dc_i as u32) << 8)
            | ((dc_i as u32) >> 1)
            | ((mixer_digital_gain as u32) << 29);
        let data_2 = ((parameter_002 as u32) >> 6)
            | (((encoded_gain >> 15) & 0x7) << 5)
            | (((encoded_gain >> 12) & 0x7) << 2);
        Self {
            words: [data_0, data_1, data_2],
            index,
        }
    }

    /// Encode one fixed-form receive-table initialization entry.
    ///
    /// SOURCE: ROM_REV0_PHY_WRITE_GAIN_MEM.
    pub const fn receive_table(index: u8, parameter_002: u8) -> Self {
        Self {
            words: [
                RECEIVE_TABLE_DATA_0,
                0x0201_0080 | ((parameter_002 as u32) << 29),
                0x0000_00fc | ((parameter_002 as u32) >> 6),
            ],
            index,
        }
    }

    /// Encode one transmit-gain entry from the reviewed table components.
    ///
    /// SOURCE: ROM_REV0_PHY_WRITE_GAIN_MEM, the transmit-gain encoder's
    /// packing of the three data words.
    pub const fn transmit(
        index: u8,
        gain_72: u16,
        gain_64: u16,
        gain_32: u8,
        seed: [u16; 4],
        config: u16,
    ) -> Self {
        let [seed_0, seed_1, seed_2, seed_3] = seed;
        let data_0 = ((config as u32) & 0x1fff)
            | ((seed_2 as u32) << 22)
            | ((seed_1 as u32) << 31)
            | ((seed_3 as u32) << 13);
        let data_1 = 0x1000_0000
            | ((seed_0 as u32) << 8)
            | ((seed_1 as u32) >> 1)
            | ((((gain_64 as u32) >> 6) & 0xff) << 17)
            | (((gain_72 as u32) & 0x7) << 31)
            | (((gain_64 as u32) & 0x3f) << 20);
        let data_2 = 0x0000_7f80
            | (((gain_72 as u32) >> 1) & 0x3)
            | (((gain_72 as u32) >> 1) & 0x1c)
            | ((gain_32 as u32) << 15);
        Self {
            words: [data_0, data_1, data_2],
            index,
        }
    }
}

/// A shared PHY-memory operation did not fit the recovered layout.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyMemoryError {
    /// Complete ROM `phy_set_pbus_mem` constructs exactly twelve groups.
    PbusGroupOutOfRange,
    /// Complete ROM `phy_write_pbus_mem` publishes a ten-bit command.
    PbusCommandOutOfRange,
}

/// The largest ten-bit PBUS-memory command.
///
/// SOURCE: ROM_REV0_PHY_WRITE_PBUS_MEM.
const PBUS_MEMORY_COMMAND_MAXIMUM: u16 = 0x3ff;

/// Restore one packed PBUS-memory group boundary without rewriting table
/// memory.
///
/// SOURCE: ROM_REV0_PHY_WRITE_PBUS_MEM. Group `2k` takes the even pair and
/// group `2k + 1` the odd pair of boundary word `k`.
pub fn configure_pbus_memory_group_boundary<P: Placement>(
    memory: &mut PhyMemory<P>,
    boundary: PbusMemoryGroupBoundary,
) -> Result<(), PhyMemoryError> {
    let word = Index::<6>::new(usize::from(boundary.group / 2))
        .map_err(|_| PhyMemoryError::PbusGroupOutOfRange)?;
    let (first, last) = (boundary.first_entry, boundary.last_entry);
    if boundary.group.is_multiple_of(2) {
        memory.modify_group_boundary(word, |w| {
            w.set_even_group_first_entry(first)
                .set_even_group_last_entry(last);
        });
    } else {
        memory.modify_group_boundary(word, |w| {
            w.set_odd_group_first_entry(first)
                .set_odd_group_last_entry(last);
        });
    }
    Ok(())
}

/// Publish one PBUS-memory entry in complete rev0 ROM access order.
///
/// SOURCE: ROM_REV0_PHY_WRITE_PBUS_MEM. The command's low byte is the memory
/// index, its bits 8 and 9 the two command bits.
pub fn program_pbus_memory_entry<P: Placement>(
    memory: &mut PhyMemory<P>,
    boundary: Option<PbusMemoryGroupBoundary>,
    data: u32,
    command: u16,
) -> Result<(), PhyMemoryError> {
    if command > PBUS_MEMORY_COMMAND_MAXIMUM {
        return Err(PhyMemoryError::PbusCommandOutOfRange);
    }
    if let Some(boundary) = boundary {
        configure_pbus_memory_group_boundary(memory, boundary)?;
    }
    memory.write_data_0(|w| {
        w.set_value(data);
    });
    let [index, high] = command.to_le_bytes();
    memory.modify_command(|w| {
        w.set_memory_index(index)
            .set_gain_write_or_pbus_command_bit_8(high & 1 != 0)
            .set_pbus_command_bit_9(high & 2 != 0);
    });
    Ok(())
}

/// Sample the shared CFR/gain-memory base index exactly once.
pub fn read_table_memory_base_index<P: Placement>(clock: &PhyClockOracle<P>) -> u8 {
    clock.read_table_memory_index_source().base_index()
}

/// Configure the shared CFR/gain-memory base index through one fresh RMW.
pub fn configure_table_memory_base_index<P: Placement>(clock: &mut PhyClockOracle<P>, index: u8) {
    clock.modify_table_memory_index_source(|w| {
        w.set_base_index(index);
    });
}

/// Publish one TX-CFR entry and its complete set/clear commit pulse.
pub fn program_tx_cfr_entry<P: Placement>(memory: &mut PhyMemory<P>, data: u32, index: u8) {
    memory.write_data_0(|w| {
        w.set_value(data);
    });
    memory.modify_command(|w| {
        w.set_memory_index(index);
    });
    // SOURCE: BLOB_LIBPHY_PHY_SET_TX_CFR_MEM, S31_ESP_PACS_SVD.
    memory.modify_command(|w| {
        w.set_tx_cfr_commit(true);
    });
    // SOURCE: BLOB_LIBPHY_PHY_SET_TX_CFR_MEM, S31_ESP_PACS_SVD.
    memory.modify_command(|w| {
        w.set_tx_cfr_commit(false);
    });
}

/// Publish one three-word gain-memory entry in complete ROM order.
///
/// SOURCE: ROM_REV0_PHY_WRITE_GAIN_MEM. The final RMW clears the low command
/// field, writes the index and sets gain-write.
pub fn program_gain_memory_entry<P: Placement>(
    memory: &mut PhyMemory<P>,
    entry: PhyGainMemoryEntry,
) {
    let [data_0, data_1, data_2] = entry.words;
    memory.write_data_0(|w| {
        w.set_value(data_0);
    });
    memory.write_data_1(|w| {
        w.set_value(data_1);
    });
    memory.write_data_2(|w| {
        w.set_value(data_2);
    });
    memory.modify_command(|w| {
        w.set_bits_0_10_opaque(Bits::constant::<0>())
            .set_memory_index(entry.index)
            .set_gain_write_or_pbus_command_bit_8(true);
    });
}
