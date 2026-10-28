//! The boot owners over host memory from each instance's base: each test
//! fills the words with a pattern, runs one sequence of
//! `oer-espressif-boot` on an owner and reads the fields back through a
//! typed view of the same words. Host memory never clears a command the owner starts,
//! so a sequence that starts one ends with that command's named timeout,
//! after every word it set up.

use std::{vec, vec::Vec};

use oer_espressif_boot::{
    Access, BootFailure, FlashFailure, MmuFailure, WaitFailure,
    cache::{CacheGeometry, SuspendWaits, publish_code, with_fetch_suspended},
    clock::{ActivePllPower, ClockRates, ClockWaits, CpuClockProfile, bring_up},
    flash::{FlashGeometry, FlashReadCommand, FlashReadCommands, read},
    mmu::{self, MmuTarget, MmuWindow},
    psram::{self, CacheAccess, ChipSelectTiming, OctalPsramProfile, PsramOwners, PsramWaits},
    wait::{BootBudget, BootDeadline, BootWait},
};
use oer_mmio::Bits;
use oer_time::{Clock, Duration, Instant};
use oer_time_virtual::VirtualClock;

use super::{
    ActivePower, BusDividers, CacheMap, CacheRegisters, ClockRegisters, CpllDividers, CpuRoot,
    FlashRegisters, MemDivider, PsramRegisters, PsramSupplyLevel,
};
use oer_esp32s31_pac::{
    cache::CacheBoot as Cache, hp_alive_sys::HpAliveSysPllClock as HpAliveSys,
    hp_sys_clkrst::HpSysClkrstBootClock as HpSysClkrst, instances,
    iomux_mspi_pin::IomuxMspiPsramPins as IomuxMspiPin,
    lp_aonclkrst::LpAonClkrstCpll as LpAonClkrst, pmu::PmuBootClock as Pmu,
    spi_mem_c::Spi0Flash as Spi0, spi_mem_s::Memspi2Psram as Memspi2,
    spi1_mem_c::Spi1Flash as Spi1, spi1_mem_s::Memspi3Psram as Memspi3,
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

#[test]
fn xip_reads_return_the_exact_rejected_index_and_region_length() {
    use oer_espressif_boot::flash::XipWindow;

    let words = [0x1234_5678, 0x89ab_cdef];
    let region = super::XipRegion::new(&words);
    assert_eq!(region.word(0), Ok(words[0]));
    assert_eq!(region.word(1), Ok(words[1]));
    for index in [words.len(), usize::MAX] {
        assert_eq!(
            region.word(index),
            Err(FlashFailure::XipOutside {
                index,
                words: words.len(),
            })
        );
    }
    assert_eq!(
        super::XipRegion::new(&[]).word(0),
        Err(FlashFailure::XipOutside { index: 0, words: 0 })
    );
}

/// The PMU's active-state power: the radio PMU block's word, not this
/// PAC's soc blocks; the tests count its calls.
#[derive(Default)]
struct Active {
    cpu: usize,
    memory: usize,
}

impl ActivePllPower for Active {
    fn keep_cpu_pll(&mut self, _: Access<'_>) {
        self.cpu += 1;
    }
    fn keep_memory_pll(&mut self, _: Access<'_>) {
        self.memory += 1;
    }
}

/// The clock owner's four blocks over host memory.
struct ClockMemory {
    clkrst: Vec<u32>,
    aon: Vec<u32>,
    pmu: Vec<u32>,
    alive: Vec<u32>,
}

impl ClockMemory {
    fn new(fill: u32) -> Self {
        Self {
            clkrst: memory(instances::hp_sys_clkrst::SIZE, fill),
            aon: memory(instances::lp_aon_clkrst::SIZE, fill),
            pmu: memory(instances::pmu::SIZE, fill),
            alive: memory(instances::hp_alive_sys::SIZE, fill),
        }
    }

    fn owner(
        &mut self,
    ) -> ClockRegisters<
        oer_mmio::Host<'_>,
        oer_mmio::Host<'_>,
        oer_mmio::Host<'_>,
        oer_mmio::Host<'_>,
    > {
        ClockRegisters::host(
            &mut self.clkrst,
            &mut self.aon,
            &mut self.pmu,
            &mut self.alive,
        )
        .expect("host memory holds them")
    }
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
    calibration: const {
        match BootWait::new("cpll-calibration", Duration::from_micros(100)) {
            Ok(wait) => wait,
            Err(_) => panic!("a boot wait has a nonzero bound"),
        }
    },
    calibration_settle: const {
        match BootWait::new("cpll-settle", Duration::from_micros(10)) {
            Ok(wait) => wait,
            Err(_) => panic!("a boot wait has a nonzero bound"),
        }
    },
    update: const {
        match BootWait::new("clock-update", Duration::from_micros(100)) {
            Ok(wait) => wait,
            Err(_) => panic!("a boot wait has a nonzero bound"),
        }
    },
};

#[test]
fn a_cold_cpu_clock_is_staged_with_its_pll_powered_and_waits_for_the_running_update() {
    // All ones: the root reads PLL_F240M, the CPLL calibration's end reads
    // set, and an update reads running: the start is refused until its wait
    // ends, after every divider was staged.
    let mut words = ClockMemory::new(u32::MAX);
    let virtual_clock = VirtualClock::new();
    let ticking = Ticking(&virtual_clock);
    let deadline = BootDeadline::start(&ticking, budget());
    let mut active_words = memory(instances::pmu::SIZE, 0x5a5a_a5a5);
    let active_before = active_words.clone();
    let mut active = ActivePower::host(&mut active_words).expect("host memory holds it");
    let mut owner = words.owner();
    assert_eq!(
        bring_up(&mut owner, &mut active, PROFILE, WAITS, &deadline),
        Err(BootFailure::Wait(WaitFailure::TimedOut("clock-update")))
    );
    drop(owner);
    drop(active);
    assert_active_power(&mut active_words, &active_before, true);

    let view = HpSysClkrst::host(&mut words.clkrst).expect("host memory holds it");
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
    let lp = LpAonClkrst::host(&mut words.aon).expect("host memory holds it");
    let divider = lp.read_cpll_div();
    assert_eq!(divider.cpll_fb_div(), 8);
    assert_eq!(divider.cpll_ref_div().get(), 1);
    let alive = HpAliveSys::host(&mut words.alive).expect("host memory holds it");
    assert!(alive.read_hp_clk_ctrl().hp_cpll_300m_clk_en());
}

#[test]
fn a_cpll_that_never_calibrates_stops_the_boot_before_the_root_moves() {
    // All zeros: the root reads XTAL and the calibration never ends.
    let mut words = ClockMemory::new(0);
    let virtual_clock = VirtualClock::new();
    let ticking = Ticking(&virtual_clock);
    let deadline = BootDeadline::start(&ticking, budget());
    let mut active = Active::default();
    let mut owner = words.owner();
    assert_eq!(
        bring_up(&mut owner, &mut active, PROFILE, WAITS, &deadline),
        Err(BootFailure::Wait(WaitFailure::TimedOut("cpll-calibration")))
    );
    drop(owner);
    let view = HpSysClkrst::host(&mut words.clkrst).expect("host memory holds it");
    assert_eq!(view.read_soc_clk_sel().reg_soc_clk_sel().get(), 0, "XTAL");
    assert_eq!(view.read_cpu_freq_ctrl0().reg_cpu_clk_div_num(), 0);
    assert!(
        !view.read_ana_pll_ctrl0().reg_cpu_pll_cal_stop(),
        "left running"
    );
    assert!(
        !view.read_root_clk_ctrl0().reg_soc_clk_update(),
        "no update started"
    );
    let lp = LpAonClkrst::host(&mut words.aon).expect("host memory holds it");
    assert_eq!(lp.read_cpll_div().cpll_fb_div(), 8);
}

const GEOMETRY: CacheGeometry<CacheMap> = CacheGeometry {
    line_bytes: 64,
    max_range_bytes: (1 << 28) - 64,
    data: CacheMap::Data,
    writeback_passes: 2,
    instruction: Some(CacheMap::BothInstructionCaches),
};

#[test]
fn a_publication_starts_one_writeback_of_the_data_caches_whole_lines() {
    // All zeros: no command runs, and the engine never reports done.
    let mut words = memory(instances::cache::SIZE, 0);
    let virtual_clock = VirtualClock::new();
    let ticking = Ticking(&virtual_clock);
    let deadline = BootDeadline::start(&ticking, budget());
    let mut owner = CacheRegisters::host(&mut words).expect("host memory holds it");
    let sync = const {
        match BootWait::new("cache-sync", Duration::from_micros(50)) {
            Ok(wait) => wait,
            Err(_) => panic!("a boot wait has a nonzero bound"),
        }
    };
    assert_eq!(
        publish_code(&mut owner, GEOMETRY, 0x5000_0123, 0x100, sync, &deadline),
        Err(BootFailure::Wait(WaitFailure::TimedOut("cache-sync")))
    );
    drop(owner);
    let view = Cache::host(&mut words).expect("host memory holds it");
    assert_eq!(
        view.read_sync_map().sync_map().get(),
        0x10,
        "the data cache"
    );
    assert_eq!(view.read_sync_addr().sync_addr(), 0x5000_0100);
    assert_eq!(view.read_sync_size().sync_size().get(), 0x140);
    let ctrl = view.read_sync_ctrl();
    assert!(ctrl.writeback_ena(), "the writeback started");
    assert!(!ctrl.invalidate_ena() && !ctrl.clean_ena() && !ctrl.writeback_invalidate_ena());
}

const fn command(opcode: u8, address_bits: u8, dummy_cycles: u8) -> FlashReadCommand {
    FlashReadCommand {
        opcode,
        address_bits,
        dummy_cycles,
    }
}

const FLASH: FlashGeometry = FlashGeometry {
    capacity: 16 * 1024 * 1024,
    commands: FlashReadCommands {
        qio: command(0xeb, 32, 4),
        dio: command(0xbb, 28, 2),
        qout: command(0x6b, 24, 8),
        dout: command(0x3b, 24, 8),
        fast_read: command(0x0b, 24, 8),
        slow_read: command(0x03, 24, 0),
    },
};

/// SPI1 in QIO mode, idle, its status word sending no mode bits.
fn qio_spi1() -> Vec<u32> {
    let mut words = memory(instances::spi1::SIZE, 0);
    let mut view = Spi1::host(&mut words).expect("host memory holds it");
    view.modify_ctrl(|w| {
        w.set_fread_qio(true).set_fastrd_mode(true);
    });
    words
}

#[test]
fn a_flash_read_sets_up_the_read_modes_command_then_starts_the_user_command() {
    let mut spi0 = memory(instances::spi0::SIZE, 0);
    let mut spi1 = qio_spi1();
    let virtual_clock = VirtualClock::new();
    let ticking = Ticking(&virtual_clock);
    let deadline = BootDeadline::start(&ticking, budget());
    let mut owner = FlashRegisters::host(&mut spi0, &mut spi1).expect("host memory holds them");
    let wait = const {
        match BootWait::new("flash-command", Duration::from_micros(50)) {
            Ok(wait) => wait,
            Err(_) => panic!("a boot wait has a nonzero bound"),
        }
    };
    let mut destination = [0_u32; 4];
    assert_eq!(
        read(&mut owner, FLASH, 0x2000, &mut destination, wait, &deadline),
        Err(BootFailure::Wait(WaitFailure::TimedOut("flash-command")))
    );
    drop(owner);
    let view = Spi1::host(&mut spi1).expect("host memory holds it");
    let user = view.read_user();
    assert!(user.usr_command() && user.usr_addr() && user.usr_dummy());
    assert!(user.usr_miso() && !user.usr_mosi());
    let user1 = view.read_user1();
    assert_eq!(
        user1.usr_addr_bitlen().get(),
        31,
        "24 address and 8 mode bits"
    );
    assert_eq!(user1.usr_dummy_cyclelen().get(), 3, "4 dummy cycles");
    let user2 = view.read_user2();
    assert_eq!(user2.usr_command_bitlen().get(), 7);
    assert_eq!(user2.usr_command_value(), 0xeb);
    assert_eq!(view.read_addr().usr_addr_value(), 0x2000);
    assert_eq!(view.read_miso_dlen().usr_miso_dbitlen().get(), 127);
    assert!(view.read_cmd().usr(), "the user command started");
}

#[test]
fn a_flash_read_waits_for_every_command_of_the_word_before_writing_its_phases() {
    // Every command of SPI1's command word but the user one reads running.
    let mut spi0 = memory(instances::spi0::SIZE, 0);
    let mut spi1 = qio_spi1();
    spi1[oer_esp32s31_pac::spi1_mem_c::cmd::OFFSET / 4] =
        oer_esp32s31_pac::spi1_mem_c::cmd::BUSY & !oer_esp32s31_pac::spi1_mem_c::cmd::COMMANDS;
    let before = spi1.clone();
    let virtual_clock = VirtualClock::new();
    let ticking = Ticking(&virtual_clock);
    let deadline = BootDeadline::start(&ticking, budget());
    let mut owner = FlashRegisters::host(&mut spi0, &mut spi1).expect("host memory holds them");
    let wait = const {
        match BootWait::new("flash-command", Duration::from_micros(50)) {
            Ok(wait) => wait,
            Err(_) => panic!("a boot wait has a nonzero bound"),
        }
    };
    let mut destination = [0_u32; 1];
    assert_eq!(
        read(&mut owner, FLASH, 0, &mut destination, wait, &deadline),
        Err(BootFailure::Wait(WaitFailure::TimedOut("flash-command")))
    );
    drop(owner);
    assert_eq!(spi1, before, "no phase, address or length changed");
}

#[test]
fn a_flash_read_while_the_status_word_sends_mode_bits_is_refused_unwritten() {
    // All ones: the status word's mode-bit enable reads set.
    let mut spi0 = memory(instances::spi0::SIZE, 0);
    let mut spi1 = memory(instances::spi1::SIZE, u32::MAX);
    let mut owner = FlashRegisters::host(&mut spi0, &mut spi1).expect("host memory holds them");
    let virtual_clock = VirtualClock::new();
    let ticking = Ticking(&virtual_clock);
    let deadline = BootDeadline::start(&ticking, budget());
    let wait = const {
        match BootWait::new("flash-command", Duration::from_micros(50)) {
            Ok(wait) => wait,
            Err(_) => panic!("a boot wait has a nonzero bound"),
        }
    };
    let mut destination = [0_u32; 1];
    assert_eq!(
        read(&mut owner, FLASH, 0, &mut destination, wait, &deadline),
        Err(BootFailure::Flash(FlashFailure::StatusModeBits))
    );
    drop(owner);
    assert!(spi1.iter().all(|&word| word == u32::MAX), "nothing written");
}

const FLASH_WINDOW: MmuWindow = MmuWindow {
    virtual_base: 0x4000_0000,
    page_bytes: 0x1_0000,
    first_entry: 0,
    entries: 1024,
    physical_pages: 2048,
};

#[test]
fn a_flash_entry_maps_flash_only_and_an_invalid_one_is_never_page_zero() {
    let mut spi0 = memory(instances::spi0::SIZE, 0);
    let mut spi1 = memory(instances::spi1::SIZE, 0);
    let mut owner = FlashRegisters::host(&mut spi0, &mut spi1).expect("host memory holds them");
    assert_eq!(
        mmu::physical_address(&mut owner, FLASH_WINDOW, MmuTarget::Flash, 0x4000_1234),
        Err(BootFailure::Mmu(MmuFailure::Unmapped))
    );
    assert_eq!(
        mmu::map(
            &mut owner,
            FLASH_WINDOW,
            MmuTarget::Flash,
            0x4003_0000,
            0x7_0000,
            0x1_0000
        )
        .map(|mapped| mapped.len),
        Ok(0x1_0000)
    );
    assert_eq!(
        mmu::physical_address(&mut owner, FLASH_WINDOW, MmuTarget::Flash, 0x4003_0010),
        Ok(0x7_0010)
    );
    drop(owner);
    let view = Spi0::host(&mut spi0).expect("host memory holds it");
    let content = view.read_mmu_item_content();
    assert!(content.valid() && !content.sensitive());
    assert_eq!(content.paddr().get(), 7);
    assert_eq!(view.read_spi_mem_mmu_item_index().spi_mmu_item_index(), 3);

    let mut spi0 = memory(instances::spi0::SIZE, 0);
    let mut owner = FlashRegisters::host(&mut spi0, &mut spi1).expect("host memory holds them");
    assert_eq!(
        mmu::map(
            &mut owner,
            FLASH_WINDOW,
            MmuTarget::Psram,
            0x4000_0000,
            0,
            0x1_0000
        ),
        Err(BootFailure::Mmu(MmuFailure::Readback)),
        "a PSRAM target is written invalid"
    );
}

const PSRAM_WINDOW: MmuWindow = MmuWindow {
    virtual_base: 0x5000_0000,
    page_bytes: 0x1_0000,
    first_entry: 0,
    entries: 1024,
    physical_pages: 1024,
};

/// The PSRAM owner's three blocks over host memory.
struct PsramMemory {
    memspi2: Vec<u32>,
    memspi3: Vec<u32>,
    pins: Vec<u32>,
}

impl PsramMemory {
    fn new() -> Self {
        Self {
            memspi2: memory(instances::memspi2::SIZE, 0),
            memspi3: memory(instances::memspi3::SIZE, 0),
            pins: memory(instances::iomux_mspi_pin::SIZE, 0),
        }
    }

    fn owner(
        &mut self,
    ) -> PsramRegisters<oer_mmio::Host<'_>, oer_mmio::Host<'_>, oer_mmio::Host<'_>> {
        PsramRegisters::host(&mut self.memspi2, &mut self.memspi3, &mut self.pins)
            .expect("host memory holds them")
    }
}

