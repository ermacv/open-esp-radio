//! The per-domain transfers of platform blocks from esp-hal's singletons to
//! the chip PAC: the eFuse MAC words, the true random number generator, the
//! reset causes and the watchdogs.
//!
//! Each function consumes the domain's esp-hal singletons, which esp-hal
//! hands out once, and never returns them, so it runs at most once per
//! program and no esp-hal owner of the blocks remains. esp-hal hands out no
//! `EFUSE` singleton, so the eFuse transfer succeeds once per program by a
//! flag instead. No function makes an MMIO access.

use esp_hal::peripherals::{LP_AON_CLK_RST, LP_PERI, LP_WDT, RNG, TIMG0, TIMG1};
use oer_esp32s31_pac::platform::{
    EfuseHandoff, EfuseRegisters, EntropyHandoff, EntropyRegisters, ResetCauseHandoff,
    ResetCauseRegisters,
};
use oer_esp32s31_pac::watchdog::{WatchdogBlocks, WatchdogHandoff};

/// Whether the eFuse words were transferred.
static EFUSE_TAKEN: crate::once::Once = crate::once::Once::new();

/// Transfer the eFuse read words; `None` after the first transfer.
pub fn take_efuse() -> Option<EfuseRegisters> {
    if !EFUSE_TAKEN.first() {
        return None;
    }
    #[allow(
        unsafe_code,
        reason = "the once flag proves the permanent eFuse handoff"
    )]
    // SAFETY: the flag above lets this run once per program. esp-hal keeps
    // `EFUSE` hidden, hands out no owner of it and never writes the read
    // words; its readers (`esp_hal::efuse`, the chip revision in
    // `esp_hal::init`) only load them, which the owner's reads do not
    // disturb.
    let handoff = unsafe { EfuseHandoff::assume_exclusive() };
    Some(EfuseRegisters::from_handoff(handoff))
}

/// Transfer the true random number generator and its clock and reset word.
///
/// `esp_hal::init` enabled and configured the generator; its owner only
/// reads it.
pub fn take_entropy(rng: RNG<'static>, lp_peri: LP_PERI<'static>) -> EntropyRegisters {
    let _consumed = (rng, lp_peri);
    #[allow(
        unsafe_code,
        reason = "the consumed esp-hal owners prove the permanent entropy handoff"
    )]
    // SAFETY: esp-hal issues `RNG` and `LP_PERI` once; both are consumed
    // above and never returned, so this runs at most once. esp-hal's later
    // writers of the TRNG words (`TrngSource::new`) and readers (`Rng`,
    // `Trng`) are not called by any image; `TrngSource::new` needs `RNG`.
    let handoff = unsafe { EntropyHandoff::assume_exclusive() };
    EntropyRegisters::from_handoff(handoff)
}

/// Transfer the reset-cause words of both HP cores.
pub fn take_reset_causes(lp_aon_clk_rst: LP_AON_CLK_RST<'static>) -> ResetCauseRegisters {
    let _consumed = lp_aon_clk_rst;
    #[allow(
        unsafe_code,
        reason = "the consumed esp-hal owner proves the permanent reset-cause handoff"
    )]
    // SAFETY: esp-hal issues `LP_AON_CLK_RST` once; it is consumed above and
    // never returned, so this runs at most once. esp-hal's only reader of the
    // cause words is the ROM getter behind `reset_reason`, which no image
    // calls, and esp-hal does not write them.
    let handoff = unsafe { ResetCauseHandoff::assume_exclusive() };
    ResetCauseRegisters::from_handoff(handoff)
}

/// Transfer both timer groups' main watchdogs, timer group 1's clock and
/// reset word, and the RTC and super watchdogs.
///
/// `esp_hal::init` stopped every watchdog; the HAL's `Watchdogs` owns them
/// from here.
pub fn take_watchdogs(
    timg0: TIMG0<'static>,
    timg1: TIMG1<'static>,
    lp_wdt: LP_WDT<'static>,
) -> WatchdogBlocks {
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
    WatchdogBlocks::from_handoff(handoff)
}
