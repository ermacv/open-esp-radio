//! ESP-HAL clocks that the radio arbiter's modem clocks depend on.
//!
//! ESP-HAL owns the SoC clock tree and keeps its own reference counts: the
//! 160 MHz PLL output and MPLL through its clock-tree nodes, and the
//! `MODEM_LPCON.CLK_CONF` gates (analog-I2C master, coexistence and
//! low-power timer) under the one lock that also serves its regi2c
//! accesses. The radio arbiter reaches them only through
//! [`PlatformClockProvider`].

use esp_hal::clock::ll::{
    ClockTree, acquire_analog_i2c_master_clock, acquire_modem_coexistence_clock,
    acquire_modem_low_power_timer_clock, release_analog_i2c_master_clock,
    release_modem_coexistence_clock, release_modem_low_power_timer_clock, release_mpll_clk,
    release_pll_f160m, request_mpll_clk, request_pll_f160m,
};
use oer_esp32s31_hal::power::{
    PlatformClock, PlatformClockError, PlatformClockGuard, PlatformClockProvider,
};

/// Platform clock provider backed by ESP-HAL's reference-counted clocks.
///
/// It is a zero-size capability: ESP-HAL's counts are global, so each
/// [`PlatformClockGuard`] it returns gives its reference back through
/// [`PlatformClockGuard::release`] without borrowing the provider. An
/// abandoned or dropped guard keeps ESP-HAL's reference, and the provider
/// records nothing: the radio's one shared owner lives for the whole
/// program.
#[derive(Debug, Default)]
// CAPABILITY: whole-radio-cold-power-and-clocks-shared-clock-lifetime
pub struct EspHalRadioClocks {
    _private: (),
}

impl EspHalRadioClocks {
    pub const fn new() -> Self {
        Self { _private: () }
    }
}

fn release(clock: PlatformClock) {
    match clock {
        PlatformClock::Pll160m => ClockTree::with(release_pll_f160m),
        PlatformClock::Mpll => ClockTree::with(release_mpll_clk),
        PlatformClock::AnalogI2cMaster => release_analog_i2c_master_clock(),
        PlatformClock::ModemCoexistence => release_modem_coexistence_clock(),
        PlatformClock::ModemLowPowerTimer => release_modem_low_power_timer_clock(),
    }
}

impl PlatformClockProvider for EspHalRadioClocks {
    fn acquire(&self, clock: PlatformClock) -> Result<PlatformClockGuard, PlatformClockError> {
        match clock {
            PlatformClock::Pll160m => ClockTree::with(request_pll_f160m),
            PlatformClock::Mpll => ClockTree::with(request_mpll_clk),
            PlatformClock::AnalogI2cMaster => acquire_analog_i2c_master_clock(),
            PlatformClock::ModemCoexistence => acquire_modem_coexistence_clock(),
            PlatformClock::ModemLowPowerTimer => acquire_modem_low_power_timer_clock(),
        }
        // An abandoned or dropped guard keeps ESP-HAL's reference: the
        // radio's single shared owner lives for the whole program, so there
        // is nothing to record on the chip.
        Ok(PlatformClockGuard::new(clock, release, |_, _| {}))
    }
}
