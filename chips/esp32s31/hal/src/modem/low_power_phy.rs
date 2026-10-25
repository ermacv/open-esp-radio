//! Closed MODEM_LPCON capability owned by the shared ESP32-S31 PHY route.
//!
//! Only the complete-libphy-evidenced `TICK_CONF` transition is present. The
//! remaining MODEM_LPCON clock and reset registers stay outside this owner
//! until their Wi-Fi, Bluetooth, coexistence, and IEEE 802.15.4 lifecycles
//! share one explicit clock capability.

#![forbid(unsafe_code)]

use crate::phy::RadioPhyRegisters;

impl<P: crate::phy::registers::RadioPlacement> RadioPhyRegisters<P> {
    /// Configure the modem-power tick for the fixed 40 MHz S31 crystal.
    ///
    /// Complete `phy_get_xtal_freq` replaces only the low six bits with
    /// `frequency_mhz - 1`; the generated transaction preserves every other
    /// bit of `TICK_CONF`.
    pub fn configure_fixed_xtal_40mhz_tick(&mut self) {
        // SOURCE: BLOB_LIBPHY_PHY_GET_XTAL_FREQ. The 40 MHz crystal counts from zero.
        self.peripherals.modem_lpcon_phy_tick.modify_tick_conf(|w| {
            w.set_modem_pwr_tick_target(oer_mmio::Bits::constant::<39>());
        });
    }
}
