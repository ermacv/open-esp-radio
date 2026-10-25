use oer_time::Duration;

use super::{
    BOOT_BUDGET, BOOTLOADER_RTC_WATCHDOG, ClockRates, CpuProfile, CpuRoot, MAX_MEM_HZ, MemDivider,
};

#[test]
fn the_default_profile_runs_the_cpu_at_160_mhz_from_the_cpll() {
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
fn the_fast_profile_halves_the_memory_clock_to_its_limit() {
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
