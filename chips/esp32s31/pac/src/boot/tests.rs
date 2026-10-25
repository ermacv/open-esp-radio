//! The boot owners over host memory: each test fills the words with a
//! pattern, runs one sequence of `oer-espressif-boot` on an owner and reads
//! the fields back through a typed view of the same words, never through an
//! address or a mask.
//!
//! The clock update and the PLL's power are not written by this PAC yet
//! (the render has no command kind for `SOC_CLK_UPDATE` and no writer of
//! `IMM_HP_CK_POWER_1`); the test implements them over nothing, so the
//! clock sequence runs on the words this PAC does write.

use std::{vec, vec::Vec};

use oer_espressif_boot::{
    Access, BootFailure, WaitFailure,
    cache::{SuspendWaits, with_fetch_suspended},
    clock::{ClockRates, ClockUpdate, ClockWaits, CpuClockProfile, PllPower, bring_up},
    wait::{BootBudget, BootDeadline, BootWait},
};
use oer_mmio::{Bits, Host};
use oer_time::{Clock, Duration, Instant};
use oer_time_virtual::VirtualClock;

use super::{BusDividers, CacheRegisters, ClockRegisters, CpllDividers, CpuRoot, MemDivider};
use crate::generated::soc::{
    cache::{self, Cache},
    hp_sys_clkrst::{self, HpSysClkrst},
    lp_aon_clkrst::{self, LpAonClkrst},
};

fn memory(size: usize, fill: u32) -> Vec<u32> {
    vec![fill; size / 4]
}

/// Virtual time that moves a microsecond at every read.
struct Ticking<'a>(&'a VirtualClock);

impl Clock for Ticking<'_> {
    fn now(&self) -> Instant {
        self.0
            .advance(Duration::from_micros(1))
            .expect("the test's virtual time does not overflow")
    }
}

fn budget() -> BootBudget {
    BootBudget::below(Duration::from_millis(10), Duration::from_secs(9)).expect("below")
}

