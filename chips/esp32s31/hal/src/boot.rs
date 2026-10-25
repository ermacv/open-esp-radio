//! The ESP32-S31 bootstrap's facts for the `oer-espressif-boot` sequences:
//! the CPU clock profiles, the flash read commands and timing candidates,
//! the octal PSRAM's profile, the named boot waits and the boot budget below
//! the RTC watchdog the bootloader armed.
//!
//! Every bound below is checked against the budget when the crate compiles.
//! Where the vendor polls without a limit, the bound is this crate's,
//! stated with the poll it replaces; a first-boot stand run confirms it.

mod active_power;
mod brownout;
mod cache;
mod clock;
mod flash;
mod psram;
mod soc;

pub use active_power::ActivePower;
pub use brownout::{BrownoutFailure, BrownoutRegisters, THRESHOLD_WAIT, apply_idf_policy};
pub use cache::{CacheMap, CacheRegisters};
pub use clock::{
    BusDividers, ClockRegisters, CpllDividers, CpuRoot, FlashClockWord, MemDivider,
    PsramSupplyLevel,
};
pub use flash::{FlashRegisters, FlashTimingCandidate, FlashTimingWords};
pub use oer_espressif_boot::{
    BootFailure,
    cache::{CacheGeometry, SuspendWaits},
    clock::{ClockRates, ClockWaits, CpuClockProfile},
    flash::{FlashGeometry, FlashReadCommand, FlashReadCommands, TuningWaits, XipWindow},
    mmu::MmuWindow,
    psram::{CacheAccess, ChipSelectTiming, OctalPsramProfile, PsramWaits},
    wait::{BootBudget, BootDeadline, BootWait},
};
pub use psram::PsramRegisters;
pub use soc::{Soc, SocFailure};

use oer_esp32s31_pac::{lp_ana::LpAnaBrownout, ownership::AnalogBusPeripherals};
use oer_mmio::Bits;
use oer_time::{Clock, Duration};

#[cfg(test)]
mod register_tests;
#[cfg(test)]
mod tests;

/// The flash MMU's window: 64-KiB pages from `0x4000_0000`, 1024 entries of
/// the flash controller's table, each naming one of 2048 physical pages
/// (its 11-bit page field).
///
/// SOURCE: ESP-IDF 4d59230d
/// `components/soc/esp32s31/include/soc/ext_mem_defs.h:19,75,85,93`
/// (`SOC_MMU_PAGE_SIZE`, `SOC_MMU_FLASH_VALID_VAL_MASK` 0x7ff,
/// `SOC_MMU_ENTRY_NUM`, `SOC_MMU_FLASH_VADDR_BASE`).
pub const FLASH_MMU_WINDOW: MmuWindow = MmuWindow {
    virtual_base: 0x4000_0000,
    page_bytes: 0x1_0000,
    first_entry: 0,
    entries: oer_esp32s31_pac::spi_mem_c::MMU_ENTRIES as u16,
    physical_pages: oer_esp32s31_pac::spi_mem_c::mmu_item_content::PADDR_MAX + 1,
};

/// The PSRAM MMU's window: 64-KiB pages from `0x5000_0000`, 1024 entries of
/// the PSRAM controller's table, each naming one of 1024 physical pages
/// (its 10-bit page field).
///
/// SOURCE: ESP-IDF 4d59230d
/// `components/soc/esp32s31/include/soc/ext_mem_defs.h:19,76,85,94`
/// (`SOC_MMU_PAGE_SIZE`, `SOC_MMU_PSRAM_VALID_VAL_MASK` 0x3ff,
/// `SOC_MMU_ENTRY_NUM`, `SOC_MMU_PSRAM_VADDR_BASE`).
pub const PSRAM_MMU_WINDOW: MmuWindow = MmuWindow {
    virtual_base: 0x5000_0000,
    page_bytes: 0x1_0000,
    first_entry: 0,
    entries: oer_esp32s31_pac::spi_mem_s::MMU_ENTRIES as u16,
    physical_pages: oer_esp32s31_pac::spi_mem_s::mmu_item_content::PADDR_MAX + 1,
};

/// The flash tuning region as instruction fetch maps it: the bootstrap's
/// own cold reference words, read through the cache, each read a volatile
/// load the compiler keeps.
pub struct XipRegion<'a>(&'a [u32]);

