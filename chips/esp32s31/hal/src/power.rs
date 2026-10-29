//! Owned S31 register and reference adapter for the family power driver.
use crate::{clock::PlatformClockRefs, phy::RadioPhyRegisters};

/// Power-sequence port over one route's shared PHY, its platform clock
/// references and the platform clock owner.
pub(crate) struct RoutePower<'route, P: PlatformClockProvider> {
    pub(crate) phy: &'route mut RadioPhyRegisters,
    pub(crate) refs: &'route mut PlatformClockRefs<P::Backend>,
    pub(crate) platform: &'route P,
}

impl<P: PlatformClockProvider> oer_espressif_radio::power::PowerSequenceBackend
    for RoutePower<'_, P>
{
    fn publish_modem_power_state(&mut self) {
        self.phy.publish_modem_power_state();
    }
    fn enable_modem_register_bus_clock(&mut self) {
        self.phy.enable_modem_register_bus_clock();
    }
    fn acquire_platform_clock(
        &mut self,
        clock: oer_espressif_radio::power::PowerClock,
    ) -> Result<(), oer_espressif_radio::power::ReferenceRefused> {
        use oer_espressif_radio::power::{PowerClock, ReferenceRefused};
        let clock = match clock {
            PowerClock::Mpll => PlatformClock::Mpll,
            PowerClock::Pll160m => PlatformClock::Pll160m,
            PowerClock::AnalogI2cMaster => PlatformClock::AnalogI2cMaster,
        };
        self.refs
            .acquire(clock, self.platform)
            .map_err(|_| ReferenceRefused)
    }
    fn configure_modem_source_clocks(&mut self) {
        self.phy.configure_modem_source_clocks();
    }
    fn set_wifi_baseband_reset(&mut self, asserted: bool) {
        self.phy.set_wifi_baseband_reset(asserted);
    }
    fn configure_wifi_power_clock_map(&mut self) {
        self.phy.configure_wifi_power_clock_map();
    }
    fn enable_phy_calibration_clocks(&mut self) {
        self.phy.enable_phy_calibration_clocks();
    }
    fn select_phy_i2c_160mhz_source(&mut self) {
        self.phy.select_phy_i2c_160mhz_source();
    }
    fn prepare_shared_modem_clock_map(&mut self) {
        self.phy.prepare_shared_modem_clock_map();
    }
    fn power_readback(&self) -> oer_espressif_radio::power::PowerClockReadback {
        let platform = self.phy.platform_clock_power_observation();
        let modem = self.phy.modem_syscon_power_observation();
        let shared = self.phy.shared_modem_clock_observation();
        oer_espressif_radio::power::PowerClockReadback {
            reset_released: modem.wifi_reset_released,
            hp_active_icg_selected: platform.hp_active_icg_selected,
            modem_bus_clock_enabled: platform.modem_register_bus_clock_enabled,
            hp_active_clock_map_configured: modem.active_clock_map_configured,
            shared_clock_map_configured: shared.power_state_map_configured,
            modem_source_clocks_configured: platform.modem_source_clocks_configured,
            phy_calibration_clocks_enabled: modem.phy_calibration_clocks_enabled,
            phy_i2c_160mhz_selected: modem.phy_i2c_160mhz_selected,
        }
    }
}

mod chip_clocks;
pub(crate) mod clock;
#[cfg(test)]
pub(crate) mod test_clocks;
pub use chip_clocks::{ChipClocks, PsramMpll};
pub use clock::{
    PlatformClock, PlatformClockAbandonment, PlatformClockBackend, PlatformClockError,
    PlatformClockGuard, PlatformClockHolds, PlatformClockProvider, PlatformClockReleaseRefused,
};
