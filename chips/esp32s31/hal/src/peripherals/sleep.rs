//! The HP system's light-sleep request: the PMU's sleep state, its waits,
//! the request and outcome, and the LP main timer that wakes it.
//!
//! [`SleepRegisters`] implements the family sleep port
//! (`oer_chips_family_power::sleep::Registers`), one register operation per
//! method. The state it writes powers no domain down and keeps the PLLs, the
//! main crystal and the fast RC oscillator on, with the regulators at their
//! active output: only the CPU stalls, and the clock gates follow the
//! request.
//!
//! SOURCE: ESP-IDF 4d80c1b2 `components/esp_hw_support/port/esp32s31/pmu_sleep.c:203-353`
//! (`pmu_sleep_config_default`, `pmu_sleep_init`: the light-sleep power,
//! digital, analog and wait parameters), `.../private_include/pmu_param.h:22-56,258-275,361-534`
//! (the light-sleep defaults and machine constants), `pmu_param.c:87-173`
//! (the HP_SLEEP power and clock defaults) and `pmu_sleep.c:354-421`
//! (`pmu_sleep_start`: wake and reject enables, acknowledgements, request
//! and outcome).

use oer_chips_esp32s31_pac::{
    ownership::SleepPeripherals,
    pmu::{
        PmuSleepControl, PmuSleepState, hp_sleep_icg_modem::HpSleepDigIcgModemCode,
        hp_sleep_sysclk::HpSleepDigSysClkSel, slp_wakeup_cntl3::SleepPrtSel,
    },
    rtc_timer::RtcTimerWake,
};
use oer_chips_family_power::{
    Access,
    sleep::{RawOutcome, Registers, RtcTicks, SleepClocks},
};
use oer_portable_mmio::{Bits, Fixed, Placement};

#[cfg(test)]
mod tests;

/// Where the PMU lies.
pub type PmuPlacement = Fixed<{ oer_chips_esp32s31_pac::instances::pmu::BASE }>;
/// Where the LP main timer lies.
pub type RtcTimerPlacement = Fixed<{ oer_chips_esp32s31_pac::instances::rtc_timer::BASE }>;

/// The fast RC oscillator, the PMU's work clock, and the slow RC
/// oscillator, the LP timer's clock.
///
/// SOURCE: ESP-IDF 4d80c1b2 `components/soc/esp32s31/include/soc/clk_tree_defs.h:51-52`
/// (`SOC_CLK_RC_FAST_FREQ_APPROX` 17.5 MHz, `SOC_CLK_RC_SLOW_FREQ_APPROX`
/// 136 kHz). The vendor converts with calibrated periods; these nominal
/// rates round every wait up.
const RC_FAST_HZ: u64 = 17_500_000;
const RC_SLOW_HZ: u64 = 136_000;

/// Cycles of a `hz` clock covering `micros`, rounded up.
const fn cycles(micros: u64, hz: u64) -> u64 {
    (micros * hz).div_ceil(1_000_000)
}

/// A cycle count checked, when the crate compiles, to fit a field of
/// `WIDTH` bits.
const fn field<const WIDTH: u32>(cycles: u64) -> Bits<WIDTH> {
    if cycles > u32::MAX as u64 {
        panic!("a wait wider than a register word");
    }
    match Bits::new(cycles as u32) {
        Ok(bits) => bits,
        Err(_) => panic!("a wait wider than its field"),
    }
}

