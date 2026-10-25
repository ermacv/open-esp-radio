//! The ESP32-S31 bootstrap's facts for the `oer-espressif-boot` sequences:
//! the CPU clock profiles, the flash read commands and timing candidates,
//! the octal PSRAM's profile, the named boot waits and the boot budget below
//! the RTC watchdog the bootloader armed.
//!
//! Every bound below is checked against the budget when the crate compiles.
//! Where the vendor polls without a limit, the bound is this crate's,
//! stated with the poll it replaces; a first-boot stand run confirms it.

pub use oer_esp32s31_pac::boot::{
    BootBlocks, BootHandoff, BusDividers, CacheMap, CacheRegisters, ClockRegisters, CpllDividers,
    CpuRoot, FlashRegisters, FlashTimingCandidate, MemDivider, PsramRegisters, PsramSupplyLevel,
};
pub use oer_espressif_boot::{
    BootFailure,
    cache::{CacheGeometry, SuspendWaits},
    clock::{ClockRates, ClockWaits, CpuClockProfile},
    flash::{FlashGeometry, FlashReadCommand, FlashReadCommands, TuningWaits},
    psram::{CacheAccess, ChipSelectTiming, OctalPsramProfile, PsramWaits},
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

/// One user command of the flash or PSRAM command port: 64 bytes or a
/// 16-bit mode register at the bootloader's 80 MHz take microseconds. The
/// ROM's `Wait_SPI_Idle` and `SPI_read_data` and esp-hal 1f646c5
/// `esp-hal/src/psram/oct_hex.rs:369-381` poll `CMD.USR` without a time
/// limit; this bound is this crate's.
pub const COMMAND_WAIT: BootWait = BootWait::new("spi-command", Duration::from_millis(1));

/// The flash timing sweep's waits.
pub const TUNING_WAITS: TuningWaits = TuningWaits {
    command: COMMAND_WAIT,
    suspend: SUSPEND_WAITS,
};

const fn read_command(opcode: u8, address_bits: u8, dummy_cycles: u8) -> FlashReadCommand {
    FlashReadCommand {
        opcode,
        address_bits,
        dummy_cycles,
    }
}

/// The read command of each read mode, as the ROM's flash read sends it:
/// a 24-bit address (the ROM's `rd_addr_bit_len`, ESP-IDF's
/// `SPI1_R_QIO_ADDR_BITSLEN` 23 + 1), QIO and DIO with their continuous-mode
/// bits in the address phase, and the ROM's dummy cycles plus its
/// `dummy_len_plus[1]`, which is zero on this chip.
///
/// SOURCE(esp32s31): pinned rev0 ROM (esp-rom-elfs 20260528, SHA-256
/// a52ad751...ab87), Blobray function records of `_esp_rom_spiflash_read`
/// @0x2f80f0c2 (QIO 0xEB, 8 more address bits, dummy + 4; DIO 0xBB, 4 more,
/// dummy + 2; QOUT 0x6B, DOUT 0x3B and fast read 0x0B, dummy + 8; slow read
/// 0x03, dummy + 0) and `spi_dummy_len_fix` @0x2f80df7e (stores zero to
/// `dummy_len_plus`); ESP-IDF 4d59230d
/// `components/esp_rom/esp32s31/include/esp32s31/rom/spi_flash.h:54-60`
/// (24-bit read addresses) and
/// `components/bootloader_support/bootloader_flash/src/bootloader_flash_config_esp32s31.c`
/// (the S31 bootloader sets no `dummy_len_plus`).
pub const FLASH_READ_COMMANDS: FlashReadCommands = FlashReadCommands {
    qio: read_command(0xeb, 24 + 8, 4),
    dio: read_command(0xbb, 24 + 4, 2),
    qout: read_command(0x6b, 24, 8),
    dout: read_command(0x3b, 24, 8),
    fast_read: read_command(0x0b, 24, 8),
    slow_read: read_command(0x03, 24, 0),
};

/// The flash of `capacity` bytes, the module's.
pub const fn flash_geometry(capacity: u32) -> FlashGeometry {
    FlashGeometry {
        capacity,
        commands: FLASH_READ_COMMANDS,
    }
}

const fn candidate(din_mode: u32, din_num: u32, extra_dummy: u32) -> FlashTimingCandidate {
    FlashTimingCandidate {
        din_mode: Bits::truncate(din_mode),
        din_num: Bits::truncate(din_num),
        extra_dummy: Bits::truncate(extra_dummy),
    }
}

/// The 120-MHz STR timing candidates, in the sweep's order: input delay
/// mode, delay number and extra dummy cycles.
///
/// SOURCE: esp-hal 1f646c5 `esp-hal/src/flash/timing.rs:53-66`
/// (`FLASH_120MHZ_TIMING`, ESP-IDF's 120-MHz STR table of the related P4,
/// C5 and C61 MSPI timing block).
pub const FLASH_TIMING_CANDIDATES: [FlashTimingCandidate; 12] = [
    candidate(2, 0, 1),
    candidate(0, 0, 0),
    candidate(2, 2, 2),
    candidate(2, 1, 2),
    candidate(2, 0, 2),
    candidate(0, 0, 1),
    candidate(2, 2, 3),
    candidate(2, 1, 3),
    candidate(2, 0, 3),
    candidate(0, 0, 2),
    candidate(2, 2, 4),
    candidate(2, 1, 4),
];

/// The PSRAM PHY supply's level, in millivolts.
///
/// SOURCE: esp-hal 1f646c5 `esp-hal/src/soc/esp32s31/clocks.rs:335-337`.
pub const PSRAM_SUPPLY_MV: u32 = 1800;

/// The external LDO's reference and multiplier codes nearest `millivolts`:
/// `vout = vref × (4 + mul) / 4 × 20`, `vref` 10 + dref below 9, else
/// 20 + 2 × (dref − 9); the first closest pair in order.
///
/// SOURCE: esp-hal 1f646c5 `esp-hal/src/soc/esp32s31/clocks.rs:353-383`
/// (`ldo_voltage_to_params`, without eFuse calibration).
pub const fn psram_supply_level(millivolts: u32) -> PsramSupplyLevel {
    let mut best = u32::MAX;
    let mut level = (0, 0);
    let mut dref = 0;
    while dref < 16 {
        let vref_20 = if dref < 9 {
            10 + dref
        } else {
            20 + (dref - 9) * 2
        };
        let mut mul = 0;
        while mul < 8 {
            let vout = vref_20 * 1000 * (4000 + mul * 1000);
            let diff = (millivolts * 80_000).abs_diff(vout);
            if diff < best {
                best = diff;
                level = (dref, mul);
            }
            mul += 1;
        }
        dref += 1;
    }
    PsramSupplyLevel {
        dref: Bits::truncate(level.0),
        mul: Bits::truncate(level.1),
    }
}

/// The memory PLL's rate, the PSRAM controllers' source.
///
/// SOURCE: ESP-IDF 4d59230d
/// `components/esp_psram/device/esp_psram_impl_ap_oct.c:60-61`
/// (`AP_OCT_PSRAM_MPLL_DEFAULT_FREQ_MHZ` 500 at 250 MHz).
pub const MPLL_HZ: u32 = 500_000_000;

/// The octal PSRAM's clock.
pub const PSRAM_HZ: u32 = 250_000_000;

/// The board's octal PSRAM at 250 MHz, of `size` bytes (the module's).
///
/// SOURCE: esp-hal 1f646c5 `esp-hal/src/psram/esp32s31.rs:97-105`
/// (`PsramTimingParams::MHZ_250`: MR0 read latency 6, MR4 write latency 3,
/// cache dummies 33 and 15 as the controller holds them, register dummy
/// 16), `esp-hal/src/psram/oct_hex.rs:270-284` and ESP-IDF 4d59230d
/// `components/esp_psram/device/esp_psram_impl_ap_oct.c:55-58` (chip
/// select setup and hold 4, hold delay 3).
pub const fn octal_psram(size: u32) -> OctalPsramProfile<PsramSupplyLevel> {
    OctalPsramProfile {
        supply: psram_supply_level(PSRAM_SUPPLY_MV),
        divider: (MPLL_HZ / PSRAM_HZ) as u8,
        read_latency: 6,
        write_latency: 3,
        register_dummy_bits: 16,
        chip_select: ChipSelectTiming {
            setup: 4,
            hold: 4,
            hold_delay: 3,
        },
        cache: CacheAccess {
            read_dummy_cyclelen: 33,
            write_dummy_cyclelen: 15,
        },
        size,
    }
}

const _: () = assert!(
    MPLL_HZ.is_multiple_of(PSRAM_HZ) && MPLL_HZ / PSRAM_HZ <= u8::MAX as u32,
    "the PSRAM clock divides the memory PLL's"
);

/// The PSRAM bring-up's waits.
pub const PSRAM_WAITS: PsramWaits = PsramWaits {
    // SOURCE: esp-hal 1f646c5 `esp-hal/src/soc/esp32s31/clocks.rs:347`
    // (1000 µs after the supply's current limit ends).
    supply_settle: BootWait::new("psram-supply", Duration::from_millis(1)),
    command: COMMAND_WAIT,
    cache_sync: CACHE_SYNC,
};

const _: () = {
    let budget = BOOT_BUDGET;
    assert!(budget.admits(COMMAND_WAIT));
    assert!(budget.admits(PSRAM_WAITS.supply_settle));
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
