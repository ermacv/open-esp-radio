//! The bootstrap's epoch of the PLL power fields.
//!
//! The PMU's active-state clock-power word (`HP_ACTIVE_HP_CK_POWER`) holds
//! the CPLL and MPLL power and their analog-I2C power, which the bootstrap
//! sets before it programs the CPU clock and the PSRAM clock, and the
//! front-end clock image and baseband analog-I2C power, which the radio
//! sets when it opens the PHY. The word belongs to the radio's PMU view.
//! The bootstrap claims the view first, from [`PllPowerHandoff`]:
//! [`BootstrapPllPower`] reaches only the four PLL power fields, and each
//! change is one read-modify-write that keeps every other bit of the word.
//! Consuming the owner with [`BootstrapPllPower::release`] yields
//! [`PllPowerReleased`], which the radio root
//! ([`crate::RadioPartitions::from_handoff`]) takes by value, so the two
//! owners of the view never exist at once:
//!
//! ```compile_fail,E0382
//! use oer_esp32s31_pac::{
//!     RadioHandoff, RadioPartitions,
//!     pll_power::{BootstrapPllPower, PllPower, PllPowerHandoff},
//! };
//! fn overlap(radio: RadioHandoff, pll: PllPowerHandoff) {
//!     let mut bootstrap = BootstrapPllPower::from_handoff(pll);
//!     let partitions = RadioPartitions::from_handoff(radio, bootstrap.release());
//!     bootstrap.set_pll_power(PllPower::default()); // the owner was consumed
//! }
//! ```
//!
//! ```compile_fail,E0423
//! use oer_esp32s31_pac::pll_power::PllPowerReleased;
//! let released = PllPowerReleased(()); // only `release` makes one
//! ```
//!
//! SOURCE: S31_PMU_HEADERS (`pmu_reg.h`: `PMU_HP_ACTIVE_XPD_CPLL_I2C` bit 23,
//! `PMU_HP_ACTIVE_XPD_MPLL_I2C` bit 26, `PMU_HP_ACTIVE_XPD_CPLL` bit 27,
//! `PMU_HP_ACTIVE_XPD_MPLL` bit 30).

#![deny(unsafe_code)]

use oer_mmio::{Fixed, Placement};

use crate::generated::pmu_radio::{self, PmuRadio};

#[cfg(test)]
mod tests;

/// Proof that the bootstrap owns the PMU's radio view for its epoch.
#[must_use = "the bootstrap's PLL power owner needs the handoff"]
pub struct PllPowerHandoff(());

impl PllPowerHandoff {
    /// Assert the bootstrap's epoch of the PMU's radio view.
    ///
    /// # Safety
    /// Call at most once per boot. No code other than the
    /// [`BootstrapPllPower`] made from it, and after its release the radio
    /// root, writes the PMU's radio view.
    #[allow(
        unsafe_code,
        reason = "the bootstrap proves exclusive ownership of the PMU's radio view for its epoch"
    )]
    pub const unsafe fn assume_exclusive() -> Self {
        Self(())
    }
}

/// The CPLL and MPLL power fields, as the word holds them.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PllPower {
    /// The CPLL is powered.
    pub cpll: bool,
    /// The CPLL's analog-I2C interface is powered.
    pub cpll_i2c: bool,
    /// The MPLL is powered.
    pub mpll: bool,
    /// The MPLL's analog-I2C interface is powered.
    pub mpll_i2c: bool,
}

/// Proof that the bootstrap's owner of the PMU's radio view was consumed;
/// the radio root takes it by value.
#[must_use = "the radio root needs the released PMU view"]
pub struct PllPowerReleased(());

/// The bootstrap's owner of the PLL power fields.
#[must_use = "the radio root needs the owner's release"]
pub struct BootstrapPllPower<P: Placement = Fixed<{ pmu_radio::BASE }>>(PmuRadio<P>);

impl BootstrapPllPower {
    /// Claim the PMU's radio view for the bootstrap's epoch.
    #[allow(
        unsafe_code,
        reason = "the consumed handoff proves unique register ownership for the epoch"
    )]
    pub fn from_handoff(_handoff: PllPowerHandoff) -> Self {
        #[allow(
            clippy::disallowed_methods,
            reason = "the bootstrap claims the PMU's radio view once, from its handoff"
        )]
        // SAFETY: `PllPowerHandoff`'s constructor guarantees that this owner
        // is the only one of the PMU's radio view, and it is consumed here,
        // so the view is claimed once. The radio root claims the view again
        // only from the `PllPowerReleased` that consuming this owner yields,
        // so the two owners never exist at once. The generated
        // `const` assertion beside the typed blocks checks that no other
        // typed block lists its words; esp-hal's writes of this word are
        // reviewed in `registers/shared-words.toml`.
        let claim = unsafe { oer_mmio::assume_owned::<pmu_radio::Claim>() };
        Self(PmuRadio::new(claim))
    }
}

impl<P: Placement> BootstrapPllPower<P> {
    /// End the bootstrap's epoch of the PMU's radio view: the owner is
    /// consumed, and the proof hands the view to the radio root.
    pub fn release(self) -> PllPowerReleased {
        PllPowerReleased(())
    }

    /// The PLL power fields read now.
    pub fn pll_power(&self) -> PllPower {
        let word = self.0.read_hp_active_hp_ck_power();
        PllPower {
            cpll: word.hp_active_xpd_cpll(),
            cpll_i2c: word.hp_active_xpd_cpll_i2c(),
            mpll: word.hp_active_xpd_mpll(),
            mpll_i2c: word.hp_active_xpd_mpll_i2c(),
        }
    }

    /// Write the four PLL power fields in one read-modify-write; every other
    /// bit of the word keeps the value read.
    pub fn set_pll_power(&mut self, power: PllPower) {
        self.0.modify_hp_active_hp_ck_power(|w| {
            w.set_hp_active_xpd_cpll(power.cpll)
                .set_hp_active_xpd_cpll_i2c(power.cpll_i2c)
                .set_hp_active_xpd_mpll(power.mpll)
                .set_hp_active_xpd_mpll_i2c(power.mpll_i2c);
        });
    }
}

#[cfg(test)]
impl<'a> BootstrapPllPower<oer_mmio::Host<'a>> {
    /// The owner over host memory, for tests.
    fn host(words: &'a mut [u32]) -> Self {
        Self(PmuRadio::host(words).expect("memory holds the PMU's radio view"))
    }
}
