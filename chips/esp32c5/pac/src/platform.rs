//! Platform blocks the image takes from esp-hal one domain at a time: the
//! random number generator and the reset cause.
//!
//! Each domain has a handoff proof, made once by the esp-hal adapter that
//! consumed the domain's esp-hal singletons, and an owner built from it that
//! claims only that domain's typed blocks.

use oer_mmio::{Bits, Fixed, Placement};

use crate::generated::{
    lp_clkrst_reset_cause::{self, LpClkrstResetCause},
    lpperi_rng::{self, LpperiRng},
    peripheral_ownership::{
        EntropyPeripherals, EntropyPeripheralsClaims, ResetCausePeripherals,
        ResetCausePeripheralsClaims,
    },
};

#[cfg(test)]
mod tests;

/// Proof that the platform transferred esp-hal's `RNG` and `LP_PERI`
/// singletons.
#[must_use = "the entropy owner needs the consumed platform ownership"]
pub struct EntropyHandoff(());

impl EntropyHandoff {
    /// Assert the platform's unique, permanent entropy handoff.
    ///
    /// # Safety
    /// Call at most once. The caller consumed esp-hal's `RNG` and `LP_PERI`
    /// singletons, and no other code writes `LPPERI.RNG_CFG` or the
    /// generator's gate in `LPPERI.CLK_EN` thereafter.
    #[allow(unsafe_code, reason = "the platform proves exclusive RNG ownership")]
    pub const unsafe fn assume_exclusive() -> Self {
        Self(())
    }
}

/// Where the device's generator words lie.
pub type RngPlacement = Fixed<{ lpperi_rng::BASE }>;

/// The random number generator's clock gate, configuration and output.
#[must_use = "dropping a register owner permanently loses its register authority"]
pub struct EntropyRegisters<P: Placement = RngPlacement>(LpperiRng<P>);

impl EntropyRegisters {
    /// Claim the generator's block from the platform handoff.
    #[allow(
        unsafe_code,
        reason = "the consumed handoff proves unique register ownership"
    )]
    pub fn from_handoff(_handoff: EntropyHandoff) -> Self {
        #[allow(
            clippy::disallowed_methods,
            reason = "the chip PAC's root claims a domain's typed blocks once, from its handoff"
        )]
        // SAFETY: `EntropyHandoff`'s constructor guarantees exclusive,
        // permanent ownership of `RNG_CFG`, `RNG_DATA_SYNC` and the
        // generator's gate in `CLK_EN`. The generated `const` assertion
        // beside the typed blocks checks that no other typed block lists
        // them. esp-hal's other writers of `CLK_EN` are reviewed in
        // `registers/shared-words.toml`.
        let claims = unsafe { oer_mmio::assume_owned::<EntropyPeripheralsClaims>() };
        Self(EntropyPeripherals::from_claims(claims).lpperi_rng)
    }
}

/// The generator timer's clock divider ESP-IDF configures.
///
/// SOURCE: ESP-IDF 4d59230d
/// `components/hal/esp32c5/include/hal/rng_ll.h:19` (`RNG_LL_CFG_PSCALE`).
const TIMER_PRESCALER: u8 = 255;

impl<P: Placement> EntropyRegisters<P> {
    /// Enable the generator as ESP-IDF does: its clock, then sampling with
    /// the generator timer and the RTC timer mixed in before and after the
    /// CRC. Each change keeps every other bit of its word, so the other
    /// low-power clock gates in `CLK_EN` stay as they are.
    ///
    /// SOURCE: ESP-IDF 4d59230d
    /// `components/hal/esp32c5/include/hal/rng_ll.h:89-96`
    /// (`rng_ll_enable`), called by `init_rng` in
    /// `components/esp_hw_support/hw_random.c:115-119`.
    pub fn enable(&mut self) {
        self.0.modify_clk_en(|clock| {
            clock.set_rng_ck_en(true);
        });
        self.0.modify_rng_cfg(|config| {
            config
                .set_rng_timer_pscale(TIMER_PRESCALER)
                .set_rng_sample_enable(true)
                .set_rtc_timer_en(Bits::constant::<0b11>())
                .set_rng_timer_en(true);
        });
    }

    /// One 32-bit word of the generator's synchronized output.
    ///
    /// SOURCE: ESP-IDF 4d59230d
    /// `components/hal/esp32c5/include/hal/rng_ll.h:32`
    /// (`rng_ll_read_data` reads `LPPERI_RNG_DATA_SYNC_REG`).
    pub fn random_word(&self) -> u32 {
        self.0.read_rng_data_sync().rnd_sync_data()
    }
}

/// Proof that the platform transferred esp-hal's `LP_CLKRST` singleton.
#[must_use = "the reset-cause owner needs the consumed platform ownership"]
pub struct ResetCauseHandoff(());

impl ResetCauseHandoff {
    /// Assert the platform's unique, permanent reset-cause handoff.
    ///
    /// # Safety
    /// Call at most once. The caller consumed esp-hal's `LP_CLKRST`
    /// singleton, and no other code accesses the reset-cause word
    /// thereafter.
    #[allow(
        unsafe_code,
        reason = "the platform proves exclusive reset-cause ownership"
    )]
    pub const unsafe fn assume_exclusive() -> Self {
        Self(())
    }
}

/// The HP core's reset-cause word.
#[must_use = "dropping a register owner permanently loses its register authority"]
pub struct ResetCauseRegisters<P: Placement = Fixed<{ lp_clkrst_reset_cause::BASE }>>(
    LpClkrstResetCause<P>,
);

impl ResetCauseRegisters {
    /// Claim the reset-cause block from the platform handoff.
    #[allow(
        unsafe_code,
        reason = "the consumed handoff proves unique register ownership"
    )]
    pub fn from_handoff(_handoff: ResetCauseHandoff) -> Self {
        #[allow(
            clippy::disallowed_methods,
            reason = "the chip PAC's root claims a domain's typed blocks once, from its handoff"
        )]
        // SAFETY: `ResetCauseHandoff`'s constructor guarantees exclusive,
        // permanent ownership of the reset-cause word. The generated `const`
        // assertion beside the typed blocks checks that no other typed block
        // lists it.
        let claims = unsafe { oer_mmio::assume_owned::<ResetCausePeripheralsClaims>() };
        Self(ResetCausePeripherals::from_claims(claims).lp_clkrst_reset_cause)
    }
}

impl<P: Placement> ResetCauseRegisters<P> {
    /// The cause code the HP core last left reset with, as its field holds
    /// it.
    pub fn cause_code(&self) -> u8 {
        self.0.read_reset_cause().reset_cause().get().to_le_bytes()[0]
    }
}

#[cfg(any(test, feature = "test-support"))]
impl<'a> EntropyRegisters<oer_mmio::Host<'a>> {
    /// The generator over host memory of at least [`lpperi_rng::SIZE`]
    /// bytes.
    pub fn host(words: &'a mut [u32]) -> Result<Self, oer_mmio::HostTooSmall> {
        LpperiRng::host(words).map(Self)
    }
}

#[cfg(any(test, feature = "test-support"))]
impl<'a> ResetCauseRegisters<oer_mmio::Host<'a>> {
    /// The reset-cause word over host memory of at least
    /// [`lp_clkrst_reset_cause::SIZE`] bytes.
    pub fn host(words: &'a mut [u32]) -> Result<Self, oer_mmio::HostTooSmall> {
        LpClkrstResetCause::host(words).map(Self)
    }
}
