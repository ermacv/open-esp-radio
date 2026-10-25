//! The ESP32-S31 bootstrap's facts for the `oer-espressif-boot` sequences:
//! the CPU clock profiles, the named boot waits and the boot budget below
//! the RTC watchdog the bootloader armed.
//!
//! Every bound below is checked against the budget when the crate compiles.
//! Where the vendor polls without a limit, the bound is this crate's,
//! stated with the poll it replaces; a first-boot stand run confirms it.

pub use oer_esp32s31_pac::boot::{
    BootBlocks, BootHandoff, BusDividers, CacheMap, CacheRegisters, ClockRegisters, CpllDividers,
    CpuRoot, MemDivider,
};
pub use oer_espressif_boot::{
    BootFailure,
    cache::{CacheGeometry, SuspendWaits},
    clock::{ClockRates, ClockWaits, CpuClockProfile},
    wait::{BootBudget, BootDeadline, BootWait},
};
use oer_mmio::Bits;
use oer_time::Duration;

#[cfg(test)]
mod tests;

/// The CPU PLL's rate.
///
/// SOURCE: ESP-IDF 4d59230d
/// `components/esp_hal_clock/esp32s31/include/hal/clk_tree_ll.h:448-453`
/// (`CLK_LL_PLL_320M_FREQ_MHZ`, the only CPLL rate, from the 40 MHz XTAL).
pub const CPLL_HZ: u32 = 320_000_000;

/// The CPU PLL's dividers for [`CPLL_HZ`]: feedback 8 over reference 1.
///
/// SOURCE: esp-hal 1f646c5 `esp-hal/src/soc/esp32s31/clocks.rs:209-213`.
pub const CPLL_320: CpllDividers = CpllDividers {
    feedback: 8,
    reference: Bits::constant::<1>(),
};

/// An image's CPU clock profile.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CpuProfile {
    /// 160 MHz, the default of every image.
    Mhz160,
    /// 320 MHz, for the HIL images that ask for it.
    Mhz320,
}

impl CpuProfile {
    /// The profile's root, PLL, dividers and rates.
    ///
    /// SOURCE: esp-hal 1f646c5 `esp-hal/src/soc/esp32s31/clocks.rs:41-91`
    /// (`PRESET_160`: CPLL, CPU 1, memory 0, system 1, APB 1;
    /// `PRESET_320`: CPLL, CPU 0, memory 1, system 2, APB 1; the memory
    /// clock at most 160 MHz).
    pub const fn clock(self) -> CpuClockProfile<CpuRoot, BusDividers, CpllDividers> {
        let dividers = match self {
            Self::Mhz160 => BusDividers {
                cpu: 1,
                mem: MemDivider::Same,
                sys: 1,
                apb: 1,
            },
            Self::Mhz320 => BusDividers {
                cpu: 0,
                mem: MemDivider::Half,
                sys: 2,
                apb: 1,
            },
        };
        CpuClockProfile {
            root: CpuRoot::Cpll,
            pll: Some(CPLL_320),
            dividers,
            rates: rates(CPLL_HZ, dividers),
        }
    }
}

/// The rates `dividers` give from a root of `root_hz`: the CPU divides the
/// root, the memory and system clocks divide the CPU clock, the APB clock
/// divides the system clock, each by its code plus one.
///
/// SOURCE: esp-hal 1f646c5
/// `esp-metadata-generated/src/_generated_esp32s31.rs:3662-3748`.
pub const fn rates(root_hz: u32, dividers: BusDividers) -> ClockRates {
    let cpu_hz = root_hz / (dividers.cpu as u32 + 1);
    let mem_hz = match dividers.mem {
        MemDivider::Same => cpu_hz,
        MemDivider::Half => cpu_hz / 2,
    };
    let sys_hz = cpu_hz / (dividers.sys as u32 + 1);
    ClockRates {
        cpu_hz,
        mem_hz,
        sys_hz,
        apb_hz: sys_hz / (dividers.apb as u32 + 1),
    }
}

/// The highest memory clock.
///
/// SOURCE: esp-hal 1f646c5 `esp-hal/src/soc/esp32s31/clocks.rs:44,61,78`
/// (`MEM_CLK` "Max 160 MHz").
pub const MAX_MEM_HZ: u32 = 160_000_000;

const _: () = {
    assert!(CpuProfile::Mhz160.clock().rates.cpu_hz == 160_000_000);
    assert!(CpuProfile::Mhz320.clock().rates.cpu_hz == 320_000_000);
    assert!(CpuProfile::Mhz160.clock().rates.mem_hz <= MAX_MEM_HZ);
    assert!(CpuProfile::Mhz320.clock().rates.mem_hz <= MAX_MEM_HZ);
};