#[test]
fn a_psram_entry_needs_its_access_flag_and_valid_flag() {
    let mut words = PsramMemory::new();
    {
        let mut view = Memspi2::host(&mut words.memspi2).expect("host memory holds it");
        view.modify_mmu_item_content(|w| {
            w.set_valid(true).set_paddr(Bits::constant::<5>());
        });
    }
    let mut owner = words.owner();
    assert_eq!(
        mmu::physical_address(&mut owner, PSRAM_WINDOW, MmuTarget::Psram, 0x5000_0000),
        Err(BootFailure::Mmu(MmuFailure::Unmapped)),
        "valid without the PSRAM access flag is no PSRAM entry"
    );
    assert_eq!(
        mmu::map(
            &mut owner,
            PSRAM_WINDOW,
            MmuTarget::Psram,
            0x5000_0000,
            0,
            0x1_0000
        )
        .map(|mapped| mapped.virtual_start),
        Ok(0x5000_0000)
    );
    drop(owner);
    let view = Memspi2::host(&mut words.memspi2).expect("host memory holds it");
    let content = view.read_mmu_item_content();
    assert!(content.valid() && content.access_psram() && !content.sensitive());
    assert_eq!(content.paddr().get(), 0);
}

const PSRAM_PROFILE: OctalPsramProfile<PsramSupplyLevel> = OctalPsramProfile {
    supply: PsramSupplyLevel {
        dref: Bits::constant::<6>(),
        mul: Bits::constant::<5>(),
    },
    divider: 2,
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
    size: 16 * 1024 * 1024,
};

