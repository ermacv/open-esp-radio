//! Closed MODEM_LPCON capability owned by the shared ESP32-S31 PHY route.
//!
//! Only the complete-libphy-evidenced `TICK_CONF` transition is present. The
//! remaining MODEM_LPCON clock and reset registers stay outside this owner
//! until their Wi-Fi, Bluetooth, coexistence, and IEEE 802.15.4 lifecycles
//! share one explicit clock capability.

#![forbid(unsafe_code)]

use crate::{RadioPhyRegisters, generated::ModemPowerTickTarget};

impl RadioPhyRegisters {
    /// Configure the modem-power tick for the fixed 40 MHz S31 crystal.
    ///
    /// Complete `phy_get_xtal_freq` replaces only the low six bits with
    /// `frequency_mhz - 1`; the generated transaction preserves every other
    /// bit of `TICK_CONF`.
    pub fn configure_fixed_xtal_40mhz_tick(&mut self) {
        crate::generated::configure_modem_power_tick_for_xtal_40mhz(
            &self.peripherals.modem_lpcon_phy_tick,
            ModemPowerTickTarget::Xtal40Mhz,
        );
    }
}
