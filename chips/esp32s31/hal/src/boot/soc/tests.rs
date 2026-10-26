//! The SoC initialization's steps over host memory from each instance's
//! base, against ESP-IDF 4d59230d's register headers: `lp_apm_reg.h:990`
//! (`FUNC_CTRL` 0xbc), `hp_apm_reg.h:1950` and `hp_mem_apm_reg.h:1214`
//! (`FUNC_CTRL` 0xc4), `trng_reg.h` (`CONF` 0x0: NOISE_SOURCE_SEL 4:0,
//! NOISE_POS_SEL 9:5, REPETITION_VALUE_C 18:10, ADPATIVE_VALUE_C 27:19,
//! RANDOM_OUTPUT_MODE 29, NOISE_CRC_EN 30, SAMPLE_ENABLE 31; `DEBUG_CONF`
//! 0x4: STARTUP_TEST_START 0, STARTUP_TEST_LIMIT 16:1, HEALTH_TEST_BYPASS
//! 30; `DATE` 0xfc: CLK_EN 28) and `lp_periclkrst_reg.h:324-338`
//! (`RNG_CTRL` 0x3c: LP_RNG_CLK_EN 30, LP_RNG_RST_EN 31).

use std::{vec, vec::Vec};

use oer_esp32s31_pac::{
    hp_apm::HpApmFilter, hp_mem_apm::HpMemApmFilter, instances, lp_apm::LpApmFilter,
    lp_peri_clkrst::LpPericlkrstRngInit, trng::trng_host,
};

use super::{contain_hart1, open_apm_filters, start_trng};
use crate::platform::{
    HpCore,
    core_control::{CoreControlRegisters, HOST_SIZES},
};

fn memory(size: usize, fill: u32) -> Vec<u32> {
    vec![fill; size / 4]
}

/// The byte offsets of the words that differ.
fn changed(before: &[u32], after: &[u32]) -> Vec<usize> {
    before
        .iter()
        .zip(after)
        .enumerate()
        .filter(|(_, (a, b))| a != b)
        .map(|(index, _)| index * 4)
        .collect()
}

#[test]
fn the_apm_filters_open_every_master_and_keep_the_other_bits() {
    let mut lp = memory(instances::lp_apm::SIZE, u32::MAX);
    let mut hp = memory(instances::hp_apm::SIZE, u32::MAX);
    let mut mem = memory(instances::hp_mem_apm::SIZE, u32::MAX);
    let (lp_before, hp_before, mem_before) = (lp.clone(), hp.clone(), mem.clone());
    open_apm_filters(
        &mut LpApmFilter::host(&mut lp).expect("memory holds the block"),
        &mut HpApmFilter::host(&mut hp).expect("memory holds the block"),
        &mut HpMemApmFilter::host(&mut mem).expect("memory holds the block"),
    );
    assert_eq!(changed(&lp_before, &lp), [0xbc]);
    assert_eq!(lp[0xbc / 4], !0xf, "M0..M3 cleared");
    assert_eq!(changed(&hp_before, &hp), [0xc4]);
    assert_eq!(hp[0xc4 / 4], !0x7f, "M0..M6 cleared");
    assert_eq!(changed(&mem_before, &mem), [0xc4]);
    assert_eq!(mem[0xc4 / 4], !0x3f, "M0..M5 cleared");
}

#[test]
fn the_trng_runs_with_its_health_tests_and_starts_its_startup_test() {
    let mut trng = memory(instances::trng::SIZE, 0);
    let mut clock = memory(instances::lp_periclkrst::SIZE, 0);
    clock[0x3c / 4] = 1 << 31; // held in reset
    trng[1] = 1 << 30; // DEBUG_CONF at 0x4 // health tests bypassed
    let (trng_before, clock_before) = (trng.clone(), clock.clone());
    start_trng(
        &mut trng_host(&mut trng).expect("memory holds the block").0,
        &mut LpPericlkrstRngInit::host(&mut clock).expect("memory holds the block"),
    );
    assert_eq!(changed(&clock_before, &clock), [0x3c]);
    assert_eq!(clock[0x3c / 4], 1 << 30, "clock on, reset released");
    assert_eq!(changed(&trng_before, &trng), [0x0, 0x4, 0xfc]);
    assert_eq!(
        trng[0],
        0x10 | (0x10 << 5) | (0x1f << 10) | (0x12 << 19) | (0b111 << 29)
    );
    assert_eq!(
        trng[1],
        (1024 << 1) | 1,
        "the limit set, the bypass cleared and the startup test started"
    );
    assert_eq!(trng[0xfc / 4], 1 << 28, "the clock gate open");
}

#[test]
fn hart_1_is_stalled_held_and_left_no_boot_address() {
    let [stall, status, clocks, reset, system] = HOST_SIZES;
    let mut words = [
        memory(stall, 0),
        memory(status, 0),
        memory(clocks, 0),
        memory(reset, 0),
        memory(system, u32::MAX),
    ];
    {
        let [stall, status, clocks, reset, system] = &mut words;
        let mut cores = CoreControlRegisters::host(stall, status, clocks, reset, system)
            .expect("memory holds the blocks");
        contain_hart1(&mut cores);
        assert!(cores.stall_requested(HpCore::Core1));
        assert!(!cores.stall_requested(HpCore::Core0));
        assert!(cores.core1_held());
    }
    assert_eq!(words[4][0x164 / 4], 0, "the stale entry cleared");
    assert!(
        words[3].iter().all(|word| *word == 0),
        "no core reset pulsed"
    );
}