const PSRAM_WAITS: PsramWaits = PsramWaits {
    supply_settle: const {
        match BootWait::new("psram-supply", Duration::from_micros(1000)) {
            Ok(wait) => wait,
            Err(_) => panic!("a boot wait has a nonzero bound"),
        }
    },
    command: const {
        match BootWait::new("psram-command", Duration::from_micros(50)) {
            Ok(wait) => wait,
            Err(_) => panic!("a boot wait has a nonzero bound"),
        }
    },
    cache_sync: const {
        match BootWait::new("cache-sync", Duration::from_micros(50)) {
            Ok(wait) => wait,
            Err(_) => panic!("a boot wait has a nonzero bound"),
        }
    },
};

#[test]
fn a_mode_register_read_selects_four_byte_addresses_even_at_address_zero() {
    let mut clock = ClockMemory::new(0);
    let mut psram = PsramMemory::new();
    let mut cache_words = memory(instances::cache::SIZE, 0);
    let mut active_words = memory(instances::pmu::SIZE, 0);
    let virtual_clock = VirtualClock::new();
    let ticking = Ticking(&virtual_clock);
    let deadline = BootDeadline::start(&ticking, budget());
    let mut clock_owner = clock.owner();
    let mut psram_owner = psram.owner();
    let mut cache_owner = CacheRegisters::host(&mut cache_words).expect("host memory holds it");
    let mut active = ActivePower::host(&mut active_words).expect("host memory holds it");
    assert_eq!(
        psram::bring_up(
            PsramOwners {
                clock: &mut clock_owner,
                active: &mut active,
                psram: &mut psram_owner,
                cache: &mut cache_owner,
            },
            PSRAM_PROFILE,
            PSRAM_WINDOW,
            GEOMETRY,
            PSRAM_WAITS,
            &deadline,
        ),
        Err(BootFailure::Wait(WaitFailure::TimedOut("psram-command")))
    );
    drop(psram_owner);
    let command = Memspi3::host(&mut psram.memspi3).expect("host memory holds it");
    assert!(command.read_cmd().usr(), "the first MR read was submitted");
    assert_eq!(command.read_addr().usr_addr_value(), 0);
    assert!(
        command.read_cache_fctrl().cache_usr_addr_4byte(),
        "a 32-bit address also needs the user-command four-byte selector"
    );
}

