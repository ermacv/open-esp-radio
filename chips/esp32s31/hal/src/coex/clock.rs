//! The coexistence clock port of the Espressif policy over this HAL.
//!
//! The HAL owns register access and reports decoded clock observations. This
//! binding maps those observations into the executor-neutral
//! [`CoexClockHardware`] port of `oer-espressif-coex`.

use oer_espressif_coex::{CoexClockHardware, CoexClockSelector, CoexError, CoexTimerClock};

use crate::{
    coex::CoexTimerBank,
    ieee80211::mac::WifiMacColdHal,
    types::{CoexistenceLowPowerClockObservation, CoexistenceLowPowerClockSource},
};

/// Main crystal frequency of every supported ESP32-S31 board profile.
const XTAL_MHZ: u32 = 40;

/// Convert one decoded clock sample into the timer clock used by the core.
///
/// `real_chip` selects the silicon Selector8 frequency; the alternative is the
/// vendor's FPGA constant retained for compiled comparison.
pub fn timer_clock(
    observation: Option<CoexistenceLowPowerClockObservation>,
    real_chip: bool,
) -> Result<CoexTimerClock, CoexError> {
    let observation = observation.ok_or(CoexError::UnsupportedClock)?;
    let selector = match observation.source {
        CoexistenceLowPowerClockSource::Selector1 => CoexClockSelector::Selector1,
        CoexistenceLowPowerClockSource::Selector2 => CoexClockSelector::Selector2,
        CoexistenceLowPowerClockSource::Selector4 => CoexClockSelector::Selector4,
        CoexistenceLowPowerClockSource::Selector8 => CoexClockSelector::Selector8,
    };
    Ok(CoexTimerClock::from_hardware_fields(
        selector,
        observation.divider_minus_one,
        XTAL_MHZ,
        real_chip,
    ))
}

impl CoexClockHardware for WifiMacColdHal<'_> {
    fn sample(&mut self) -> Result<CoexTimerClock, CoexError> {
        timer_clock(self.sample_coexistence_low_power_clock(), true)
    }
}

impl CoexClockHardware for CoexTimerBank<'_> {
    fn sample(&mut self) -> Result<CoexTimerClock, CoexError> {
        timer_clock(self.sample_low_power_clock(), true)
    }
}

#[cfg(test)]
mod tests;
