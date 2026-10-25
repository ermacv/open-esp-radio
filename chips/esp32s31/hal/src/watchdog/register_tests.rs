//! The watchdog owners over host memory from each instance's base: each
//! test fills the words with a pattern, runs one sequence of
//! `oer-espressif-watchdog` on an owner, reads the fields back through a
//! typed view of the same words and checks that no other word changed.
//!
//! Word offsets are ESP-IDF 4d59230d's: `timer_group_reg.h`
//! (`WDTCONFIG0` 0x48, `WDTCONFIG1` 0x4c, `WDTCONFIG2` 0x50, `WDTFEED`
//! 0x60, `WDTWPROTECT` 0x64), `hp_sys_clkrst_reg.h` (`TIMERGRP1_CTRL0`
//! 0x11c) and `lp_wdt_reg.h` (`CONFIG0` 0x0, `WPROTECT` 0x18,
//! `SWD_CONFIG` 0x1c, `SWD_WPROTECT` 0x20).

use core::num::NonZeroU32;
use std::{vec, vec::Vec};

use oer_espressif_watchdog::{
    LOCK, MainWatchdog, MicrosecondTick, Microseconds, RtcWatchdog, StoppedMainWatchdog,
    SuperWatchdog,
};

use super::{Mwdt0Registers, Mwdt1Registers, RwdtRegisters, SwdRegisters};
use oer_esp32s31_pac::instances;

fn memory(size: usize, fill: u32) -> Vec<u32> {
    vec![fill; size / 4]
}

/// The byte offsets of the words that differ between `before` and `after`.
fn changed(before: &[u32], after: &[u32]) -> Vec<usize> {
    before
        .iter()
        .zip(after)
        .enumerate()
        .filter(|(_, (a, b))| a != b)
        .map(|(index, _)| index * 4)
        .collect()
}

fn tick() -> MicrosecondTick {
    MicrosecondTick::from_source_hz(40_000_000).expect("40 MHz is whole megahertz")
}

#[test]
fn stopping_a_main_watchdog_clears_only_enable_and_flash_boot() {
    let mut words = memory(instances::timg0::SIZE, u32::MAX);
    let before = words.clone();
    let _stopped =
        StoppedMainWatchdog::new(Mwdt0Registers::host(&mut words).expect("host memory holds it"));
    let view = Mwdt0Registers::host(&mut words).expect("host memory holds the block");
    let config = view.0.read_wdtconfig0();
    assert!(!config.wdt_en() && !config.wdt_flashboot_mod_en());
    assert_eq!(config.wdt_stg0().get(), 3, "the stage actions are kept");
    assert_eq!(config.wdt_sys_reset_length().get(), 7);
    assert!(config.wdt_appcpu_reset_en() && config.wdt_procpu_reset_en());
    assert_eq!(view.0.read_wdtwprotect().wdt_wkey(), LOCK, "locked again");
    drop(view);
    assert_eq!(changed(&before, &words), [0x48, 0x64]);
}

#[test]
fn arming_a_main_watchdog_writes_its_timeout_stages_and_reset_lengths() {
    let mut words = memory(instances::timg1::SIZE, 0);
    let mut clock = memory(instances::hp_sys_clkrst::SIZE, 0);
    let mut watchdog = MainWatchdog::new(
        Mwdt1Registers::host(&mut words, &mut clock).expect("host memory holds the blocks"),
        tick(),
    );
    watchdog.arm_system_reset(Microseconds::new(
        NonZeroU32::new(1_000_000).expect("nonzero"),
    ));
    let mut untouched = memory(instances::timg1::SIZE, 0);
    untouched[0x48 / 4] = words[0x48 / 4];
    untouched[0x4c / 4] = words[0x4c / 4];
    untouched[0x50 / 4] = words[0x50 / 4];
    untouched[0x60 / 4] = words[0x60 / 4];
    assert_eq!(untouched, words, "no other word of the group changed");
    assert!(clock.iter().all(|word| *word == 0), "arming runs no clock");
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
    drop(view);
    // SOURCE: ESP-IDF 4d59230d `mwdt_ll.h:219-222`: the feed stores one.
    assert_eq!(words[0x60 / 4], 1);
}

#[test]
fn group_one_runs_out_of_reset_with_its_watchdog_on_xtal() {
    let mut words = memory(instances::timg1::SIZE, 0);
    let mut clock = memory(instances::hp_sys_clkrst::SIZE, u32::MAX);
    let before = clock.clone();
    let mut mwdt =
        Mwdt1Registers::host(&mut words, &mut clock).expect("host memory holds the blocks");
    mwdt.start_clock();
    let control = mwdt.clock.read_timergrp1_ctrl0();
    assert!(
        control.reg_timergrp1_apb_clk_en()
            && !control.reg_timergrp1_rst_en()
            && control.reg_timergrp1_wdt_clk_en()
    );
    assert_eq!(control.reg_timergrp1_wdt_src_sel().get(), 0, "XTAL");
    assert!(control.reg_timergrp1_force_norst(), "other fields are kept");
    drop(mwdt);
    assert_eq!(changed(&before, &clock), [0x11c]);
    assert!(words.iter().all(|word| *word == 0));
}

#[test]
fn stopping_the_rtc_watchdog_keeps_its_reset_lengths_and_sleep_pause() {
    let mut words = memory(instances::lp_wdt::SIZE, u32::MAX);
    let before = words.clone();
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
    drop(view);
    assert_eq!(changed(&before, &words), [0x0, 0x18]);
}

#[test]
fn the_super_watchdog_feeds_itself_and_keeps_its_signal_width() {
    let mut words = memory(instances::lp_wdt::SIZE, 0x12c0_0000);
    let before = words.clone();
    SuperWatchdog::new(SwdRegisters::host(&mut words).expect("host memory holds it"))
        .feed_automatically();
    let view = SwdRegisters::host(&mut words).expect("host memory holds the block");
    let config = view.0.read_swd_config();
    assert!(config.swd_auto_feed_en() && !config.swd_disable());
    assert_eq!(config.swd_signal_width().get(), 300);
    assert_eq!(view.0.read_swd_wprotect().swd_wkey(), LOCK, "locked again");
    drop(view);
    assert_eq!(changed(&before, &words), [0x1c, 0x20]);
}