#[test]
fn a_cold_psram_is_powered_and_clocked_and_its_first_mode_register_read_never_ending_maps_nothing()
{
    let mut clock = ClockMemory::new(0);
    {
        let mut view = Pmu::host(&mut clock.pmu).expect("host memory holds it");
        view.modify_ext_ldo_ctrl(|w| {
            w.set_ext_ldo_tie_high(true);
        });
    }
    let mut psram = PsramMemory::new();
    let mut cache_words = memory(instances::cache::SIZE, 0);
    let virtual_clock = VirtualClock::new();
    let ticking = Ticking(&virtual_clock);
    let deadline = BootDeadline::start(&ticking, budget());
    let mut active_words = memory(instances::pmu::SIZE, 0x5a5a_a5a5);
    let active_before = active_words.clone();
    let mut active = ActivePower::host(&mut active_words).expect("host memory holds it");
    let mut clock_owner = clock.owner();
    let mut psram_owner = psram.owner();
    let mut cache_owner = CacheRegisters::host(&mut cache_words).expect("host memory holds it");
    assert_eq!(
        psram::bring_up(
            PsramOwners {
                clock: &mut clock_owner,
                active: &mut active,
                psram: &mut psram_owner,
                cache: &mut cache_owner,
            },
            PSRAM_PROFILE,
            PSRAM_WINDOW,
            GEOMETRY,
            PSRAM_WAITS,
            &deadline,
        ),
        Err(BootFailure::Wait(WaitFailure::TimedOut("psram-command")))
    );
    drop((clock_owner, psram_owner, cache_owner));
    drop(active);
    assert_active_power(&mut active_words, &active_before, false);

    let view = HpSysClkrst::host(&mut clock.clkrst).expect("host memory holds it");
    let ctrl = view.read_psram_ctrl0();
    assert!(
        ctrl.reg_psram_sys_clk_en() && ctrl.reg_psram_pll_clk_en() && ctrl.reg_psram_core_clk_en()
    );
    assert!(
        !ctrl.reg_psram_axi_rst_en() && !ctrl.reg_psram_apb_rst_en(),
        "reset released"
    );
    assert_eq!(ctrl.reg_psram_clk_src_sel().get(), 1, "MPLL");
    assert_eq!(view.read_ref_25m_ctrl0().reg_ref_25m_clk_div_num(), 19);
    assert_eq!(view.read_ref_50m_ctrl0().reg_ref_50m_clk_div_num(), 9);
    assert!(view.read_ref_500m_ctrl0().reg_ref_500m_sel());
    let pmu = Pmu::host(&mut clock.pmu).expect("host memory holds it");
    let ldo = pmu.read_ext_ldo_ctrl();
    assert!(!ldo.ext_cur_lim(), "the current limit ends");
    assert!(ldo.ext_ldo_en_vdet() && !ldo.ext_ldo_tie_high());
    assert_eq!((ldo.ext_ldo_dref().get(), ldo.ext_ldo_mul().get()), (6, 5));
    assert!(pmu.read_psram_cfg().psram_xpd());
    let alive = HpAliveSys::host(&mut clock.alive).expect("host memory holds it");
    assert!(alive.read_hp_clk_ctrl().hp_mpll_500m_clk_en());

    let spi2 = Memspi2::host(&mut psram.memspi2).expect("host memory holds it");
    let sram_clk = spi2.read_sram_clk();
    assert!(!sram_clk.sclk_equ_sysclk());
    assert_eq!(
        (
            sram_clk.sclkcnt_n(),
            sram_clk.sclkcnt_h(),
            sram_clk.sclkcnt_l()
        ),
        (1, 0, 1)
    );
    assert!(
        spi2.read_timing_cali().dll_timing_cali() && spi2.read_smem_timing_cali().dll_timing_cali()
    );
    let ac = spi2.read_smem_ac();
    assert!(ac.cs_setup() && ac.cs_hold() && ac.split_trans_en());
    assert_eq!(
        (
            ac.cs_setup_time().get(),
            ac.cs_hold_time().get(),
            ac.cs_hold_delay().get()
        ),
        (3, 3, 2)
    );
    assert!(
        !spi2.read_cache_fctrl().axi_req_en(),
        "the cache's access waits for the density"
    );
    assert!(!spi2.read_mmu_item_content().valid(), "nothing mapped");
    let spi3 = Memspi3::host(&mut psram.memspi3).expect("host memory holds it");
    let clock3 = spi3.read_clock();
    assert_eq!(
        (clock3.clkcnt_n(), clock3.clkcnt_h(), clock3.clkcnt_l()),
        (1, 0, 1)
    );
    let ctrl = spi3.read_ctrl();
    assert!(
        ctrl.fdout_oct()
            && ctrl.fdin_oct()
            && ctrl.faddr_oct()
            && ctrl.fcmd_oct()
            && ctrl.fastrd_mode()
    );
    assert!(spi3.read_ddr().en() && !spi3.read_ddr().cmd_dis());
    let user2 = spi3.read_user2();
    assert_eq!(
        (user2.usr_command_bitlen().get(), user2.usr_command_value()),
        (15, 0x4040)
    );
    let user1 = spi3.read_user1();
    assert_eq!(
        (
            user1.usr_addr_bitlen().get(),
            user1.usr_dummy_cyclelen().get()
        ),
        (31, 15)
    );
    assert_eq!(spi3.read_miso_dlen().usr_miso_dbitlen().get(), 15);
    let user = spi3.read_user();
    assert!(
        user.usr_command()
            && user.usr_addr()
            && user.usr_miso()
            && user.usr_dummy()
            && !user.usr_mosi()
    );
    let misc = spi3.read_misc();
    assert!(!misc.cs1_dis() && misc.cs0_dis(), "the PSRAM's chip select");
    assert!(spi3.read_cmd().usr(), "the mode register read started");
    let pins = IomuxMspiPin::host(&mut psram.pins).expect("host memory holds it");
    assert_eq!(pins.read_psram_d_pin0().reg_psram_d_drv().get(), 2);
    assert!(pins.read_psram_dqs_0_pin0().reg_psram_dqs_0_xpd());
}

