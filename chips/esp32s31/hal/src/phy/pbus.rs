//! Owned access to the recovered ESP32-S31 PHY PBus registers.

#[cfg(target_arch = "riscv32")]
use crate::{
    owner::{SharedPhyAccess, SharedPhyContext},
    phy_blocks_mut,
};

pub mod ops;

/// Apply one finite force-TX/RX phase around its caller-owned delay edge.
///
/// Basis: complete rev0 ROM `phy_force_txrx_off` at `0x2f827bb0`, size
/// `0x66`. The parent performs two fresh-read replacements of bits 11:8 with
/// a one-microsecond delay after each. This method owns exactly one
/// replacement; the Rust transition owns phase order and both timers.
#[cfg(target_arch = "riscv32")]
pub fn configure_force_txrx(registers: &mut impl SharedPhyAccess, enabled: bool, phase: u8) {
    ops::set_force_txrx_mode(&mut phy_blocks_mut(registers).phy_pbus, enabled, phase == 0);
}

/// Publish one PBus force-test command before the first busy sample.
///
/// Basis: complete rev0 ROM `phy_pbus_force_test` at `0x2f824228`, size
/// `0x42`. The combined argument mask and final transaction bit are exact.
/// In particular, ROM accepts signed RX-DCO halfword images and retains their
/// low eleven bits while composing the command; it does not reject values
/// above the separately visible nine-bit PBus result window. Like ROM, this
/// edge publishes first; completion sampling and its finite Rust deadline are
/// owned by the family PHY completion driver.
#[cfg(target_arch = "riscv32")]
pub fn start_force_test(
    registers: &mut impl SharedPhyAccess,
    selector: u8,
    path: u8,
    test_value: u16,
) {
    ops::publish_force_test(
        &mut phy_blocks_mut(registers).phy_pbus,
        selector,
        path,
        test_value,
    );
}

/// Read the nine-bit packed result selected by the ROM address/shift tables.
///
/// Basis: complete rev0 ROM `phy_pbus_rd` plus its jump tables at
/// `0x2f84d910` and `0x2f84d924`. Result-window locations are exact; except
/// for selector 1's low RX-DCO consumer, their analog meanings remain
/// intentionally unknown.
#[cfg(target_arch = "riscv32")]
pub fn read_result(registers: &mut impl SharedPhyAccess, selector: u8, path: u8) -> Option<u16> {
    Some(ops::read_result(
        &phy_blocks_mut(registers).phy_pbus,
        selector,
        path,
    ))
}

/// Enable or disable both recovered RX clock bits as one indivisible pair.
///
/// Basis: complete rev0 ROM `phy_set_rxclk_en` at `0x2f827cf6`, size
/// `0x20`. The body never distinguishes the two constituent clocks, so the
/// PAC and HAL retain pair-level semantics.
#[cfg(target_arch = "riscv32")]
pub fn configure_rx_clock(registers: &mut impl SharedPhyAccess, enabled: bool) {
    ops::set_rx_clock_pair(&mut phy_blocks_mut(registers).phy_pbus, enabled);
}

/// Enable or disable both recovered TX clock bits as one indivisible pair.
///
/// Basis: complete rev0 ROM `phy_set_txclk_en` at `0x2f827cd2`, size
/// `0x24`. The body never distinguishes the two constituent clocks, so the
/// PAC and HAL retain pair-level semantics.
#[cfg(target_arch = "riscv32")]
pub fn configure_tx_clock(registers: &mut impl SharedPhyAccess, enabled: bool) {
    ops::set_tx_clock_pair(&mut phy_blocks_mut(registers).phy_pbus, enabled);
}

#[cfg(target_arch = "riscv32")]
/// Exclusive borrowed PBus atoms from the current affine PHY owner.
pub struct Port<'a, T: ?Sized>(&'a mut T);

#[cfg(target_arch = "riscv32")]
pub fn port<T: SharedPhyAccess + ?Sized>(owner: &mut T) -> Port<'_, T> {
    Port(owner)
}

#[cfg(target_arch = "riscv32")]
impl<T: SharedPhyAccess + ?Sized> oer_espressif_phy::pbus::PbusAccess for Port<'_, T> {
    fn set_work_mode(&mut self, enabled: bool) {
        ops::set_work_mode(&mut phy_blocks_mut(self.0).phy_pbus, enabled);
    }
    fn set_debug_mode(&mut self, enabled: bool) {
        ops::set_debug_mode(&mut phy_blocks_mut(self.0).phy_pbus, enabled);
    }
    fn is_busy(&self) -> bool {
        ops::is_busy(&crate::phy_blocks(self.0).phy_pbus)
    }
    fn clear_transaction(&mut self) {
        ops::clear_transaction(&mut phy_blocks_mut(self.0).phy_pbus);
    }
}
#[cfg(target_arch = "riscv32")]
impl<T: SharedPhyContext + ?Sized> oer_espressif_phy::pbus::PbusContext for Port<'_, T> {
    fn wifi_baseband_enabled(&self) -> bool {
        crate::phy_blocks(self.0)
            .modem_syscon_radio
            .read_wifi_bb_cfg()
            .wifi_enable()
    }
}
