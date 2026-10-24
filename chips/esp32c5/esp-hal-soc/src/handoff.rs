//! The per-domain transfers of platform blocks from esp-hal's singletons to
//! the chip PAC: the random number generator, the reset cause and the
//! watchdogs.
//!
//! Each function consumes the domain's esp-hal singletons, which esp-hal
//! hands out once, and never returns them, so it runs at most once per
//! program and no esp-hal owner of the blocks remains. No function makes
//! an MMIO access.

use esp_hal::{
    clock::xtal_clock,
    peripherals::{LP_CLKRST, LP_PERI, LP_WDT, RNG, TIMG0, TIMG1},
};
use oer_esp32c5_hal::platform::{
    EntropyHandoff, EntropyRegisters, ResetCauseHandoff, ResetCauseRegisters,
};
use oer_esp32c5_hal::watchdog::{MicrosecondTick, WatchdogBlocks, WatchdogHandoff, Watchdogs};

/// Transfer the random number generator's clock gate, configuration and
/// output.
///
/// esp-hal does not enable the ESP32-C5 generator; the HAL's entropy owner
/// enables it.
pub fn take_entropy(rng: RNG<'static>, lp_peri: LP_PERI<'static>) -> EntropyRegisters {
    let _consumed = (rng, lp_peri);
    #[allow(
        unsafe_code,
        reason = "the consumed esp-hal owners prove the permanent entropy handoff"
    )]
    // SAFETY: esp-hal issues `RNG` and `LP_PERI` (both the `LPPERI` block)
    // once; both are consumed above and never returned, so this runs at
    // most once. esp-hal writes no `RNG_CFG`; its other writers of
    // `CLK_EN` are reviewed in `registers/shared-words.toml`, and its `Rng`
    // reads the separate `RNG_DATA` word.
    let handoff = unsafe { EntropyHandoff::assume_exclusive() };
    EntropyRegisters::from_handoff(handoff)
}

/// Transfer the HP core's reset-cause word.
pub fn take_reset_cause(lp_clkrst: LP_CLKRST<'static>) -> ResetCauseRegisters {
    let _consumed = lp_clkrst;
    #[allow(
        unsafe_code,
        reason = "the consumed esp-hal owner proves the permanent reset-cause handoff"
    )]
    // SAFETY: esp-hal issues `LP_CLKRST` once; it is consumed above and
    // never returned, so this runs at most once. esp-hal's only reader of
    // the cause word is the ROM getter behind `reset_reason`, which no image
    // calls, and esp-hal does not write it.
    let handoff = unsafe { ResetCauseHandoff::assume_exclusive() };
    ResetCauseRegisters::from_handoff(handoff)
}

/// Transfer the watchdogs once and own them, the main watchdogs counting
/// microseconds of the XTAL clock esp-hal detected.
///
/// `esp_hal::init` stopped every watchdog; [`Watchdogs::new`] stops both
/// main watchdogs again, latched.
pub fn take_watchdogs(
    timg0: TIMG0<'static>,
    timg1: TIMG1<'static>,
    lp_wdt: LP_WDT<'static>,
) -> Watchdogs {
    let _consumed = (timg0, timg1, lp_wdt);
    #[allow(
        unsafe_code,
        reason = "the consumed esp-hal owners prove the permanent watchdog handoff"
    )]
    // SAFETY: esp-hal issues `TIMG0`, `TIMG1` and `LP_WDT` once; all three
    // are consumed above and never returned, so this runs at most once.
    // esp-hal writes the watchdog words only in `esp_hal::init`, which has
    // returned, and in its `TimerGroup`, `Wdt` and `Rtc` drivers: `TimerGroup`
    // needs a consumed timer group, `Rtc` is built only inside `init`, and
    // no image calls `Wdt::new` or `Wdt::set_wdt_enabled` on its own. The
    // RTC calibration words of timer group 0 that esp-hal's clock code
    // writes are not part of the watchdog blocks.
    let handoff = unsafe { WatchdogHandoff::assume_exclusive() };
    // SOURCE: esp-hal 1f646c5 `esp-hal/src/clock/mod.rs:571-574`
    // (`xtal_clock`, the 40 or 48 MHz XTAL `esp_hal::init` detected).
    #[expect(
        clippy::expect_used,
        reason = "esp-hal detects a 40 or 48 MHz XTAL at init, both whole megahertz"
    )]
    let tick =
        MicrosecondTick::from_source_hz(xtal_clock().as_hz()).expect("the C5 XTAL is 40 or 48 MHz");
    Watchdogs::new(WatchdogBlocks::from_handoff(handoff), tick)
}