/// The PMU's minimum sleep, HP and LP: 450 µs of the slow clock.
///
/// SOURCE: `pmu_param.h:499-503,512` (`min_slp_time_us` 450, LP and HP).
const MIN_SLEEP_CYCLES: Bits<8> = const { field(cycles(450, RC_SLOW_HZ)) };
/// The HP regulator's power-up wait: 154 µs of the fast clock.
///
/// SOURCE: `pmu_param.h:515` (`hp.analog_wait_time_us` 154).
const HP_ANALOG_WAIT_CYCLES: Bits<16> = const { field(cycles(154, RC_FAST_HZ)) };
/// The LP regulator's power-up wait: 154 µs of the slow clock.
///
/// SOURCE: `pmu_param.h:502` (`lp.analog_wait_time_us` 154).
const LP_ANALOG_WAIT_CYCLES: Bits<8> = const { field(cycles(154, RC_SLOW_HZ)) };
/// The main crystal's stabilization while it stays on: 250 µs of the slow
/// clock.
///
/// SOURCE: `pmu_param.h:503` (`lp.xtal_wait_stable_time_us` 250) and
/// `pmu_sleep.c:191-195` (slow-clock cycles while the crystal is on).
const XTAL_STABLE_CYCLES: Bits<16> = const { field(cycles(250, RC_SLOW_HZ)) };
/// The PLLs' stabilization: 50 µs of the fast clock.
///
/// SOURCE: `pmu_param.h:528` (`hp.pll_wait_stable_time_us` 50).
const PLL_STABLE_CYCLES: Bits<16> = const { field(cycles(50, RC_FAST_HZ)) };
/// The digital power supply and power-up waits, HP and LP: 2 µs each of
/// the fast clock.
///
/// SOURCE: `pmu_param.h:508-509,520-521` (`power_supply_wait_time_us`,
/// `power_up_wait_time_us` 2).
const POWER_WAIT_CYCLES: u64 = cycles(2, RC_FAST_HZ);
/// The isolation and reset waits, HP and LP: 1 µs each of the fast clock.
///
/// SOURCE: `pmu_param.h:506-507,518-519` (`isolate_wait_time_us`,
/// `reset_wait_time_us` 1).
const CONTROL_WAIT_CYCLES: Bits<8> = const { field(cycles(1, RC_FAST_HZ)) };

/// The regulators' active output, kept while the crystal stays on.
///
/// SOURCE: `pmu_param.h:22-23` (`HP_CALI_DBIAS_DEFAULT`,
/// `LP_CALI_DBIAS_DEFAULT` 26) and `pmu_sleep.c:229-234` (kept unless the
/// crystal and the fast RC oscillator both power down).
const ACTIVE_DBIAS: Bits<5> = Bits::constant::<26>();

/// The PMU's wake-enable bit of the LP main timer.
///
/// SOURCE: ESP-IDF 4d80c1b2 `components/esp_hw_support/port/esp32s31/private_include/pmu_bit_defs.h:27`
/// (`PMU_RTC_TIMER_WAKEUP_EN` bit 13).
const RTC_TIMER_WAKE: u32 = 1 << 13;

/// The PMU's sleep words and the LP main timer's wake words, split from the
/// root's sleep partition.
#[must_use = "dropping a register owner permanently loses its register authority"]
pub struct SleepRegisters<
    P: Placement = PmuPlacement,
    Q: Placement = PmuPlacement,
    T: Placement = RtcTimerPlacement,
> {
    state: PmuSleepState<P>,
    control: PmuSleepControl<Q>,
    timer: RtcTimerWake<T>,
}

impl SleepRegisters {
    /// Own the root's sleep partition.
    pub fn new(peripherals: SleepPeripherals) -> Self {
        let SleepPeripherals {
            pmu_sleep_state,
            pmu_sleep_control,
            rtc_timer_wake,
        } = peripherals;
        Self {
            state: pmu_sleep_state,
            control: pmu_sleep_control,
            timer: rtc_timer_wake,
        }
    }
}

impl<P: Placement, Q: Placement, T: Placement> Registers for SleepRegisters<P, Q, T> {
    /// Twice the PMU's minimum sleep: the minimum and the entry.
    const MIN_SLEEP: u64 = 2 * MIN_SLEEP_CYCLES.get() as u64;

