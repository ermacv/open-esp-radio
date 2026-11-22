//! The radio's power, clock and reset words as the hardware has them, for a
//! diagnostic record: the words the bring-up writes or relies on, each read
//! through its owner's typed accessor and kept whole. A snapshot writes
//! nothing and opens no clock, power or bus to read: a modem block's word is
//! read only behind its open register bus, which the snapshot reads first.

use core::marker::PhantomData;

use oer_chips_esp32s31_pac::hp_sys_clkrst::modem_ctrl0::RegModemClkEn;

use super::registers::{RadioPhyRegisters, RadioPlacement};

/// The power and clock words of the radio's PMU and HP_SYS_CLKRST parts,
/// which answer whatever the modem's state, and the modem blocks' words
/// when their register bus is open.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RadioPowerWords {
    pub pmu_hp_active_icg_modem: u32,
    pub pmu_hp_active_hp_ck_power: u32,
    pub pmu_rf_pwc: u32,
    pub pmu_ana_peri_pwr_ctrl: u32,
    /// The modem register bus's clock gate.
    pub hp_sys_clkrst_modem_ctrl0: u32,
    /// The modem's APB clock, source clocks and reset.
    pub hp_sys_clkrst_modem_conf: u32,
    /// Absent while the modem register bus is closed: not read.
    pub modem: Option<ModemWords>,
}

/// The MODEM_SYSCON and MODEM_LPCON words the radio's parts hold.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ModemWords {
    pub syscon_clk_conf: u32,
    pub syscon_clk_conf1: u32,
    pub syscon_clk_conf_power_st: u32,
    pub syscon_modem_rst_conf: u32,
    pub syscon_wifi_bb_cfg: u32,
    pub lpcon_lp_timer_conf: u32,
    pub lpcon_coex_lp_clk_conf: u32,
    pub lpcon_wifi_lp_clk_conf: u32,
    pub lpcon_modem_32k_clk_conf: u32,
    pub lpcon_clk_conf_power_st: u32,
    pub lpcon_tick_conf: u32,
}

/// The modem register bus open as the radio's gate words read it: its
/// clock on, the modem's APB clock on and its reset released. It borrows
/// the radio's registers, so no gate changes while it is held; another
/// owner of a modem block's word reads that word with it.
///
/// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, ESP_IDF_4D59230D_S31_MODEM_PLL_GATE.
pub struct ModemBusOpen<'a>(PhantomData<&'a ()>);

impl<P: RadioPlacement> RadioPhyRegisters<P> {
    /// The radio's power words, and the proof of an open modem register bus
    /// when its gate words read open. Writes nothing.
    pub fn observe_power(&self) -> (RadioPowerWords, Option<ModemBusOpen<'_>>) {
        let parts = &self.peripherals;
        let ctrl0 = parts.hp_sys_clkrst_radio.read_modem_ctrl0();
        let conf = parts.hp_sys_clkrst_radio.read_modem_conf();
        let open = ctrl0.reg_modem_clk_en() == RegModemClkEn::Enabled
            && conf.modem_apb_clk_en()
            && !conf.modem_rst_en();
        let modem = open.then(|| ModemWords {
            syscon_clk_conf: parts.modem_syscon_clock_reset.read_clk_conf().record_word(),
            syscon_clk_conf1: parts
                .modem_syscon_clock_reset
                .read_clk_conf1()
                .record_word(),
            syscon_clk_conf_power_st: parts
                .modem_syscon_clock_reset
                .read_clk_conf_power_st()
                .record_word(),
            syscon_modem_rst_conf: parts
                .modem_syscon_clock_reset
                .read_modem_rst_conf()
                .record_word(),
            syscon_wifi_bb_cfg: parts.modem_syscon_radio.read_wifi_bb_cfg().record_word(),
            lpcon_lp_timer_conf: parts
                .bluetooth_lp_timer_clock
                .read_lp_timer_conf()
                .record_word(),
            lpcon_coex_lp_clk_conf: parts.coex_lp_clock.read_coex_lp_clk_conf().record_word(),
            lpcon_wifi_lp_clk_conf: parts
                .modem_lpcon_power
                .read_wifi_lp_clk_conf()
                .record_word(),
            lpcon_modem_32k_clk_conf: parts
                .modem_lpcon_power
                .read_modem_32k_clk_conf()
                .record_word(),
            lpcon_clk_conf_power_st: parts
                .modem_lpcon_power
                .read_clk_conf_power_st()
                .record_word(),
            lpcon_tick_conf: parts.modem_lpcon_phy_tick.read_tick_conf().record_word(),
        });
        let words = RadioPowerWords {
            pmu_hp_active_icg_modem: parts.pmu_radio.read_hp_active_icg_modem().record_word(),
            pmu_hp_active_hp_ck_power: parts.pmu_radio.read_hp_active_hp_ck_power().record_word(),
            pmu_rf_pwc: parts.pmu_radio.read_rf_pwc().record_word(),
            pmu_ana_peri_pwr_ctrl: parts.pmu_radio.read_ana_peri_pwr_ctrl().record_word(),
            hp_sys_clkrst_modem_ctrl0: ctrl0.record_word(),
            hp_sys_clkrst_modem_conf: conf.record_word(),
            modem,
        };
        (words, open.then_some(ModemBusOpen(PhantomData)))
    }
}

#[cfg(test)]
mod tests;
