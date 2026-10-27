//! The PLLs' power in the PMU's active state (`HP_ACTIVE_HP_CK_POWER`), a
//! word of the radio PMU part: the bootstrap's root lends the part to this
//! owner for its PLL epoch; in stage two the radio domain owns the part.
//!
//! SOURCE: ESP-IDF 4d80c1b2 `components/soc/esp32s31/register/soc/pmu_reg.h`
//! (`HP_ACTIVE_HP_CK_POWER`: CPLL and MPLL power and their analog-I2C power).

use oer_esp32s31_pac::{instances, pmu::PmuRadio};
use oer_espressif_boot::{Access, clock::ActivePllPower};
use oer_mmio::{Fixed, Placement};

#[cfg(test)]
mod tests;

/// The CPLL and MPLL power fields, as the word holds them.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct PllPower {
    /// The CPLL is powered.
    cpll: bool,
    /// The CPLL's analog-I2C interface is powered.
    cpll_i2c: bool,
    /// The MPLL is powered.
    mpll: bool,
    /// The MPLL's analog-I2C interface is powered.
    mpll_i2c: bool,
}

/// The bootstrap's owner of the PLLs' active-state power.
#[must_use = "dropping a register owner permanently loses its register authority"]
pub struct ActivePower<P: Placement = Fixed<{ instances::pmu::BASE }>>(PmuRadio<P>);

impl ActivePower {
    /// Own the radio PMU part for the bootstrap's PLL epoch.
    pub(crate) fn new(part: PmuRadio) -> Self {
        Self(part)
    }
}

impl<P: Placement> ActivePower<P> {
    /// The PLL power fields read now.
    fn pll_power(&self) -> PllPower {
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
    fn set_pll_power(&mut self, power: PllPower) {
        self.0.modify_hp_active_hp_ck_power(|w| {
            w.set_hp_active_xpd_cpll(power.cpll)
                .set_hp_active_xpd_cpll_i2c(power.cpll_i2c)
                .set_hp_active_xpd_mpll(power.mpll)
                .set_hp_active_xpd_mpll_i2c(power.mpll_i2c);
        });
    }
}

impl<P: Placement> ActivePllPower for ActivePower<P> {
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

#[cfg(any(test, feature = "test-support"))]
impl<'a> ActivePower<oer_mmio::Host<'a>> {
    /// The owner over host memory from the PMU's base.
    pub fn host(words: &'a mut [u32]) -> Result<Self, oer_mmio::HostTooSmall> {
        PmuRadio::host(words).map(Self)
    }
}