    fn configure_state(&mut self, _: Access<'_>, clocks: SleepClocks) {
        let gates = match clocks {
            SleepClocks::Running => u32::MAX,
            SleepClocks::Gated => 0,
        };
        let state = &mut self.state;
        state.modify_hp_sleep_dig_power(|w| {
            w.set_hp_sleep_vdd_spi_pd_en(false)
                .set_hp_sleep_pd_hp_alive_pd_en(false)
                .set_hp_sleep_hp_mem_dslp(false)
                .set_hp_sleep_pd_hp_mem_pd_en(Bits::constant::<0>())
                .set_hp_sleep_pd_modem_top_pd_en(false)
                .set_hp_sleep_pd_hp_cnnt_pd_en(false)
                .set_hp_sleep_pd_hp_cpu_pd_en(false)
                .set_hp_sleep_pd_modem_pwr_pd_en(false)
                .set_hp_sleep_pd_top_pd_en(false);
        });
        state.modify_hp_sleep_icg_hp_func(|w| {
            w.set_hp_sleep_dig_icg_func_en(gates);
        });
        state.modify_hp_sleep_icg_hp_apb(|w| {
            w.set_hp_sleep_dig_icg_apb_en(gates);
        });
        state.modify_hp_sleep_icg_hp_func1(|w| {
            w.set_hp_sleep_dig_icg_func1_en(gates);
        });
        state.modify_hp_sleep_icg_hp_apb1(|w| {
            w.set_hp_sleep_dig_icg_apb1_en(gates);
        });
        state.modify_hp_sleep_icg_modem(|w| {
            w.set_hp_sleep_dig_icg_modem_code(HpSleepDigIcgModemCode::Sleep);
        });
        // The PLLs stay on, so the system clock keeps its source through
        // the sleep: the PMU does not switch it to the crystal, and nothing
        // restores the CPU clock after the wake. The vendor switches it
        // (`sysclk_slp_sel` 1, `pmu_param.c:160-173`) because its sleep
        // powers the PLLs down.
        state.modify_hp_sleep_sysclk(|w| {
            w.set_hp_sleep_dig_sys_clk_no_div(false)
                .set_hp_sleep_icg_sys_clock_en(matches!(clocks, SleepClocks::Running))
                .set_hp_sleep_sys_clk_slp_sel(false)
                .set_hp_sleep_icg_slp_sel(true)
                .set_hp_sleep_dig_sys_clk_sel(HpSleepDigSysClkSel::Xtal);
        });
        // No domain powers down: the pads keep their active configuration
        // (the vendor selects their sleep configuration only with the
        // PSRAM pins held, `pmu_sleep.c:354-366`) and the C channel stays
        // open; the watchdogs pause and the CPU stalls.
        state.modify_hp_sleep_hp_sys_cntl(|w| {
            w.set_hp_sleep_c_channel(true)
                .set_hp_sleep_uart_wakeup_en(true)
                .set_hp_sleep_lp_pad_hold_all(false)
                .set_hp_sleep_hp_pad_hold_all(false)
                .set_hp_sleep_dig_pad_slp_sel(false)
                .set_hp_sleep_dig_pause_wdt(true)
                .set_hp_sleep_dig_cpu_stall(true);
        });
        // The active state the PMU returns to on the wake, as the vendor's
        // `pmu_init` leaves it before any sleep; the reset leaves the
        // system clock's gate closed there.
        //
        // SOURCE: `pmu_param.c:128-142,186-197,233-238`
        // (`PMU_HP_ACTIVE_CLOCK_CONFIG_DEFAULT`, `..._DIGITAL_...`,
        // `..._ANALOG_...` bias) and `pmu_init.c:72-142`.
        state.modify_hp_active_sysclk(|w| {
            w.set_hp_active_dig_sys_clk_no_div(false)
                .set_hp_active_icg_sys_clock_en(true)
                .set_hp_active_sys_clk_slp_sel(false)
                .set_hp_active_icg_slp_sel(false);
        });
        state.modify_hp_active_hp_sys_cntl(|w| {
            w.set_hp_active_c_channel(true)
                .set_hp_active_uart_wakeup_en(false)
                .set_hp_active_lp_pad_hold_all(false)
                .set_hp_active_hp_pad_hold_all(false)
                .set_hp_active_dig_pad_slp_sel(false)
                .set_hp_active_dig_pause_wdt(false)
                .set_hp_active_dig_cpu_stall(false);
        });
        state.modify_hp_active_bias(|w| {
            w.set_hp_active_xpd_bias(true)
                .set_hp_active_dbg_atten(Bits::constant::<0>())
                .set_hp_active_pd_cur(false)
                .set_sleep(false);
        });
        // The CPU, baseband and MSPI PLLs and their analog I2C stay powered
        // as in the active state; the audio PLL stays off.
        state.modify_hp_sleep_hp_ck_power(|w| {
            w.set_hp_sleep_xpd_xtalx2(false)
                .set_hp_sleep_i2c_iso_en(false)
                .set_hp_sleep_i2c_retention(false)
                .set_hp_sleep_xpd_bb_i2c(true)
                .set_hp_sleep_xpd_cpll_i2c(true)
                .set_hp_sleep_xpd_bbpll_i2c(true)
                .set_hp_sleep_xpd_apll_i2c(false)
                .set_hp_sleep_xpd_mpll_i2c(true)
                .set_hp_sleep_xpd_cpll(true)
                .set_hp_sleep_xpd_bbpll(true)
                .set_hp_sleep_xpd_apll(false)
                .set_hp_sleep_xpd_mpll(true);
        });
        state.modify_hp_sleep_xtal(|w| {
            w.set_hp_sleep_xpd_xtal(true);
        });
        state.modify_hp_sleep_bias(|w| {
            w.set_hp_sleep_xpd_bias(false)
                .set_hp_sleep_dbg_atten(Bits::constant::<0>())
                .set_hp_sleep_pd_cur(false)
                .set_sleep(true);
        });
        state.modify_hp_sleep_hp_regulator0(|w| {
            w.set_hp_sleep_hp_regulator_slp_connect_en(false)
                .set_hp_sleep_hp_regulator_slp_mem_xpd(false)
                .set_hp_sleep_hp_regulator_slp_logic_xpd(false)
                .set_hp_sleep_hp_regulator_xpd(true)
                .set_hp_sleep_hp_regulator_slp_mem_dbias(Bits::constant::<0>())
                .set_hp_sleep_hp_regulator_slp_logic_dbias(Bits::constant::<0>())
                .set_hp_sleep_hp_regulator_dbias(ACTIVE_DBIAS);
        });
        state.modify_hp_sleep_hp_regulator1(|w| {
            w.set_hp_sleep_hp_regulator_drv_b(Bits::constant::<0>());
        });
        // The LP system's sleep: its peripherals and the fast RC oscillator
        // stay on, the 32 kHz oscillators stay off.
        state.modify_lp_sleep_lp_dig_power(|w| {
            w.set_lp_sleep_lp_mem_dslp(false)
                .set_lp_sleep_pd_lp_peri_pd_en(false);
        });
        state.modify_lp_sleep_lp_ck_power(|w| {
            w.set_lp_sleep_xpd_xtal32k(false)
                .set_lp_sleep_xpd_rc32k(false)
                .set_lp_sleep_xpd_fosc_clk(true)
                .set_lp_sleep_pd_osc_clk(false);
        });
        state.modify_lp_sleep_xtal(|w| {
            w.set_lp_sleep_xpd_xtal(true);
        });
        state.modify_lp_sleep_bias(|w| {
            w.set_lp_sleep_xpd_bias(false)
                .set_lp_sleep_dbg_atten(Bits::constant::<0>())
                .set_lp_sleep_pd_cur(false)
                .set_sleep(true);
        });
        state.modify_lp_sleep_lp_regulator0(|w| {
            w.set_lp_sleep_lp_regulator_slp_xpd(false)
                .set_lp_sleep_lp_regulator_xpd(true)
                .set_lp_sleep_lp_regulator_slp_dbias(Bits::constant::<0>())
                .set_lp_sleep_lp_regulator_dbias(ACTIVE_DBIAS);
        });
        state.modify_lp_sleep_lp_regulator1(|w| {
            w.set_lp_sleep_lp_regulator_drv_b(Bits::constant::<0>());
        });
    }

