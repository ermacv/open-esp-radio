use oer_time::Duration;

use super::{
    BOOT_BUDGET, BOOTLOADER_RTC_WATCHDOG, ClockRates, CpuProfile, CpuRoot, FLASH_READ_COMMANDS,
    FLASH_TIMING_CANDIDATES, MAX_MEM_HZ, MemDivider, octal_psram, psram_supply_level,
};

#[test]
fn the_default_profile_is_the_fast_one() {
    assert_eq!(CpuProfile::DEFAULT, CpuProfile::Mhz320);
}

#[test]
fn the_slow_profile_runs_the_cpu_at_160_mhz_from_the_cpll() {
    let profile = CpuProfile::Mhz160.clock();
    assert_eq!(profile.root, CpuRoot::Cpll);
    assert_eq!(
        profile.rates,
        ClockRates {
            cpu_hz: 160_000_000,
            mem_hz: 160_000_000,
            sys_hz: 80_000_000,
            apb_hz: 40_000_000,
        }
    );
}

#[test]
fn the_fast_profile_runs_the_cpu_at_320_mhz_and_halves_the_memory_clock_to_its_limit() {
    let profile = CpuProfile::Mhz320.clock();
    assert_eq!(profile.dividers.mem, MemDivider::Half);
    assert_eq!(profile.rates.cpu_hz, 320_000_000);
    assert_eq!(profile.rates.mem_hz, MAX_MEM_HZ);
    assert_eq!(profile.rates.sys_hz, 106_666_666);
    assert_eq!(profile.rates.apb_hz, 53_333_333);
}

#[test]
fn both_profiles_share_the_cpll_so_switching_never_recalibrates_it() {
    assert_eq!(
        CpuProfile::Mhz160.clock().pll,
        CpuProfile::Mhz320.clock().pll
    );
}

#[test]
fn the_boot_leaves_the_bootloader_most_of_its_rtc_watchdog() {
    let left = BOOTLOADER_RTC_WATCHDOG.saturating_sub(BOOT_BUDGET.duration());
    assert!(left >= Duration::from_secs(5), "{left:?}");
}

#[test]
fn the_psram_supply_is_the_ldo_codes_that_give_exactly_1800_mv() {
    let level = psram_supply_level(1800);
    assert_eq!((level.dref.get(), level.mul.get()), (6, 5));
    // vref 16 (10 + 6) with mul 5: the formula gives 1800 mV exactly.
    assert_eq!((10 + 6) * (4 + 5) * 1000 * 1000, 1800 * 80_000);
}

#[test]
fn the_octal_psram_divides_the_500_mhz_memory_pll_by_two() {
    let profile = octal_psram(16 * 1024 * 1024);
    assert_eq!(profile.divider, 2);
    assert_eq!(profile.size, 16 * 1024 * 1024);
}

#[test]
fn qio_reads_send_their_mode_bits_in_the_address_phase() {
    let qio = FLASH_READ_COMMANDS.qio;
    assert_eq!(
        (qio.opcode, qio.address_bits, qio.dummy_cycles),
        (0xeb, 32, 4)
    );
    let slow = FLASH_READ_COMMANDS.slow_read;
    assert_eq!((slow.opcode, slow.dummy_cycles), (0x03, 0));
}

#[test]
fn the_sweep_has_a_candidate_per_mask_bit() {
    assert!(FLASH_TIMING_CANDIDATES.len() <= 16);
    assert_eq!(FLASH_TIMING_CANDIDATES[1].extra_dummy.get(), 0);
}