impl<'a> XipRegion<'a> {
    /// The region of `words`, which nothing else reads during the sweep.
    pub const fn new(words: &'a [u32]) -> Self {
        Self(words)
    }
}

impl XipWindow for XipRegion<'_> {
    /// Word `index` of the region.
    ///
    /// # Panics
    ///
    /// When `index` lies past the region: the sweep reads inside it.
    fn word(&self, index: usize) -> u32 {
        oer_memory::read_volatile(
            self.0
                .get(index)
                .expect("the sweep reads inside its region"),
        )
    }
}

/// The brownout detector and the analog bus whose threshold it writes,
/// owned by the bootstrap.
#[must_use = "dropping a register owner permanently loses its register authority"]
pub struct Brownout {
    registers: BrownoutRegisters,
    analog: AnalogBusPeripherals,
}

impl Brownout {
    /// Own the brownout part and the analog bus for the bootstrap.
    pub(crate) fn new(part: LpAnaBrownout, analog: AnalogBusPeripherals) -> Self {
        Self {
            registers: BrownoutRegisters::new(part),
            analog,
        }
    }

    /// Apply ESP-IDF's application brownout policy and check it reads back
    /// ([`apply_idf_policy`]).
    ///
    /// # Errors
    ///
    /// [`BrownoutFailure`].
    pub fn apply_idf_policy<C: Clock + ?Sized>(
        &mut self,
        deadline: &BootDeadline<'_, C>,
    ) -> Result<(), BrownoutFailure> {
        let mut bus = crate::phy::i2c::AnalogBus::new(&mut self.analog.i2c_ana_mst);
        apply_idf_policy(&mut self.registers, &mut bus, deadline)
    }
}

/// The CPU PLL's rate.
///
/// SOURCE: ESP-IDF 4d59230d
/// `components/esp_hal_clock/esp32s31/include/hal/clk_tree_ll.h:448-453`
/// (`CLK_LL_PLL_320M_FREQ_MHZ`, the only CPLL rate, from the 40 MHz XTAL).
pub const CPLL_HZ: u32 = 320_000_000;

/// The CPU PLL's dividers for [`CPLL_HZ`]: feedback 8 over reference 1.
///
/// SOURCE: ESP-IDF 4d59230d
/// `components/esp_hal_clock/esp32s31/include/hal/clk_tree_ll.h:422-440`
/// (`clk_ll_cpll_set_freq_mhz`: feedback 8, reference 1).
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
    /// SOURCE: ESP-IDF 4d59230d
    /// `components/esp_hw_support/port/esp32s31/rtc_clk.c:222-253`
    /// (`rtc_clk_cpu_freq_to_cpll_mhz`: 160 MHz divisors 2/1/2/2;
    /// 320 MHz divisors 1/2/3/2). The register codes are divisor minus one
    /// (`components/esp_hal_clock/esp32s31/include/hal/clk_tree_ll.h:664-750`).
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
/// SOURCE: ESP-IDF 4d59230d
/// `components/esp_hal_clock/esp32s31/include/hal/clk_tree_ll.h:664-762`
/// (integer dividers and the CPU -> memory/system -> APB clock tree).
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
/// SOURCE: ESP-IDF 4d59230d
/// `components/esp_hw_support/port/esp32s31/rtc_clk.c:224`
/// (`MEM_CLK <= 160MHz`).
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
    // SOURCE: ESP-IDF 4d59230d
    // `components/esp_hw_support/port/esp32s31/rtc_clk.c:145`:
    // 10 µs after calibration completes ("wait for true stop").
    calibration_settle: BootWait::new("cpll-settle", Duration::from_micros(10)),
    // ESP-IDF 4d59230d `clk_tree_ll.h:602-606` (`clk_ll_bus_update`) polls
    // `SOC_CLK_UPDATE` without a limit.
    update: BootWait::new("clock-update", Duration::from_micros(100)),
};