    fn configure_waits(&mut self, _: Access<'_>) {
        const HP_POWER: Bits<9> = const { field(POWER_WAIT_CYCLES) };
        const LP_POWER_UP: Bits<7> = const { field(POWER_WAIT_CYCLES) };
        const LP_SUPPLY: Bits<9> = const { field(POWER_WAIT_CYCLES) };
        let control = &mut self.control;
        control.modify_slp_wakeup_cntl3(|w| {
            w.set_lp_min_slp_val(MIN_SLEEP_CYCLES.get() as u8)
                .set_hp_min_slp_val(MIN_SLEEP_CYCLES.get() as u8)
                .set_sleep_prt_sel(SleepPrtSel::HpLpSleep);
        });
        control.modify_slp_wakeup_cntl7(|w| {
            w.set_ana_wait_target(HP_ANALOG_WAIT_CYCLES.get() as u16);
        });
        control.modify_slp_wakeup_cntl5(|w| {
            w.set_lp_ana_wait_target(LP_ANALOG_WAIT_CYCLES.get() as u8);
        });
        control.modify_power_wait_timer0(|w| {
            w.set_dg_hp_powerup_timer(HP_POWER)
                .set_dg_hp_pd_wait_timer(HP_POWER);
        });
        control.modify_power_wait_timer1(|w| {
            w.set_dg_lp_powerup_timer(LP_POWER_UP)
                .set_dg_lp_pd_wait_timer(LP_SUPPLY);
        });
        control.modify_power_wait_timer2(|w| {
            let wait = CONTROL_WAIT_CYCLES.get() as u8;
            w.set_dg_lp_iso_wait_timer(wait)
                .set_dg_lp_rst_wait_timer(wait)
                .set_dg_hp_iso_wait_timer(wait)
                .set_dg_hp_rst_wait_timer(wait);
        });
        control.modify_power_ck_wait_cntl(|w| {
            w.set_pmu_wait_xtl_stable(XTAL_STABLE_CYCLES.get() as u16)
                .set_pmu_wait_pll_stable(PLL_STABLE_CYCLES.get() as u16);
        });
    }