/// The RTC watchdog's timeout as the ESP-IDF bootloader arms it, counted
/// from the bootloader's start.
///
/// SOURCE: ESP-IDF 4d59230d `components/bootloader/Kconfig.projbuild:311-315`
/// (`CONFIG_BOOTLOADER_WDT_TIME_MS`, 9000 by default) and
/// `components/bootloader_support/src/bootloader_init.c:91-99`.
pub const BOOTLOADER_RTC_WATCHDOG: Duration = Duration::from_secs(9);

/// The bootstrap's whole bring-up, from its first wait to its hand-off. It
/// leaves the bootloader the rest of [`BOOTLOADER_RTC_WATCHDOG`].
pub const BOOT_BUDGET: BootBudget =
    match BootBudget::below(Duration::from_secs(2), BOOTLOADER_RTC_WATCHDOG) {
        Some(budget) => budget,
        None => panic!("the boot budget lies below the bootloader's RTC watchdog"),
    };

/// The clock bring-up's waits.
pub const CLOCK_WAITS: ClockWaits = ClockWaits {
    // ESP-IDF 4d59230d `components/esp_hw_support/port/esp32s31/rtc_clk.c:144`
    // polls `clk_ll_cpll_calibration_is_done` without a limit.
    calibration: BootWait::new("cpll-calibration", Duration::from_millis(1)),
    // SOURCE: `rtc_clk.c:145` ("wait for true stop") and esp-hal 1f646c5
    // `esp-hal/src/soc/esp32s31/clocks.rs:243`: 10 µs.
    calibration_settle: BootWait::new("cpll-settle", Duration::from_micros(10)),
    // ESP-IDF 4d59230d `clk_tree_ll.h:602-606` (`clk_ll_bus_update`) polls
    // `SOC_CLK_UPDATE` without a limit.
    update: BootWait::new("clock-update", Duration::from_micros(100)),
};

/// The L1 cache's publication geometry: 64-byte lines, the data cache
/// written back twice, both instruction caches invalidated.
///
/// SOURCE: esp-hal 1f646c5 `esp-hal/src/soc/esp32s31/cache.rs:3-5,35-61`;
/// ESP-IDF 4d59230d
/// `components/esp_rom/patches/esp_rom_cache_writeback_esp32p4_esp32s31.c:39-50`;
/// `components/soc/esp32s31/register/soc/cache_reg.h:1633-1660`
/// (`SYNC_MAP`; `SYNC_SIZE` 28 bits).
pub const CACHE_GEOMETRY: CacheGeometry<CacheMap> = CacheGeometry {
    line_bytes: 64,
    max_range_bytes: (1 << 28) - 64,
    data: CacheMap::Data,
    writeback_passes: 2,
    instruction: Some(CacheMap::BothInstructionCaches),
};

/// One sync command over a range of at most the stage-two image. ESP-IDF
/// 4d59230d `esp_rom_cache_writeback_esp32p4_esp32s31.c:39-50` and esp-hal
/// `cache.rs:35-61` poll `SYNC_DONE` without a limit.
pub const CACHE_SYNC: BootWait = BootWait::new("cache-sync", Duration::from_millis(50));

/// The fetch suspension's waits: each replaces an unbounded poll of the
/// pinned rev0 ROM's `Cache_Suspend_L1_CORE0_ICache` and its callees
/// (`oer_espressif_boot::cache`).
pub const SUSPEND_WAITS: SuspendWaits = SuspendWaits {
    autoload: BootWait::new("icache-autoload", Duration::from_millis(1)),
    preload: BootWait::new("icache-preload", Duration::from_millis(1)),
    sync: BootWait::new("cache-sync-idle", Duration::from_millis(50)),
    lock: BootWait::new("cache-lock-idle", Duration::from_millis(1)),
    freeze: BootWait::new("icache-freeze", Duration::from_millis(1)),
    thaw: BootWait::new("icache-thaw", Duration::from_millis(1)),
};

const _: () = {
    let budget = BOOT_BUDGET;
    assert!(budget.admits(CLOCK_WAITS.calibration));
    assert!(budget.admits(CLOCK_WAITS.calibration_settle));
    assert!(budget.admits(CLOCK_WAITS.update));
    assert!(budget.admits(CACHE_SYNC));
    assert!(budget.admits(SUSPEND_WAITS.autoload));
    assert!(budget.admits(SUSPEND_WAITS.preload));
    assert!(budget.admits(SUSPEND_WAITS.sync));
    assert!(budget.admits(SUSPEND_WAITS.lock));
    assert!(budget.admits(SUSPEND_WAITS.freeze));
    assert!(budget.admits(SUSPEND_WAITS.thaw));
};
