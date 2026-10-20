//! Finite ESP32-S31 modem/PHY prerequisite sequence.
//!
//! Domain clock prerequisites shared by all radio protocols. Wi-Fi's cold
//! BB+MAC reset belongs to its client entry after PHY calibration; this port
//! cannot issue it. Wi-Fi MAC clocks belong to the later MAC start transition.

use oer_esp32s31_pac::{
    ModemSysconPowerObservation, PlatformClockPowerObservation, RadioPhyRegisters,
    SharedModemClockObservation,
};

use crate::clock::PlatformClockRefs;

pub(crate) trait PowerSequenceBackend {
    fn select_hp_active_modem_icg(&mut self);
    fn apply_modem_icg_selection(&mut self);
    fn apply_sleep_icg_selection(&mut self);
    fn enable_modem_register_bus_clock(&mut self);
    /// Take one platform clock reference for common power.
    fn acquire_platform_clock(&mut self, clock: PlatformClock) -> Result<(), PlatformClockError>;
    fn configure_modem_source_clocks(&mut self);
    fn platform_clock_power_observation(&self) -> oer_esp32s31_pac::PlatformClockPowerObservation;
    fn set_wifi_baseband_reset(&mut self, asserted: bool);
    fn configure_wifi_power_clock_map(&mut self);
    fn enable_phy_calibration_clocks(&mut self);
    fn select_phy_i2c_160mhz_source(&mut self);
    fn modem_syscon_power_observation(&self) -> oer_esp32s31_pac::ModemSysconPowerObservation;
    fn prepare_shared_modem_clock_map(&mut self);
    fn shared_modem_clock_observation(&self) -> oer_esp32s31_pac::SharedModemClockObservation;
}

/// Power-sequence port over one route's shared PHY, its platform clock
/// references and the platform clock owner.
pub(crate) struct RoutePower<'route, P: PlatformClockProvider> {
    pub(crate) phy: &'route mut RadioPhyRegisters,
    pub(crate) refs: &'route mut PlatformClockRefs<P::Backend>,
    pub(crate) platform: &'route P,
}

impl<P: PlatformClockProvider> PowerSequenceBackend for RoutePower<'_, P> {
    fn select_hp_active_modem_icg(&mut self) {
        self.phy.select_hp_active_modem_icg();
    }
    fn apply_modem_icg_selection(&mut self) {
        self.phy.apply_modem_icg_selection();
    }
    fn apply_sleep_icg_selection(&mut self) {
        self.phy.apply_sleep_icg_selection();
    }
    fn enable_modem_register_bus_clock(&mut self) {
        self.phy.enable_modem_register_bus_clock();
    }
    fn acquire_platform_clock(&mut self, clock: PlatformClock) -> Result<(), PlatformClockError> {
        self.refs.acquire(clock, self.platform)
    }
    fn configure_modem_source_clocks(&mut self) {
        self.phy.configure_modem_source_clocks();
    }
    fn platform_clock_power_observation(&self) -> PlatformClockPowerObservation {
        self.phy.platform_clock_power_observation()
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
    fn modem_syscon_power_observation(&self) -> ModemSysconPowerObservation {
        self.phy.modem_syscon_power_observation()
    }
    fn prepare_shared_modem_clock_map(&mut self) {
        self.phy.prepare_shared_modem_clock_map();
    }
    fn shared_modem_clock_observation(&self) -> SharedModemClockObservation {
        self.phy.shared_modem_clock_observation()
    }
}

/// Semantic read-back state captured after the cold clock/reset sequence.
///
/// Route-owned field decoding stays in the custom PAC; remaining platform
/// decoding stays behind the integration capability. Neither register handles,
/// addresses nor system-register masks cross into this layer.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PowerClockReadback {
    pub reset_released: bool,
    pub hp_active_icg_selected: bool,
    pub modem_bus_clock_enabled: bool,
    pub hp_active_clock_map_configured: bool,
    pub shared_clock_map_configured: bool,
    pub modem_source_clocks_configured: bool,
    pub phy_calibration_clocks_enabled: bool,
    pub phy_i2c_160mhz_selected: bool,
}

/// Read-back checkpoint following the finite prerequisite sequence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PowerCheckpoint {
    /// The platform clock owner granted the MPLL reference.
    Mpll,
    /// Both Wi-Fi reset lines were released.
    ResetReleased,
    /// PMU selects the HP-active modem ICG code.
    HpActiveIcg,
    /// The modem register bus clock is enabled.
    ModemBusClock,
    /// HP-active modem domain clocks are ungated.
    HpActiveClockMap,
    /// Shared low-power modem clocks are ungated.
    SharedClockMap,
    /// The platform clock owner granted the 160 MHz reference.
    Reference160m,
    /// The modem PLL/XTAL source configuration is active.
    ModemClockSource,
    /// All PHY frontend and calibration clocks are enabled.
    PhyClocks,
    /// PHY-I²C uses the 160 MHz source.
    I2cSource,
    /// The platform clock owner granted the PHY-I²C master clock.
    I2cClock,
}