    fn capture_rtc(&mut self, _: Access<'_>) {
        self.timer.trigger_update_main_timer_update();
    }

    fn captured_rtc(&self, _: Access<'_>) -> u64 {
        let high = self.timer.read_main_buf0_high().main_timer_buf0_high();
        let low = self.timer.read_main_buf0_low().main_timer_buf0_low();
        (u64::from(high) << 32) | u64::from(low)
    }

    fn acknowledge_rtc_wake(&mut self, _: Access<'_>) {
        self.timer.trigger_int_clr_soc_wakeup_int_clr();
    }

    fn rtc_wake_pending(&mut self, _: Access<'_>) -> bool {
        self.timer.read_int_raw().soc_wakeup_int_raw()
    }

    fn arm_rtc_target(&mut self, _: Access<'_>, at: RtcTicks) {
        let ticks = at.get();
        let high = (ticks >> 32) as u16;
        let low = ticks as u32;
        self.timer.modify_tar0_high(|w| {
            w.set_main_timer_tar_high0(high);
        });
        self.timer.modify_tar0_low(|w| {
            w.set_main_timer_tar_low0(low);
        });
        self.timer.trigger_tar0_high_main_timer_tar_en0();
    }

    fn select_wake(&mut self, _: Access<'_>, rtc_timer: bool) {
        let sources = if rtc_timer { RTC_TIMER_WAKE } else { 0 };
        self.control.modify_slp_wakeup_cntl2(|w| {
            w.set_wakeup_ena(sources);
        });
    }

    fn disable_reject(&mut self, _: Access<'_>) {
        self.control.modify_slp_wakeup_cntl1(|w| {
            w.set_sleep_reject_ena(0);
        });
        self.control.modify_slp_wakeup_cntl0(|w| {
            w.set_slp_reject_en(false);
        });
    }

    fn acknowledge_outcome(&mut self, _: Access<'_>) {
        self.control.trigger_hp_int_clr_soc_wakeup_int_clr();
        self.control.trigger_hp_int_clr_soc_sleep_reject_int_clr();
        self.control.trigger_slp_wakeup_cntl4_slp_reject_cause_clr();
    }

    fn request_sleep(&mut self, _: Access<'_>) {
        self.control.trigger_slp_wakeup_cntl0_sleep_req();
    }

    fn outcome(&mut self, _: Access<'_>) -> RawOutcome {
        let raw = self.control.read_int_raw();
        RawOutcome {
            woke: raw.soc_wakeup_int_raw(),
            rejected: raw.soc_sleep_reject_int_raw(),
        }
    }

    fn wake_causes(&self, _: Access<'_>) -> u32 {
        self.control.read_slp_wakeup_status0().wakeup_cause()
    }

    fn reject_causes(&self, _: Access<'_>) -> u32 {
        self.control.read_slp_wakeup_status1().reject_cause()
    }
}

#[cfg(any(test, feature = "test-support"))]
impl<'a, 'b, 'c>
    SleepRegisters<
        oer_portable_mmio::Host<'a>,
        oer_portable_mmio::Host<'b>,
        oer_portable_mmio::Host<'c>,
    >
{
    /// The sleep words over host memory, each part from its instance's
    /// base.
    pub fn host(
        state: &'a mut [u32],
        control: &'b mut [u32],
        timer: &'c mut [u32],
    ) -> Result<Self, oer_portable_mmio::HostTooSmall> {
        Ok(Self {
            state: PmuSleepState::host(state)?,
            control: PmuSleepControl::host(control)?,
            timer: RtcTimerWake::host(timer)?,
        })
    }
}
