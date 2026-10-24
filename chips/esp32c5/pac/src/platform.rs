//! Platform blocks the image takes from esp-hal one domain at a time: the
//! random number generator and the reset cause, the system timer, the
//! software interrupt lines, the interrupt matrix with the CLIC, and the USB
//! Serial/JTAG console ([`usb_serial_jtag`]).
//!
//! Each domain has a handoff proof, made once by the owner that consumed the
//! domain's esp-hal singletons, and an owner built from it that claims only
//! that domain's typed blocks.

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

pub mod usb_serial_jtag;

pub use usb_serial_jtag::{
    UsbEndpointRegisters, UsbEvents, UsbInterruptRegisters, UsbPadRegisters, UsbSerialJtagHandoff,
    UsbSerialJtagPlacement, UsbSerialJtagRegisters,
};

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

// The step-4 domains: the executor's system timer and software
// interrupt lines, and the interrupt table's matrix and CLIC.
pub use crate::generated::{
    clic::{self, Clic},
    from_cpu_0::{self, FromCpu0},
    from_cpu_1::{self, FromCpu1},
    from_cpu_2::{self, FromCpu2},
    from_cpu_3::{self, FromCpu3},
    interrupt_core0::{self, InterruptCore0},
    interrupt_core0_status::{self, InterruptCore0Status},
    peripheral_ownership::{
        InterruptMatrixPeripherals, SoftwareInterrupt0Peripherals, SoftwareInterrupt1Peripherals,
        SoftwareInterrupt2Peripherals, SoftwareInterrupt3Peripherals, SystemTimerPeripherals,
    },
    systimer::{self, Systimer},
    systimer_acknowledge::{self, SystimerAcknowledge},
    systimer_counter::{self, SystimerCounter},
};

/// Declare a platform domain's handoff proof and its partition's claim.
macro_rules! handoff {
    ($(#[$doc:meta])* $handoff:ident => $partition:ident: $claims:ident, $contract:literal) => {
        $(#[$doc])*
        #[must_use = "the platform blocks need the consumed esp-hal owner"]
        pub struct $handoff(());

        impl $handoff {
            /// Assert the platform's unique, permanent handoff of the domain.
            ///
            /// # Safety
            /// Call at most once.
            #[doc = $contract]
            #[allow(
                unsafe_code,
                reason = "the platform proves exclusive ownership of the domain's words"
            )]
            pub const unsafe fn assume_exclusive() -> Self {
                Self(())
            }
        }

        impl crate::generated::peripheral_ownership::$partition {
            /// Claim the domain's blocks once, from its handoff.
            #[allow(
                unsafe_code,
                reason = "the consumed handoff proves unique register ownership"
            )]
            pub fn from_handoff(_handoff: $handoff) -> Self {
                #[allow(
                    clippy::disallowed_methods,
                    reason = "a platform handoff claims its domain's typed blocks once"
                )]
                // SAFETY: the handoff's constructor guarantees that its
                // domain's words are claimed once and that no other code
                // writes them but the esp-hal writers that
                // `registers/shared-words.toml` reviews with their
                // coordination. The generated `const` assertion checks that
                // no two typed blocks list the same word.
                let claims = unsafe {
                    oer_mmio::assume_owned::<crate::generated::peripheral_ownership::$claims>()
                };
                Self::from_claims(claims)
            }
        }
    };
}

handoff!(
    /// Proof that the time driver consumed esp-hal's `SYSTIMER` singleton.
    SystemTimerHandoff => SystemTimerPeripherals: SystemTimerPeripheralsClaims,
    "The caller consumed esp-hal's `SYSTIMER` singleton for good, so no esp-hal `SystemTimer`, unit or alarm exists; esp-hal's `Instant::now` strobes the counter latch as `shared-words.toml` reviews."
);
handoff!(
    /// Proof that the owner of software interrupt line 0 consumed esp-hal's
    /// `SoftwareInterrupt<0>`.
    SoftwareInterrupt0Handoff => SoftwareInterrupt0Peripherals: SoftwareInterrupt0PeripheralsClaims,
    "The caller consumed esp-hal's `SoftwareInterrupt<0>` for good, the only esp-hal writer of the line."
);
handoff!(
    /// Proof that the owner of software interrupt line 1 consumed esp-hal's
    /// `SoftwareInterrupt<1>`.
    SoftwareInterrupt1Handoff => SoftwareInterrupt1Peripherals: SoftwareInterrupt1PeripheralsClaims,
    "The caller consumed esp-hal's `SoftwareInterrupt<1>` for good, the only esp-hal writer of the line."
);
handoff!(
    /// Proof that the owner of software interrupt line 2 consumed esp-hal's
    /// `SoftwareInterrupt<2>`.
    SoftwareInterrupt2Handoff => SoftwareInterrupt2Peripherals: SoftwareInterrupt2PeripheralsClaims,
    "The caller consumed esp-hal's `SoftwareInterrupt<2>` for good, the only esp-hal writer of the line."
);
handoff!(
    /// Proof that the owner of software interrupt line 3 consumed esp-hal's
    /// `SoftwareInterrupt<3>`.
    SoftwareInterrupt3Handoff => SoftwareInterrupt3Peripherals: SoftwareInterrupt3PeripheralsClaims,
    "The caller consumed esp-hal's `SoftwareInterrupt<3>` for good, the only esp-hal writer of the line."
);
handoff!(
    /// Proof that the image's interrupt table took esp-hal's routing
    /// capability after esp-hal's vectoring.
    InterruptMatrixHandoff => InterruptMatrixPeripherals: InterruptMatrixPeripheralsClaims,
    "The caller took esp-hal's one `InterruptRoutes` and keeps each core's routes beside that core's matrix owner, and esp-hal's vectoring of this boot has run: esp-hal still writes the route and line words only as `shared-words.toml` reviews."
);

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
