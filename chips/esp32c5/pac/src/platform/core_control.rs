//! Core control of the one HP core: UART0's function clock, which the boot
//! ROM prints through after a software reset.
//!
//! One handoff proof makes the owner of the block. The HAL's cores module
//! makes it only after its holder's compare-and-swap made the fatal path the
//! word's one writer, for good.

use oer_mmio::{Bits, Fixed, Placement};

use crate::generated::{
    pcr_uart0_clock::{self, PcrUart0Clock},
    peripheral_ownership::{CoreControlPeripherals, CoreControlPeripheralsClaims},
};

#[cfg(test)]
mod tests;

/// Proof that the caller holds the core-control word alone.
#[must_use = "the core-control owner needs the platform's transfer"]
pub struct CoreControlHandoff(());

impl CoreControlHandoff {
    /// Assert that the caller holds the core-control word alone.
    ///
    /// # Safety
    /// While the owner built from this proof lives, no other code accesses
    /// the word: no owner built from another proof lives at the same time,
    /// and no esp-hal writer of it runs.
    #[allow(
        unsafe_code,
        reason = "the platform proves exclusive core-control ownership"
    )]
    pub const unsafe fn assume_exclusive() -> Self {
        Self(())
    }
}

/// The core-control word.
#[must_use = "dropping a register owner permanently loses its register authority"]
pub struct CoreControlRegisters<P: Placement = Fixed<{ pcr_uart0_clock::BASE }>>(PcrUart0Clock<P>);

impl CoreControlRegisters {
    /// Claim the block from the platform handoff.
    #[allow(
        unsafe_code,
        reason = "the consumed handoff proves unique register ownership"
    )]
    pub fn from_handoff(_handoff: CoreControlHandoff) -> Self {
        #[allow(
            clippy::disallowed_methods,
            reason = "the chip PAC's root claims a domain's typed blocks, from its handoff"
        )]
        // SAFETY: `CoreControlHandoff`'s constructor guarantees that no
        // other code accesses the word while this owner lives. The generated
        // `const` assertion beside the typed blocks checks that no other
        // typed block lists it.
        let claims = unsafe { oer_mmio::assume_owned::<CoreControlPeripheralsClaims>() };
        Self(CoreControlPeripherals::from_claims(claims).pcr_uart0_clock)
    }
}

impl<P: Placement> CoreControlRegisters<P> {
    /// Return UART0's function clock to its power-on state, XTAL undivided
    /// with its gate open: the boot ROM prints through UART0 after a
    /// software reset, and the source the firmware chose may not run any
    /// more.
    ///
    /// SOURCE: esp-hal 1f646c5 `esp-hal/src/uart/clocks/v2_pcr.rs`
    /// (`restore_power_on_function_clock`); ESP-IDF 4d59230d `pcr_reg.h`
    /// (`PCR_UART0_SCLK_SEL` 0 is XTAL_CLK, zero divider fields divide by
    /// one).
    pub fn restore_uart0_clock(&mut self) {
        self.0.modify_uart0_sclk_conf(|clock| {
            clock
                .set_uart0_sclk_sel(Bits::constant::<0>())
                .set_uart0_sclk_div_a(Bits::constant::<0>())
                .set_uart0_sclk_div_b(Bits::constant::<0>())
                .set_uart0_sclk_div_num(0)
                .set_uart0_sclk_en(true);
        });
    }
}

#[cfg(any(test, feature = "test-support"))]
impl<'a> CoreControlRegisters<oer_mmio::Host<'a>> {
    /// The block over host memory, which holds the words from the block's
    /// base to its last one.
    pub fn host(words: &'a mut [u32]) -> Result<Self, oer_mmio::HostTooSmall> {
        PcrUart0Clock::host(words).map(Self)
    }
}

/// The bytes of host memory the block needs.
#[cfg(any(test, feature = "test-support"))]
pub const HOST_SIZE: usize = pcr_uart0_clock::SIZE;
