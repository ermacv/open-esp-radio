//! Platform blocks the image takes from esp-hal one domain at a time: the
//! eFuse MAC words, the true random number generator and the reset causes,
//! the system timer, the software interrupt lines, the interrupt matrix
//! with the CLIC, the USB Serial/JTAG console ([`usb_serial_jtag`]) and
//! core control ([`core_control`]).
//!
//! Each domain has a handoff proof, made once by the owner that consumed the
//! domain's esp-hal singletons, and an owner built from it that claims only
//! that domain's typed blocks. The eFuse, entropy and reset-cause owners only
//! read.

use oer_mmio::{Fixed, Placement};

use crate::generated::{
    efuse::{self, Efuse},
    lp_aon_reset_cause::{self, LpAonResetCause},
    lp_periclkrst_rng::{self, LpPericlkrstRng},
    peripheral_ownership::{
        EfusePeripherals, EfusePeripheralsClaims, EntropyPeripherals, EntropyPeripheralsClaims,
        ResetCausePeripherals, ResetCausePeripheralsClaims,
    },
    trng::{self, Trng},
};

pub mod core_control;
pub mod usb_serial_jtag;

pub use core_control::{
    CoreControlHandoff, CoreControlRegisters, Uart0ClockHandoff, Uart0ClockRegisters,
};

pub use usb_serial_jtag::{UsbSerialJtagHandoff, UsbSerialJtagPlacement, UsbSerialJtagRegisters};

/// Proof that the platform transferred the eFuse read words.
#[must_use = "the eFuse owner needs the platform's transfer"]
pub struct EfuseHandoff(());

impl EfuseHandoff {
    /// Assert the platform's unique, permanent eFuse handoff.
    ///
    /// # Safety
    /// Call at most once. No other code writes the eFuse read words
    /// thereafter, and only code that cannot disturb a read (esp-hal's
    /// eFuse readers) reads them.
    #[allow(unsafe_code, reason = "the platform proves exclusive eFuse ownership")]
    pub const unsafe fn assume_exclusive() -> Self {
        Self(())
    }
}

/// The eFuse controller's read words of the factory MAC.
#[must_use = "dropping a register owner permanently loses its register authority"]
pub struct EfuseRegisters<P: Placement = Fixed<{ efuse::BASE }>>(pub Efuse<P>);

impl EfuseRegisters {
    /// Claim the eFuse block from the platform handoff.
    #[allow(
        unsafe_code,
        reason = "the consumed handoff proves unique register ownership"
    )]
    pub fn from_handoff(_handoff: EfuseHandoff) -> Self {
        #[allow(
            clippy::disallowed_methods,
            reason = "the chip PAC's root claims a domain's typed blocks once, from its handoff"
        )]
        // SAFETY: `EfuseHandoff`'s constructor guarantees exclusive, permanent
        // ownership of the eFuse read words. The generated `const` assertion
        // beside the typed blocks checks that no other typed block lists them.
        let claims = unsafe { oer_mmio::assume_owned::<EfusePeripheralsClaims>() };
        Self(EfusePeripherals::from_claims(claims).efuse)
    }
}

/// Proof that the platform transferred esp-hal's `RNG` and `LP_PERI`
/// singletons.
#[must_use = "the entropy owner needs the consumed platform ownership"]
pub struct EntropyHandoff(());

impl EntropyHandoff {
    /// Assert the platform's unique, permanent entropy handoff.
    ///
    /// # Safety
    /// Call at most once. The caller consumed esp-hal's `RNG` and `LP_PERI`
    /// singletons, and no other code accesses the TRNG words or
    /// `LP_PERICLKRST.RNG_CTRL` thereafter.
    #[allow(unsafe_code, reason = "the platform proves exclusive TRNG ownership")]
    pub const unsafe fn assume_exclusive() -> Self {
        Self(())
    }
}

/// Where the device's TRNG block lies.
pub type TrngPlacement = Fixed<{ trng::BASE }>;

/// Where the device's `RNG_CTRL` word lies.
pub type RngClockPlacement = Fixed<{ lp_periclkrst_rng::BASE }>;

/// The true random number generator, its clock and reset word.
#[must_use = "dropping a register owner permanently loses its register authority"]
pub struct EntropyRegisters<P: Placement = TrngPlacement, Q: Placement = RngClockPlacement> {
    pub trng: Trng<P>,
    pub clock: LpPericlkrstRng<Q>,
}