/// A prerequisite failed its bounded semantic read-back checkpoint.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PowerError {
    /// Semantic checkpoint that failed.
    pub checkpoint: PowerCheckpoint,
    /// Expected semantic state.
    pub expected: bool,
    /// Observed semantic state.
    pub observed: bool,
}

pub(crate) fn execute_owned(registers: &mut impl PowerSequenceBackend) -> Result<(), PowerError> {
    // Wi-Fi's combined BB+MAC reset belongs to its client entry. The BB-only
    // pulse below is the PHY clock-enable prerequisite, repeated by the
    // vendor modem clock manager; it preserves the MAC calibration window.
    // The PHY writes front-end baseband power into the PMU word where the
    // platform powers MPLL; holding MPLL keeps the platform from writing it.
    registers
        .acquire_platform_clock(PlatformClock::Mpll)
        .map_err(|PlatformClockError| refused(PowerCheckpoint::Mpll))?;
    registers.select_hp_active_modem_icg();
    registers.apply_modem_icg_selection();
    registers.apply_sleep_icg_selection();
    registers.enable_modem_register_bus_clock();
    registers.configure_wifi_power_clock_map();
    registers.prepare_shared_modem_clock_map();
    // The platform owns the 160 MHz gate; its reference replaces the gate
    // write that ESP-IDF's modem clock performs first for this source.
    registers
        .acquire_platform_clock(PlatformClock::Pll160m)
        .map_err(|PlatformClockError| refused(PowerCheckpoint::Reference160m))?;
    registers.configure_modem_source_clocks();
    registers.set_wifi_baseband_reset(true);
    registers.set_wifi_baseband_reset(false);
    registers.enable_phy_calibration_clocks();
    registers.select_phy_i2c_160mhz_source();
    registers
        .acquire_platform_clock(PlatformClock::AnalogI2cMaster)
        .map_err(|PlatformClockError| refused(PowerCheckpoint::I2cClock))?;

    let platform = registers.platform_clock_power_observation();
    let modem = registers.modem_syscon_power_observation();
    let shared = registers.shared_modem_clock_observation();
    let readback = PowerClockReadback {
        reset_released: modem.wifi_reset_released,
        hp_active_icg_selected: platform.hp_active_icg_selected,
        modem_bus_clock_enabled: platform.modem_register_bus_clock_enabled,
        hp_active_clock_map_configured: modem.active_clock_map_configured,
        shared_clock_map_configured: shared.power_state_map_configured,
        modem_source_clocks_configured: platform.modem_source_clocks_configured,
        phy_calibration_clocks_enabled: modem.phy_calibration_clocks_enabled,
        phy_i2c_160mhz_selected: modem.phy_i2c_160mhz_selected,
    };
    verify_state(PowerCheckpoint::ResetReleased, readback.reset_released)?;
    verify_state(
        PowerCheckpoint::HpActiveIcg,
        readback.hp_active_icg_selected,
    )?;
    verify_state(
        PowerCheckpoint::ModemBusClock,
        readback.modem_bus_clock_enabled,
    )?;
    verify_state(
        PowerCheckpoint::HpActiveClockMap,
        readback.hp_active_clock_map_configured,
    )?;
    verify_state(
        PowerCheckpoint::SharedClockMap,
        readback.shared_clock_map_configured,
    )?;
    verify_state(
        PowerCheckpoint::ModemClockSource,
        readback.modem_source_clocks_configured,
    )?;
    verify_state(
        PowerCheckpoint::PhyClocks,
        readback.phy_calibration_clocks_enabled,
    )?;
    verify_state(PowerCheckpoint::I2cSource, readback.phy_i2c_160mhz_selected)
}

/// The platform clock owner refused a reference at `checkpoint`.
const fn refused(checkpoint: PowerCheckpoint) -> PowerError {
    PowerError {
        checkpoint,
        expected: true,
        observed: false,
    }
}

fn verify_state(checkpoint: PowerCheckpoint, observed: bool) -> Result<(), PowerError> {
    if observed {
        Ok(())
    } else {
        Err(PowerError {
            checkpoint,
            expected: true,
            observed,
        })
    }
}

#[cfg(test)]
pub(crate) mod test_clocks;

#[cfg(test)]
mod tests;

pub(crate) mod clock;
pub use clock::{
    PlatformClock, PlatformClockAbandonment, PlatformClockBackend, PlatformClockError,
    PlatformClockGuard, PlatformClockHolds, PlatformClockProvider,
};
