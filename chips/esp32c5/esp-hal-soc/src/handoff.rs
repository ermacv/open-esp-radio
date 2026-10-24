//! The per-domain transfers of platform blocks from esp-hal's singletons to
//! the chip PAC: the random number generator and the reset cause.
//!
//! Each function consumes the domain's esp-hal singletons, which esp-hal
//! hands out once, and never returns them, so it runs at most once per
//! program and no esp-hal owner of the blocks remains. No function makes
//! an MMIO access.

use esp_hal::peripherals::{LP_CLKRST, LP_PERI, RNG};
use oer_esp32c5_hal::platform::{
    EntropyHandoff, EntropyRegisters, ResetCauseHandoff, ResetCauseRegisters,
};

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
