//! Owned access to the shared ESP32-S31 PHY table-memory aperture.

pub use crate::types::{
    PbusMemoryGroupBoundary, PhyForcedPowerIndex, PhyGainMemoryEntry, PhyMemoryError,
};
#[cfg(target_arch = "riscv32")]
use crate::{owner::SharedPhyAccess, phy_pac, phy_pac_mut};

/// Publish one entry of the PBUS-memory table.
///
/// Basis: complete rev0 ROM `phy_write_pbus_mem` at `0x2f82_4634`, size
/// `0x16a`. On the first entry of a group it performs one packed-boundary
/// RMW, then writes `DATA_0`, then replaces the ten-bit command. The PAC
/// preserves that exact access order and performs no polling.
#[cfg(target_arch = "riscv32")]
pub fn program_pbus_memory_entry(
    registers: &mut impl SharedPhyAccess,
    boundary: Option<PbusMemoryGroupBoundary>,
    data: u32,
    command: u16,
) -> Result<(), PhyMemoryError> {
    let registers = phy_pac_mut(registers);
    registers.program_pbus_memory_entry(boundary, data, command)
}

/// Restore one retained PBUS-memory group boundary without rewriting the
/// table entries.
///
/// Basis: current S31 `libphy.a[phy_init.o]::phy_wakeup_init` calls
/// `phy_set_pbus_reg`, which restores the six packed boundary words saved by
/// cold `phy_set_pbus_mem`. Source-owned code regenerates the same twelve
/// semantic boundaries instead of retaining opaque register images.
#[cfg(target_arch = "riscv32")]
pub fn configure_pbus_memory_group_boundary(
    registers: &mut impl SharedPhyAccess,
    boundary: PbusMemoryGroupBoundary,
) -> Result<(), PhyMemoryError> {
    let registers = phy_pac_mut(registers);
    registers.configure_pbus_memory_group_boundary(boundary)
}

/// Sample the shared CFR/gain-memory base index once.
///
/// Basis: complete S31 `libphy.a[phy_tx_gain.o]::phy_set_tx_cfr_mem` and
/// `phy_set_tx_gain_mem_new`. Both read the generated
/// `TABLE_MEMORY_INDEX_SOURCE.BASE_INDEX` field exactly once before
/// publishing any table entry.
#[cfg(target_arch = "riscv32")]
pub fn read_table_memory_base_index(registers: &impl SharedPhyAccess) -> u8 {
    let registers = phy_pac(registers);
    registers.read_table_memory_base_index()
}

/// Configure the shared CFR/gain-memory base index.
///
/// Basis: complete rev0 ROM `phy_fe_reg_init` at `0x2f82_7740`. Its fifth
/// fresh-read RMW replaces exactly the generated base-index field with
/// `0xa0`. Complete S31 CFR and gain publishers later sample that same byte.
#[cfg(target_arch = "riscv32")]
pub fn configure_table_memory_base_index(registers: &mut impl SharedPhyAccess, index: u8) {
    let registers = phy_pac_mut(registers);
    registers.configure_table_memory_base_index(index);
}

/// Apply complete rev0 ROM `phy_force_pwr_index`.
#[cfg(target_arch = "riscv32")]
pub fn configure_forced_power_index(
    registers: &mut impl SharedPhyAccess,
    enabled: bool,
    index: PhyForcedPowerIndex,
) {
    let registers = phy_pac_mut(registers);
    registers.configure_forced_power_index(enabled, index);
}

/// Publish one TX-CFR memory entry and its complete commit pulse.
///
/// Basis: complete S31 `libphy.a[phy_tx_gain.o]::phy_set_tx_cfr_mem`, size
/// `0x76`. The body writes `DATA_0`, replaces only the index, then performs
/// fresh-read set and clear RMW operations on the commit bit.
#[cfg(target_arch = "riscv32")]
pub fn program_tx_cfr_entry(registers: &mut impl SharedPhyAccess, data: u32, index: u8) {
    let registers = phy_pac_mut(registers);
    registers.program_tx_cfr_entry(data, index);
}

/// Publish one opaque PAC-encoded gain-memory entry.
///
/// Basis: complete rev0 ROM `phy_write_gain_mem` at `0x2f82_74f0`, size
/// `0x2a`. The PAC writes all three private data words in order, clears the
/// low command field, writes the index, sets gain-write, and preserves the
/// upper fields in one final RMW.
#[cfg(target_arch = "riscv32")]
pub fn program_gain_memory_entry(registers: &mut impl SharedPhyAccess, entry: PhyGainMemoryEntry) {
    let registers = phy_pac_mut(registers);
    registers.program_gain_memory_entry(entry);
}