/// The L1 cache's publication geometry: 64-byte lines, the data cache
/// written back twice, both instruction caches invalidated.
///
/// SOURCE: ESP-IDF 4d59230d
/// `components/esp_rom/esp32s31/include/esp32s31/rom/cache.h:31,45,160-166`
/// (64-byte lines, data map 0x10, instruction map 0x03);
/// `components/esp_rom/patches/esp_rom_cache_writeback_esp32p4_esp32s31.c:39-50`;
/// `components/soc/esp32s31/register/soc/cache_reg.h:1633-1660`
/// (`SYNC_MAP`; `SYNC_SIZE` 28 bits).
pub const CACHE_GEOMETRY: CacheGeometry<CacheMap> = CacheGeometry {
    line_bytes: oer_esp32s31_pac::cache::LINE_BYTES,
    max_range_bytes: oer_esp32s31_pac::cache::sync_size::SYNC_SIZE_MAX
        & !(oer_esp32s31_pac::cache::LINE_BYTES - 1),
    data: CacheMap::Data,
    writeback_passes: 2,
    instruction: Some(CacheMap::BothInstructionCaches),
};

/// One sync command over a range of at most the stage-two image. ESP-IDF
/// 4d59230d `components/esp_rom/patches/esp_rom_cache_writeback_esp32p4_esp32s31.c:39-50`
/// polls `SYNC_DONE` without a limit.
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
/// ROM's `Wait_SPI_Idle` and `SPI_read_data` poll `CMD.USR` without a
/// time limit (`oer_espressif_boot::flash`); this bound is this crate's.
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
    qio: read_command(
        oer_esp32s31_pac::spi1_mem_c::user2::UsrCommandValue::QioRead.bits() as u8,
        24 + 8,
        4,
    ),
    dio: read_command(
        oer_esp32s31_pac::spi1_mem_c::user2::UsrCommandValue::DioRead.bits() as u8,
        24 + 4,
        2,
    ),
    qout: read_command(
        oer_esp32s31_pac::spi1_mem_c::user2::UsrCommandValue::QoutRead.bits() as u8,
        24,
        8,
    ),
    dout: read_command(
        oer_esp32s31_pac::spi1_mem_c::user2::UsrCommandValue::DoutRead.bits() as u8,
        24,
        8,
    ),
    fast_read: read_command(
        oer_esp32s31_pac::spi1_mem_c::user2::UsrCommandValue::FastRead.bits() as u8,
        24,
        8,
    ),
    slow_read: read_command(
        oer_esp32s31_pac::spi1_mem_c::user2::UsrCommandValue::SlowRead.bits() as u8,
        24,
        0,
    ),
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
/// SOURCE: ESP-IDF 4d59230d
/// `components/esp_mspi/mspi_timing_tuning/port/esp32p4/mspi_timing_tuning_configs.h:79`
/// (`MSPI_TIMING_FLASH_CONFIG_TABLE_CORE_CLK_120M_MODULE_CLK_120M_STR_MODE`).
/// This is the related P4 block's table selected for this sweep.
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
/// SOURCE: ESP-IDF 4d59230d
/// `components/esp_hw_support/port/esp32s31/Kconfig.ldo:21-33`
/// (`ESP_LDO_VOLTAGE_PSRAM_DOMAIN`, default 1800 mV).
pub const PSRAM_SUPPLY_MV: u32 = 1800;

/// The external LDO's reference and multiplier codes nearest `millivolts`:
/// `vout = vref × (4 + mul) / 4 × 20`, `vref` 10 + dref below 9, else
/// 20 + 2 × (dref − 9); the first closest pair in order.
///
/// SOURCE: ESP-IDF 4d59230d
/// `components/esp_hal_pmu/esp32s31/include/hal/ldo_ll.h:61-94`
/// (`ldo_ll_voltage_to_dref_mul`, without eFuse calibration).
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
/// SOURCE: ESP-IDF 4d59230d
/// `components/esp_psram/device/esp_psram_impl_ap_oct.c:33-38,55-58`
/// (250 MHz: read latency 6, write latency 3, dummy bits 34/16, register
/// dummy 16; chip-select setup/hold 4, hold delay 3) and
/// `components/esp_hal_mspi/esp32s31/include/hal/psram_ctrlr_ll.h:130-150`
/// (cache dummy fields hold the bit count minus one: 33 and 15).
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
    // SOURCE: ESP-IDF 4d59230d
    // `components/esp_hw_support/port/esp32s31/Kconfig.ldo:35-38`
    // (1000 µs) and `components/esp_hw_support/ldo/esp_ldo_regulator.c:92-119`
    // (current limit off before the voltage-stable delay).
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
