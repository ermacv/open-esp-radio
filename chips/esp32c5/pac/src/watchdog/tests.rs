//! The watchdog owners over host memory: each test fills the words with a
//! pattern, runs one sequence of `oer-espressif-watchdog` on an owner and
//! reads the fields back through a typed view of the same words, never
//! through an address or a mask.

use core::num::NonZeroU32;
use std::{vec, vec::Vec};

use oer_espressif_watchdog::{
    LOCK, MainWatchdog, MicrosecondTick, Microseconds, RtcWatchdog, StoppedMainWatchdog,
    SuperWatchdog,
};

use super::{Mwdt0Registers, Mwdt1Registers, RwdtRegisters, SwdRegisters};
use crate::generated::{lp_wdt, lp_wdt_swd, pcr_timg1, timg0_wdt, timg1_wdt};

fn memory(size: usize, fill: u32) -> Vec<u32> {
    vec![fill; size / 4]
}

fn tick() -> MicrosecondTick {
    MicrosecondTick::from_source_hz(40_000_000).expect("40 MHz is whole megahertz")
}

#[test]
fn stopping_a_main_watchdog_clears_only_enable_and_flash_boot() {
    let mut words = memory(timg0_wdt::SIZE, u32::MAX);
    let _stopped =
        StoppedMainWatchdog::new(Mwdt0Registers::host(&mut words).expect("host memory holds it"));
    let view = Mwdt0Registers::host(&mut words).expect("host memory holds the block");
    let config = view.0.read_wdtconfig0();
    assert!(!config.wdt_en() && !config.wdt_flashboot_mod_en());
    assert_eq!(config.wdt_stg0().get(), 3, "the stage actions are kept");
    assert_eq!(config.wdt_sys_reset_length().get(), 7);
    assert!(config.wdt_appcpu_reset_en() && config.wdt_procpu_reset_en());
    assert_eq!(view.0.read_wdtwprotect().wdt_wkey(), LOCK, "locked again");
}

#[test]
fn arming_a_main_watchdog_writes_its_timeout_stages_and_reset_lengths() {
    let mut words = memory(timg1_wdt::SIZE, 0);
    let mut clock = memory(pcr_timg1::SIZE, 0);
    let mut watchdog = MainWatchdog::new(
        Mwdt1Registers::host(&mut words, &mut clock).expect("host memory holds the blocks"),
        tick(),
    );
    watchdog.arm_system_reset(Microseconds::new(
        NonZeroU32::new(1_000_000).expect("nonzero"),
    ));
    let view = Mwdt1Registers::host(&mut words, &mut clock).expect("host memory holds the blocks");
    let config = view.wdt.read_wdtconfig0();
    assert!(config.wdt_en() && !config.wdt_flashboot_mod_en());
    assert_eq!(
        [
            config.wdt_stg0(),
            config.wdt_stg1(),
            config.wdt_stg2(),
            config.wdt_stg3()
        ]
        .map(oer_mmio::Bits::get),
        [3, 0, 0, 0]
    );
    assert_eq!(config.wdt_cpu_reset_length().get(), 7);
    assert_eq!(config.wdt_sys_reset_length().get(), 7);
    assert!(
        !config.wdt_appcpu_reset_en(),
        "the reset masks are not written"
    );
    assert_eq!(view.wdt.read_wdtconfig1().wdt_clk_prescale(), 40);
    assert_eq!(view.wdt.read_wdtconfig2().wdt_stg0_hold(), 1_000_000);
    assert_eq!(view.wdt.read_wdtwprotect().wdt_wkey(), LOCK, "locked again");
}

#[test]
fn group_one_runs_out_of_reset_with_its_watchdog_on_xtal() {
    let mut words = memory(timg1_wdt::SIZE, 0);
    let mut clock = memory(pcr_timg1::SIZE, u32::MAX);
    let mut mwdt =
        Mwdt1Registers::host(&mut words, &mut clock).expect("host memory holds the blocks");
    mwdt.start_clock();
    let group = mwdt.clock.read_timergroup1_conf();
    assert!(group.tg1_clk_en() && !group.tg1_rst_en());
    let wdt_clock = mwdt.clock.read_timergroup1_wdt_clk_conf();
    assert!(wdt_clock.tg1_wdt_clk_en());
    assert_eq!(wdt_clock.tg1_wdt_clk_sel().get(), 0, "XTAL");
}

#[test]
fn stopping_the_rtc_watchdog_keeps_its_reset_lengths_and_sleep_pause() {
    let mut words = memory(lp_wdt::SIZE, u32::MAX);
    RtcWatchdog::new(RwdtRegisters::host(&mut words).expect("host memory holds it")).disable();
    let view = RwdtRegisters::host(&mut words).expect("host memory holds the block");
    let config = view.0.read_config0();
    assert!(!config.wdt_en() && !config.wdt_flashboot_mod_en());
    assert!(
        config.wdt_pause_in_slp(),
        "a whole-word zero would clear it"
    );
    assert_eq!(config.wdt_cpu_reset_length().get(), 7);
    assert_eq!(config.wdt_stg0().get(), 7);
    assert_eq!(view.0.read_wprotect().wdt_wkey(), LOCK, "locked again");
}

#[test]
fn the_super_watchdog_feeds_itself_and_keeps_its_signal_width() {
    let mut words = memory(lp_wdt_swd::SIZE, 0x12c0_0000);
    SuperWatchdog::new(SwdRegisters::host(&mut words).expect("host memory holds it"))
        .feed_automatically();
    let view = SwdRegisters::host(&mut words).expect("host memory holds the block");
    let config = view.0.read_swd_config();
    assert!(config.swd_auto_feed_en() && !config.swd_disable());
    assert_eq!(config.swd_signal_width().get(), 300);
    assert_eq!(view.0.read_swd_wprotect().swd_wkey(), LOCK, "locked again");
}