impl EntropyRegisters {
    /// Claim the TRNG blocks from the platform handoff.
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
        // permanent ownership of the TRNG words and of `RNG_CTRL`. The
        // generated `const` assertion beside the typed blocks checks that no
        // other typed block lists them. esp-hal writes them only inside
        // `esp_hal::init` and `TrngSource::new`, reviewed in
        // `registers/shared-words.toml`.
        let claims = unsafe { oer_mmio::assume_owned::<EntropyPeripheralsClaims>() };
        let EntropyPeripherals {
            trng,
            lp_periclkrst_rng,
        } = EntropyPeripherals::from_claims(claims);
        Self {
            trng,
            clock: lp_periclkrst_rng,
        }
    }
}

/// Proof that the platform transferred esp-hal's `LP_AON_CLK_RST` singleton.
#[must_use = "the reset-cause owner needs the consumed platform ownership"]
pub struct ResetCauseHandoff(());

impl ResetCauseHandoff {
    /// Assert the platform's unique, permanent reset-cause handoff.
    ///
    /// # Safety
    /// Call at most once. The caller consumed esp-hal's `LP_AON_CLK_RST`
    /// singleton, and no other code accesses the reset-cause words
    /// thereafter.
    #[allow(
        unsafe_code,
        reason = "the platform proves exclusive reset-cause ownership"
    )]
    pub const unsafe fn assume_exclusive() -> Self {
        Self(())
    }
}

/// The reset-cause words of both HP cores.
#[must_use = "dropping a register owner permanently loses its register authority"]
pub struct ResetCauseRegisters<P: Placement = Fixed<{ lp_aon_reset_cause::BASE }>>(
    pub LpAonResetCause<P>,
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
        // permanent ownership of the reset-cause words. The generated `const`
        // assertion beside the typed blocks checks that no other typed block
        // lists them.
        let claims = unsafe { oer_mmio::assume_owned::<ResetCausePeripheralsClaims>() };
        Self(ResetCausePeripherals::from_claims(claims).lp_aon_reset_cause)
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
    interrupt_core1::{self, InterruptCore1},
    interrupt_core1_status::{self, InterruptCore1Status},
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
    /// Proof that the interrupt controller's owner consumed esp-hal's
    /// interrupt-matrix singletons after esp-hal's `init`.
    InterruptMatrixHandoff => InterruptMatrixPeripherals: InterruptMatrixPeripheralsClaims,
    "The caller consumed esp-hal's `INTERRUPT_CORE0`, `INTERRUPT_CORE1` and `CLIC` singletons for good after esp-hal's `init`, whose `setup_interrupts` is esp-hal's last write of the route and line words; no image calls esp-hal's interrupt API, as `shared-words.toml` reviews. The chip HAL's interrupt controller is the words' one writer afterwards."
);

#[cfg(any(test, feature = "test-support"))]
impl<'a> EfuseRegisters<oer_mmio::Host<'a>> {
    /// The eFuse words over host memory of at least [`efuse::SIZE`] bytes.
    pub fn host(words: &'a mut [u32]) -> Result<Self, oer_mmio::HostTooSmall> {
        Efuse::host(words).map(Self)
    }
}

#[cfg(any(test, feature = "test-support"))]
impl<'a, 'b> EntropyRegisters<oer_mmio::Host<'a>, oer_mmio::Host<'b>> {
    /// The generator over host memory of at least [`trng::SIZE`] and
    /// [`lp_periclkrst_rng::SIZE`] bytes.
    pub fn host(trng: &'a mut [u32], clock: &'b mut [u32]) -> Result<Self, oer_mmio::HostTooSmall> {
        Ok(Self {
            trng: Trng::host(trng)?,
            clock: LpPericlkrstRng::host(clock)?,
        })
    }
}

#[cfg(any(test, feature = "test-support"))]
impl<'a> ResetCauseRegisters<oer_mmio::Host<'a>> {
    /// The reset-cause words over host memory of at least
    /// [`lp_aon_reset_cause::SIZE`] bytes.
    pub fn host(words: &'a mut [u32]) -> Result<Self, oer_mmio::HostTooSmall> {
        LpAonResetCause::host(words).map(Self)
    }
}
