//! Finite common modem/PHY power prerequisites and semantic verification.

/// Upstream references needed by the common radio power sequence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PowerClock {
    Mpll,
    Pll160m,
    AnalogI2cMaster,
}

/// The platform refused a reference without granting it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReferenceRefused;

pub trait PowerSequenceBackend {
    fn publish_modem_power_state(&mut self);
    fn enable_modem_register_bus_clock(&mut self);
    /// Take one platform clock reference for common power.
    fn acquire_platform_clock(&mut self, clock: PowerClock) -> Result<(), ReferenceRefused>;
    fn configure_modem_source_clocks(&mut self);
    fn set_wifi_baseband_reset(&mut self, asserted: bool);
    fn configure_wifi_power_clock_map(&mut self);
    fn enable_phy_calibration_clocks(&mut self);
    fn select_phy_i2c_160mhz_source(&mut self);
    fn prepare_shared_modem_clock_map(&mut self);
    fn power_readback(&self) -> PowerClockReadback;
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

pub fn execute_owned(registers: &mut impl PowerSequenceBackend) -> Result<(), PowerError> {
    // Wi-Fi's combined BB+MAC reset belongs to its client entry. The BB-only
    // pulse below is the PHY clock-enable prerequisite, repeated by the
    // vendor modem clock manager; it preserves the MAC calibration window.
    // The PHY writes front-end baseband power into the PMU word where the
    // platform powers MPLL; holding MPLL keeps the platform from writing it.
    registers
        .acquire_platform_clock(PowerClock::Mpll)
        .map_err(|ReferenceRefused| refused(PowerCheckpoint::Mpll))?;
    registers.publish_modem_power_state();
    registers.enable_modem_register_bus_clock();
    registers.configure_wifi_power_clock_map();
    registers.prepare_shared_modem_clock_map();
    // The platform owns the 160 MHz gate; its reference replaces the gate
    // write that ESP-IDF's modem clock performs first for this source.
    registers
        .acquire_platform_clock(PowerClock::Pll160m)
        .map_err(|ReferenceRefused| refused(PowerCheckpoint::Reference160m))?;
    registers.configure_modem_source_clocks();
    registers.set_wifi_baseband_reset(true);
    registers.set_wifi_baseband_reset(false);
    registers.enable_phy_calibration_clocks();
    registers.select_phy_i2c_160mhz_source();
    registers
        .acquire_platform_clock(PowerClock::AnalogI2cMaster)
        .map_err(|ReferenceRefused| refused(PowerCheckpoint::I2cClock))?;

    let readback = registers.power_readback();
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
mod tests;
