//! Owned access to the recovered ESP32-S31 PHY PBus registers.

#[cfg(target_arch = "riscv32")]
use crate::{
    owner::{SharedPhyAccess, SharedPhyContext},
    phy_pac_mut,
};

/// A PBus command could not be published or completed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PbusError {
    /// The hardware busy bit was set at the single allowed observation.
    Busy,
}

/// Apply one finite force-TX/RX phase around its caller-owned delay edge.
///
/// Basis: complete rev0 ROM `phy_force_txrx_off` at `0x2f827bb0`, size
/// `0x66`. The parent performs two fresh-read replacements of bits 11:8 with
/// a one-microsecond delay after each. This method owns exactly one
/// replacement; the Rust transition owns phase order and both timers.
#[cfg(target_arch = "riscv32")]
pub fn configure_force_txrx(registers: &mut impl SharedPhyAccess, enabled: bool, phase: u8) {
    let registers = phy_pac_mut(registers);
    registers.set_pbus_force_txrx_mode(enabled, phase == 0);
}

/// Enter the PBus debug mode used before force-test transactions.
///
/// Basis: complete rev0 ROM `phy_pbus_debugmode` and
/// `phy_pbus_force_mode(1)`. They clear `MODE.WORK_MODE_ENABLE`, then set
/// `COMMAND.DEBUG_MODE_ENABLE`, with one fresh read before each write.
#[cfg(target_arch = "riscv32")]
pub fn configure_debug_mode(registers: &mut impl SharedPhyAccess) {
    let registers = phy_pac_mut(registers);
    registers.set_pbus_work_mode(false);
    registers.set_pbus_debug_mode(true);
}

/// Enter PBus work mode and sample the optional settle-pulse condition once.
///
/// Basis: complete rev0 ROM `phy_pbus_force_mode(0)`. It clears
/// `COMMAND.DEBUG_MODE_ENABLE`, sets `MODE.WORK_MODE_ENABLE`, and samples the
/// semantic Wi-Fi-baseband-enable condition. The returned boolean only reports
/// that instruction-evidenced condition; the delayed pulse itself belongs to
/// the caller's async transition.
#[cfg(target_arch = "riscv32")]
pub fn configure_work_mode(registers: &mut impl SharedPhyContext) -> bool {
    {
        let registers = phy_pac_mut(registers);
        registers.set_pbus_debug_mode(false);
        registers.set_pbus_work_mode(true);
    }
    // The ROM samples this condition after both mode writes. Preserve that
    // observation boundary instead of using a pre-transition snapshot.
    SharedPhyContext::wifi_baseband_enable_observation(registers).is_enabled()
}

/// Publish one PBus force-test command before the first busy sample.
///
/// Basis: complete rev0 ROM `phy_pbus_force_test` at `0x2f824228`, size
/// `0x42`. The combined argument mask and final transaction bit are exact.
/// In particular, ROM accepts signed RX-DCO halfword images and retains their
/// low eleven bits while composing the command; it does not reject values
/// above the separately visible nine-bit PBus result window. Like ROM, this
/// edge publishes first; completion sampling and its finite Rust deadline are
/// owned by [`try_finish_force_test`].
#[cfg(target_arch = "riscv32")]
pub fn start_force_test(
    registers: &mut impl SharedPhyAccess,
    selector: u8,
    path: u8,
    test_value: u16,
) {
    let registers = phy_pac_mut(registers);
    registers.publish_pbus_force_test(selector, path, test_value);
}

/// Observe one force-test completion edge and clear its transaction bit.
///
/// Basis: the suffix of complete rev0 ROM `phy_pbus_force_test`. ROM spins
/// until `STATUS_CLOCK_FORCE.BUSY` clears and then clears
/// `COMMAND.TRANSACTION_START`; this HAL performs only one observation so the
/// Rust async owner controls retries and timeout.
#[cfg(target_arch = "riscv32")]
pub fn try_finish_force_test(registers: &mut impl SharedPhyAccess) -> Result<(), PbusError> {
    let registers = phy_pac_mut(registers);
    if registers.pbus_is_busy() {
        return Err(PbusError::Busy);
    }
    registers.clear_pbus_transaction();
    Ok(())
}

/// Read the nine-bit packed result selected by the ROM address/shift tables.
///
/// Basis: complete rev0 ROM `phy_pbus_rd` plus its jump tables at
/// `0x2f84d910` and `0x2f84d924`. Result-window locations are exact; except
/// for selector 1's low RX-DCO consumer, their analog meanings remain
/// intentionally unknown.
#[cfg(target_arch = "riscv32")]
pub fn read_result(registers: &mut impl SharedPhyAccess, selector: u8, path: u8) -> Option<u16> {
    let registers = phy_pac_mut(registers);
    registers.read_pbus_result(selector, path)
}

/// Enable or disable both recovered RX clock bits as one indivisible pair.
///
/// Basis: complete rev0 ROM `phy_set_rxclk_en` at `0x2f827cf6`, size
/// `0x20`. The body never distinguishes the two constituent clocks, so the
/// PAC and HAL retain pair-level semantics.
#[cfg(target_arch = "riscv32")]
pub fn configure_rx_clock(registers: &mut impl SharedPhyAccess, enabled: bool) {
    let registers = phy_pac_mut(registers);
    registers.set_pbus_rx_clock_pair(enabled);
}

/// Enable or disable both recovered TX clock bits as one indivisible pair.
///
/// Basis: complete rev0 ROM `phy_set_txclk_en` at `0x2f827cd2`, size
/// `0x24`. The body never distinguishes the two constituent clocks, so the
/// PAC and HAL retain pair-level semantics.
#[cfg(target_arch = "riscv32")]
pub fn configure_tx_clock(registers: &mut impl SharedPhyAccess, enabled: bool) {
    let registers = phy_pac_mut(registers);
    registers.set_pbus_tx_clock_pair(enabled);
}