const SUSPEND: SuspendWaits = SuspendWaits {
    autoload: const {
        match BootWait::new("icache-autoload", Duration::from_micros(20)) {
            Ok(wait) => wait,
            Err(_) => panic!("a boot wait has a nonzero bound"),
        }
    },
    preload: const {
        match BootWait::new("icache-preload", Duration::from_micros(20)) {
            Ok(wait) => wait,
            Err(_) => panic!("a boot wait has a nonzero bound"),
        }
    },
    sync: const {
        match BootWait::new("cache-sync-idle", Duration::from_micros(20)) {
            Ok(wait) => wait,
            Err(_) => panic!("a boot wait has a nonzero bound"),
        }
    },
    lock: const {
        match BootWait::new("cache-lock-idle", Duration::from_micros(20)) {
            Ok(wait) => wait,
            Err(_) => panic!("a boot wait has a nonzero bound"),
        }
    },
    freeze: const {
        match BootWait::new("icache-freeze", Duration::from_micros(20)) {
            Ok(wait) => wait,
            Err(_) => panic!("a boot wait has a nonzero bound"),
        }
    },
    thaw: const {
        match BootWait::new("icache-thaw", Duration::from_micros(20)) {
            Ok(wait) => wait,
            Err(_) => panic!("a boot wait has a nonzero bound"),
        }
    },
};

#[test]
fn a_cache_that_never_reports_idle_has_fetch_opened_again() {
    // All zeros: autoload is off and never reports done.
    let mut words = memory(instances::cache::SIZE, 0);
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
    let mut words = memory(instances::cache::SIZE, u32::MAX);
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

/// The boot sequence keeps its PLL powered without changing the radio's other fields.
fn assert_active_power(words: &mut [u32], before: &[u32], cpu: bool) {
    let mut expected = before.to_vec();
    let mut view = oer_esp32s31_pac::pmu::PmuRadio::host(&mut expected).expect("host PMU");
    view.modify_hp_active_hp_ck_power(|w| {
        if cpu {
            w.set_hp_active_xpd_cpll(true)
                .set_hp_active_xpd_cpll_i2c(true);
        } else {
            w.set_hp_active_xpd_mpll(true)
                .set_hp_active_xpd_mpll_i2c(true);
        }
    });
    assert_eq!(
        words, expected,
        "only this PLL's active-state power changes"
    );
}
