//! The watchdog owners over host memory: each test fills the words with a
//! pattern, runs one register trait call and reads the fields back through
//! the typed block, never through an address or a mask.

use std::{vec, vec::Vec};

use oer_espressif_watchdog::{
    LOCK, MainConfig, MainWatchdogRegisters, RtcWatchdogRegisters, SuperWatchdogRegisters,
    WRITE_KEY, WriteProtected,
};

use super::{Mwdt0Registers, Mwdt1Registers, RwdtRegisters, SwdRegisters};
use crate::generated::{hp_sys_clkrst_timg1, lp_wdt, lp_wdt_swd, timg0_wdt, timg1_wdt};

fn memory(size: usize, fill: u32) -> Vec<u32> {
    vec![fill; size / 4]
}

#[test]
fn stopping_a_main_watchdog_clears_only_enable_and_flash_boot() {
    let mut words = memory(timg0_wdt::SIZE, u32::MAX);
    let mut mwdt = Mwdt0Registers::host(&mut words).expect("host memory holds the block");
    mwdt.set_running(false, false);
    let config = mwdt.0.read_wdtconfig0();
    assert!(!config.wdt_en() && !config.wdt_flashboot_mod_en());
    assert_eq!(config.wdt_stg0().get(), 3, "the stage actions are kept");
    assert_eq!(config.wdt_sys_reset_length().get(), 7);
    assert!(config.wdt_appcpu_reset_en() && config.wdt_procpu_reset_en());
}

#[test]
fn configuring_a_main_watchdog_writes_every_stage_and_reset_length() {
    let mut words = memory(timg1_wdt::SIZE, 0);
    let mut clock = memory(hp_sys_clkrst_timg1::SIZE, 0);
    let mut mwdt =
        Mwdt1Registers::host(&mut words, &mut clock).expect("host memory holds the blocks");
    mwdt.configure(MainConfig::SYSTEM_RESET_ON_STAGE0);
    let config = mwdt.wdt.read_wdtconfig0();
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
}

#[test]
fn the_timeout_lands_in_the_prescaler_and_the_stage0_hold() {
    let mut words = memory(timg0_wdt::SIZE, 0);
    let mut mwdt = Mwdt0Registers::host(&mut words).expect("host memory holds the block");
    mwdt.set_prescale(40);
    mwdt.set_stage0_timeout(1_000_000);
    mwdt.reset_divider();
    mwdt.latch();
    mwdt.feed();
    assert_eq!(mwdt.0.read_wdtconfig1().wdt_clk_prescale(), 40);
    assert_eq!(mwdt.0.read_wdtconfig2().wdt_stg0_hold(), 1_000_000);
    assert!(
        !mwdt.0.read_wdtconfig0().wdt_en(),
        "neither trigger enables"
    );
}

#[test]
fn the_write_key_is_stored_whole_and_cleared_to_lock() {
    let mut words = memory(timg0_wdt::SIZE, 0);
    let mut mwdt = Mwdt0Registers::host(&mut words).expect("host memory holds the block");
    mwdt.store_write_key(WRITE_KEY);
    assert_eq!(mwdt.0.read_wdtwprotect().wdt_wkey(), WRITE_KEY);
    mwdt.store_write_key(LOCK);
    assert_eq!(mwdt.0.read_wdtwprotect().wdt_wkey(), LOCK);

    let mut words = memory(lp_wdt::SIZE, 0);
    let mut rwdt = RwdtRegisters::host(&mut words).expect("host memory holds the block");
    rwdt.store_write_key(WRITE_KEY);
    assert_eq!(rwdt.0.read_wprotect().wdt_wkey(), WRITE_KEY);

    let mut words = memory(lp_wdt_swd::SIZE, 0);
    let mut swd = SwdRegisters::host(&mut words).expect("host memory holds the block");
    swd.store_write_key(WRITE_KEY);
    assert_eq!(swd.0.read_swd_wprotect().swd_wkey(), WRITE_KEY);
}

#[test]
fn group_one_runs_out_of_reset_with_its_watchdog_on_xtal() {
    let mut words = memory(timg1_wdt::SIZE, 0);
    let mut clock = memory(hp_sys_clkrst_timg1::SIZE, u32::MAX);
    let mut mwdt =
        Mwdt1Registers::host(&mut words, &mut clock).expect("host memory holds the blocks");
    mwdt.start_clock();
    let control = mwdt.clock.read_timergrp1_ctrl0();
    assert!(control.apb_clk_en() && !control.rst_en() && control.wdt_clk_en());
    assert_eq!(control.wdt_src_sel().get(), 0, "XTAL");
}

#[test]
fn stopping_the_rtc_watchdog_keeps_its_reset_lengths_and_sleep_pause() {
    let mut words = memory(lp_wdt::SIZE, u32::MAX);
    let mut rwdt = RwdtRegisters::host(&mut words).expect("host memory holds the block");
    rwdt.set_running(false, false);
    let config = rwdt.0.read_config0();
    assert!(!config.wdt_en() && !config.wdt_flashboot_mod_en());
    assert!(
        config.wdt_pause_in_slp(),
        "a whole-word zero would clear it"
    );
    assert_eq!(config.wdt_cpu_reset_length().get(), 7);
    assert_eq!(config.wdt_stg0().get(), 7);
}

#[test]
fn the_super_watchdog_feeds_itself_and_keeps_its_signal_width() {
    let mut words = memory(lp_wdt_swd::SIZE, 0x12c0_0000);
    let mut swd = SwdRegisters::host(&mut words).expect("host memory holds the block");
    swd.set_auto_feed(true);
    let config = swd.0.read_swd_config();
    assert!(config.swd_auto_feed_en() && !config.swd_disable());
    assert_eq!(config.swd_signal_width().get(), 300);
}
