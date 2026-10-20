//! The handoff from esp-hal's singletons and esp-hal's platform clocks.

use esp_hal::{
    clock::ll,
    peripherals::{BT, I2C_ANA_MST, IEEE802154, LP_TSENS, MODEM_LPCON, MODEM_SYSCON, WIFI},
    psram::Psram,
};
use oer_esp32s31_hal::{
    power::{PlatformClock, PlatformClockAbandonment},
    root::{AnalogBusOwnership, RadioHardware},
};

use crate::PlatformClocks;
use oer_esp32s31_hal::power::PlatformClockBackend;

/// esp-hal's singletons of the blocks the radio owns whole.
///
/// - `I2C_ANA_MST`: the analog-I2C master. esp-hal's only writer of the bus
///   after `esp_hal::init`, `rtc_cntl::brownout::configure`, borrows it.
/// - `MODEM_SYSCON`, `MODEM_LPCON`: the modem system and low-power clock
///   control. esp-hal still writes `MODEM_LPCON.CLK_CONF` through
///   `clock::ll`, on the radio's own requests ([`EspHalClocks`]).
/// - `WIFI`, `BT`, `IEEE802154`: the MAC blocks; consuming them excludes an
///   esp-hal-side radio driver in the same image.
/// - `LP_TSENS`: the temperature sensor the PHY reads.
pub struct RadioSingletons {
    pub i2c_ana_mst: I2C_ANA_MST<'static>,
    pub modem_syscon: MODEM_SYSCON<'static>,
    pub modem_lpcon: MODEM_LPCON<'static>,
    pub wifi: WIFI<'static>,
    pub bt: BT<'static>,
    pub ieee802154: IEEE802154<'static>,
    pub lp_tsens: LP_TSENS<'static>,
}

/// The radio root was not handed over.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RadioHandoffError {
    /// The radio PAC's register singleton was already taken.
    RootTaken,
}

/// The radio's platform clocks on esp-hal.
pub type EspHalClocks = PlatformClocks<EspHal>;

/// esp-hal's reference counts: the three `MODEM_LPCON.CLK_CONF` gates
/// through `clock::ll`'s counted acquire and release, MPLL and the 160 MHz PLL
/// output through the clock tree's request and release. The operations
/// stay in SRAM because the radio actor calls them at interrupt level 2.
pub struct EspHal;

impl PlatformClockBackend for EspHal {
    fn acquire(clock: PlatformClock) {
        match clock {
            PlatformClock::AnalogI2cMaster => ll::acquire_analog_i2c_master_clock(),
            PlatformClock::ModemCoexistence => ll::acquire_modem_coexistence_clock(),
            PlatformClock::ModemLowPowerTimer => ll::acquire_modem_low_power_timer_clock(),
            PlatformClock::Mpll => ll::ClockTree::with(ll::request_mpll_clk),
            PlatformClock::Pll160m => ll::ClockTree::with(ll::request_pll_f160m),
        }
    }

    fn release(clock: PlatformClock) {
        match clock {
            PlatformClock::AnalogI2cMaster => ll::release_analog_i2c_master_clock(),
            PlatformClock::ModemCoexistence => ll::release_modem_coexistence_clock(),
            PlatformClock::ModemLowPowerTimer => ll::release_modem_low_power_timer_clock(),
            PlatformClock::Mpll => ll::ClockTree::with(ll::release_mpll_clk),
            PlatformClock::Pll160m => ll::ClockTree::with(ll::release_pll_f160m),
        }
    }

    /// The reference stays in esp-hal's count, so the clock stays on: the
    /// fail-safe end of a guard that was not released.
    fn abandon(_clock: PlatformClock, _how: PlatformClockAbandonment) {}
}

/// Hand the radio blocks from esp-hal to the radio HAL.
///
/// `singletons` are consumed: esp-hal hands each out once, so this runs at
/// most once per program. `psram` proves that this image adopted PSRAM, whose
/// MPLL reference esp-hal never releases.
///
/// # Errors
///
/// [`RadioHandoffError::RootTaken`] when the radio PAC's register singleton
/// was already taken.
pub fn take_radio(
    singletons: RadioSingletons,
    psram: &Psram,
) -> Result<(RadioHardware, EspHalClocks), RadioHandoffError> {
    let _adopted: &Psram = psram;
    // The tokens are never handed back: no other owner of these blocks can
    // be constructed from esp-hal for the rest of the program.
    let _consumed: RadioSingletons = singletons;
    #[allow(
        unsafe_code,
        reason = "the consumed I2C_ANA_MST singleton is the analog-bus ownership the token asserts"
    )]
    // SAFETY: `I2C_ANA_MST` was consumed above, and esp-hal hands it out once,
    // so this runs at most once per program and no other owner of the
    // analog-I2C master exists or can be constructed safely: esp-hal's only
    // later writer of the bus, `rtc_cntl::brownout::configure`, needs the
    // singleton.
    let analog_bus = unsafe { AnalogBusOwnership::assume_exclusive() };
    let hardware = RadioHardware::take(analog_bus).ok_or(RadioHandoffError::RootTaken)?;
    Ok((hardware, EspHalClocks::new()))
}
