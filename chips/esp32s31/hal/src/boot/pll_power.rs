//! Bootstrap PLL power operations. The PAC retains acquisition and the affine release proof.
//!
//! SOURCE: S31_PMU_HEADERS (`HP_ACTIVE_HP_CK_POWER` CPLL/MPLL power and analog-I2C power).
//!
//! ```compile_fail,E0382
//! use oer_esp32s31_hal::boot::pll_power::BootstrapPllPower;
//! fn after_release(mut pll: BootstrapPllPower) {
//!     let released = pll.release();
//!     let second = pll.release();
//! }
//! ```
use oer_esp32s31_pac::{pll_power as parts, pmu_radio};
use oer_espressif_boot::{Access, clock::ActivePllPower};
use oer_mmio::{Fixed, Placement};

/// The CPLL and MPLL power fields, as the word holds them.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct PllPower {
    /// The CPLL is powered.
    pub cpll: bool,
    /// The CPLL's analog-I2C interface is powered.
    pub cpll_i2c: bool,
    /// The MPLL is powered.
    pub mpll: bool,
    /// The MPLL's analog-I2C interface is powered.
    pub mpll_i2c: bool,
}

/// Owns the bootstrap's acquired PLL power parts until their affine release.
#[must_use = "the radio root needs the owner's release"]
pub struct BootstrapPllPower<P: Placement = Fixed<{ pmu_radio::BASE }>>(
    parts::BootstrapPllPower<P>,
);

impl<P: Placement> BootstrapPllPower<P> {
    /// Consume the already acquired bootstrap parts.
    pub const fn new(parts: parts::BootstrapPllPower<P>) -> Self {
        Self(parts)
    }

    /// The PLL power fields read now.
    fn pll_power(&self) -> PllPower {
        let word = self.0.0.read_hp_active_hp_ck_power();
        PllPower {
            cpll: word.hp_active_xpd_cpll(),
            cpll_i2c: word.hp_active_xpd_cpll_i2c(),
            mpll: word.hp_active_xpd_mpll(),
            mpll_i2c: word.hp_active_xpd_mpll_i2c(),
        }
    }

    /// Write the four PLL power fields in one read-modify-write; every other
    /// bit of the word keeps the value read.
    fn set_pll_power(&mut self, power: PllPower) {
        self.0.0.modify_hp_active_hp_ck_power(|w| {
            w.set_hp_active_xpd_cpll(power.cpll)
                .set_hp_active_xpd_cpll_i2c(power.cpll_i2c)
                .set_hp_active_xpd_mpll(power.mpll)
                .set_hp_active_xpd_mpll_i2c(power.mpll_i2c);
        });
    }
}

impl BootstrapPllPower {
    /// Consume the bootstrap owner before the radio root can claim the PMU view.
    pub fn release(self) -> parts::PllPowerReleased {
        self.0.release()
    }
}

impl<P: Placement> ActivePllPower for BootstrapPllPower<P> {
    fn keep_cpu_pll(&mut self, _: Access<'_>) {
        self.set_pll_power(PllPower {
            cpll: true,
            cpll_i2c: true,
            ..self.pll_power()
        });
    }
    fn keep_memory_pll(&mut self, _: Access<'_>) {
        self.set_pll_power(PllPower {
            mpll: true,
            mpll_i2c: true,
            ..self.pll_power()
        });
    }
}

#[cfg(test)]
impl<'a> BootstrapPllPower<oer_mmio::Host<'a>> {
    pub(crate) fn host(words: &'a mut [u32]) -> Self {
        Self(parts::BootstrapPllPower(
            pmu_radio::PmuRadio::host(words).expect("memory holds the PMU's radio view"),
        ))
    }
}
#[cfg(test)]
mod tests;