impl ClockUpdate for ClockRegisters<Host<'_>, Host<'_>> {
    fn start_update(&mut self, _: Access<'_>) {}
    fn update_pending(&self, _: Access<'_>) -> bool {
        false
    }
}

impl PllPower for ClockRegisters<Host<'_>, Host<'_>> {
    fn power_up(&mut self, _: Access<'_>) {}
}

const PROFILE: CpuClockProfile<CpuRoot, BusDividers, CpllDividers> = CpuClockProfile {
    root: CpuRoot::Cpll,
    pll: Some(CpllDividers {
        feedback: 8,
        reference: Bits::constant::<1>(),
    }),
    dividers: BusDividers {
        cpu: 1,
        mem: MemDivider::Same,
        sys: 1,
        apb: 1,
    },
    rates: ClockRates {
        cpu_hz: 160_000_000,
        mem_hz: 160_000_000,
        sys_hz: 80_000_000,
        apb_hz: 40_000_000,
    },
};

const WAITS: ClockWaits = ClockWaits {
    calibration: BootWait::new("cpll-calibration", Duration::from_micros(100)),
    calibration_settle: BootWait::new("cpll-settle", Duration::from_micros(10)),
    update: BootWait::new("clock-update", Duration::from_micros(100)),
};

#[test]
fn a_cold_cpu_clock_is_staged_with_its_pll_dividers_and_calibration_stopped() {
    // All ones: the root selector reads PLL_F240M, not the profile's CPLL,
    // and the CPLL calibration's end reads set.
    let mut clocks = memory(hp_sys_clkrst::SIZE, u32::MAX);
    let mut lp = memory(lp_aon_clkrst::SIZE, u32::MAX);
    let virtual_clock = VirtualClock::new();
    let ticking = Ticking(&virtual_clock);
    let deadline = BootDeadline::start(&ticking, budget());
    let mut owner = ClockRegisters::host(&mut clocks, &mut lp).expect("host memory holds them");
    assert_eq!(
        bring_up(&mut owner, PROFILE, WAITS, &deadline),
        Ok(PROFILE.rates)
    );
    drop(owner);

    let view = HpSysClkrst::host(&mut clocks).expect("host memory holds it");
    assert_eq!(view.read_soc_clk_sel().reg_soc_clk_sel().get(), 1, "CPLL");
    let cpu = view.read_cpu_freq_ctrl0();
    assert_eq!(cpu.reg_cpu_clk_div_num(), 1);
    assert_eq!(cpu.reg_cpu_clk_div_numerator().get(), 0);
    assert_eq!(cpu.reg_cpu_clk_div_denominator().get(), 0);
    assert!(!view.read_mem_freq_ctrl0().reg_mem_clk_div_num());
    assert_eq!(view.read_sys_freq_ctrl0().reg_sys_clk_div_num(), 1);
    assert_eq!(view.read_apb_freq_ctrl0().reg_apb_clk_div_num(), 1);
    let pll = view.read_ana_pll_ctrl0();
    assert!(pll.reg_cpu_pll_cal_stop(), "the calibration is stopped");
    assert!(pll.reg_bbpll_cal_stop(), "the other PLLs' fields are kept");
    let lp = LpAonClkrst::host(&mut lp).expect("host memory holds it");
    let divider = lp.read_lp_aonclkrst_cpll_div();
    assert_eq!(divider.lp_aonclkrst_cpll_fb_div(), 8);
    assert_eq!(divider.lp_aonclkrst_cpll_ref_div().get(), 1);
}

#[test]
fn a_cpll_that_never_calibrates_stops_the_boot_before_the_root_moves() {
    // All zeros: the root reads XTAL and the calibration never ends.
    let mut clocks = memory(hp_sys_clkrst::SIZE, 0);
    let mut lp = memory(lp_aon_clkrst::SIZE, 0);
    let virtual_clock = VirtualClock::new();
    let ticking = Ticking(&virtual_clock);
    let deadline = BootDeadline::start(&ticking, budget());
    let mut owner = ClockRegisters::host(&mut clocks, &mut lp).expect("host memory holds them");
    assert_eq!(
        bring_up(&mut owner, PROFILE, WAITS, &deadline),
        Err(BootFailure::Wait(WaitFailure::TimedOut("cpll-calibration")))
    );
    drop(owner);
    let view = HpSysClkrst::host(&mut clocks).expect("host memory holds it");
    assert_eq!(view.read_soc_clk_sel().reg_soc_clk_sel().get(), 0, "XTAL");
    assert_eq!(view.read_cpu_freq_ctrl0().reg_cpu_clk_div_num(), 0);
    assert!(
        !view.read_ana_pll_ctrl0().reg_cpu_pll_cal_stop(),
        "left running"
    );
    let lp = LpAonClkrst::host(&mut lp).expect("host memory holds it");
    assert_eq!(
        lp.read_lp_aonclkrst_cpll_div().lp_aonclkrst_cpll_fb_div(),
        8
    );
}

const SUSPEND: SuspendWaits = SuspendWaits {
    autoload: BootWait::new("icache-autoload", Duration::from_micros(20)),
    preload: BootWait::new("icache-preload", Duration::from_micros(20)),
    sync: BootWait::new("cache-sync-idle", Duration::from_micros(20)),
    lock: BootWait::new("cache-lock-idle", Duration::from_micros(20)),
    freeze: BootWait::new("icache-freeze", Duration::from_micros(20)),
    thaw: BootWait::new("icache-thaw", Duration::from_micros(20)),
};

#[test]
fn a_cache_that_never_reports_idle_has_fetch_opened_again() {
    // All zeros: autoload is off and never reports done.
    let mut words = memory(cache::SIZE, 0);
    let virtual_clock = VirtualClock::new();
    let ticking = Ticking(&virtual_clock);
    let deadline = BootDeadline::start(&ticking, budget());
    let mut owner = CacheRegisters::host(&mut words).expect("host memory holds it");
    let result = with_fetch_suspended(&mut owner, SUSPEND, &deadline, || ());
    assert_eq!(
        result,
        Err(BootFailure::Wait(WaitFailure::TimedOut("icache-autoload")))
    );
    drop(owner);
    let view = Cache::host(&mut words).expect("host memory holds it");
    assert!(!view.read_l1_icache_ctrl().l1_icache_shut_ibus0());
    assert!(
        !view
            .read_l1_icache0_autoload_ctrl()
            .l1_icache0_autoload_ena()
    );
}

#[test]
fn a_thaw_that_never_reports_restores_fetch_autoload_and_freeze() {
    // All ones: autoload runs, every idle flag and the freeze's done flag
    // read set, so the freeze takes effect and its thaw never reports.
    let mut words = memory(cache::SIZE, u32::MAX);
    let virtual_clock = VirtualClock::new();
    let ticking = Ticking(&virtual_clock);
    let deadline = BootDeadline::start(&ticking, budget());
    let mut owner = CacheRegisters::host(&mut words).expect("host memory holds it");
    let result = with_fetch_suspended(&mut owner, SUSPEND, &deadline, || ());
    assert_eq!(
        result,
        Err(BootFailure::Wait(WaitFailure::TimedOut("icache-thaw")))
    );
    drop(owner);
    let view = Cache::host(&mut words).expect("host memory holds it");
    assert!(
        !view.read_l1_icache_ctrl().l1_icache_shut_ibus0(),
        "fetch open"
    );
    assert!(
        view.read_l1_icache_ctrl().l1_icache_shut_ibus1(),
        "bus 1 kept"
    );
    assert!(
        view.read_l1_icache0_autoload_ctrl()
            .l1_icache0_autoload_ena(),
        "autoload restarted"
    );
    let freeze = view.read_l1_cache_freeze_ctrl();
    assert!(!freeze.l1_icache0_freeze_en(), "thawed");
    assert!(!freeze.l1_icache0_freeze_mode(), "mode 0");
    assert!(
        freeze.l1_dcache_freeze_en(),
        "the data cache's freeze is kept"
    );
}
