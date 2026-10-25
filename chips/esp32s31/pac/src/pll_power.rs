//! The bootstrap's epoch of the PLL power fields.
//!
//! The PMU's active-state clock-power word (`HP_ACTIVE_HP_CK_POWER`) holds
//! the CPLL and MPLL power and their analog-I2C power, which the bootstrap
//! sets before it programs the CPU clock and the PSRAM clock, and the
//! front-end clock image and baseband analog-I2C power, which the radio
//! sets when it opens the PHY. The word belongs to the radio's PMU view.
//! The HAL owns the PLL operations through the generated power-word accessors.
//! Consuming [`BootstrapPllPower::release`] yields [`PllPowerReleased`], which
//! [`crate::RadioPartitions::from_handoff`] consumes before claiming the radio PMU.
//!
//! ```compile_fail,E0423
//! use oer_esp32s31_pac::pll_power::PllPowerReleased;
//! let released = PllPowerReleased(());
//! ```
//!
//! A PMU part alone cannot create a bootstrap owner and release proof:
//!
//! ```compile_fail,E0423
//! use oer_esp32s31_pac::{
//!     pll_power::{BootstrapPllPower, PllPowerReleased}, pmu_radio::PmuRadio,
//! };
//! fn bypass_handoff(pmu: PmuRadio) -> PllPowerReleased {
//!     BootstrapPllPower(pmu).release()
//! }
//! ```
//!
//! Host-backed parts cannot manufacture the chip's release proof:
//!
//! ```compile_fail,E0599
//! use oer_esp32s31_pac::pll_power::BootstrapPllPower;
//! fn host_release(host: BootstrapPllPower<oer_mmio::Host<'_>>) {
//!     let released = host.release();
//! }
//! ```
//!
//! SOURCE: S31_PMU_HEADERS (`pmu_reg.h`: `PMU_HP_ACTIVE_XPD_CPLL_I2C` bit 23,
//! `PMU_HP_ACTIVE_XPD_MPLL_I2C` bit 26, `PMU_HP_ACTIVE_XPD_CPLL` bit 27,
//! `PMU_HP_ACTIVE_XPD_MPLL` bit 30).

#![deny(unsafe_code)]

use oer_mmio::{Fixed, Placement};

use crate::generated::pmu_radio::{self, PmuRadio};

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

impl BootstrapPllPower {
    /// End the bootstrap's epoch of the PMU's radio view: the owner is
    /// consumed, and the proof hands the view to the radio root.
    pub fn release(self) -> PllPowerReleased {
        PllPowerReleased(())
    }
}

impl<P: Placement> BootstrapPllPower<P> {
    /// Read the active-state clock-power word through its generated fields.
    pub fn read_hp_active_hp_ck_power(&self) -> pmu_radio::hp_active_hp_ck_power::R {
        self.0.read_hp_active_hp_ck_power()
    }

    /// Change active-state clock-power fields, preserving every other bit.
    pub fn modify_hp_active_hp_ck_power(
        &mut self,
        change: impl FnOnce(&mut pmu_radio::hp_active_hp_ck_power::W),
    ) {
        self.0.modify_hp_active_hp_ck_power(change);
    }
}

#[cfg(any(test, feature = "test-support"))]
impl<'a> BootstrapPllPower<oer_mmio::Host<'a>> {
    /// The PLL power fields over host memory; this owner cannot release a chip proof.
    pub fn host(words: &'a mut [u32]) -> Result<Self, oer_mmio::HostTooSmall> {
        PmuRadio::host(words).map(Self)
    }
}
