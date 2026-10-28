use core::cell::RefCell;
use std::{rc::Rc, vec, vec::Vec};

use oer_time::{Clock, Duration, Instant};
use oer_time_virtual::VirtualClock;

use super::Ticking;
use crate::{
    Access, BootFailure, Busy, PsramFailure, WaitFailure,
    cache::{CacheGeometry, CacheSyncRegisters, LineRange, SyncCommand},
    clock::ActivePllPower,
    mmu::{MappedRange, MmuEntry, MmuRegisters, MmuTarget, MmuWindow},
    psram::{
        CacheAccess, ChipSelectTiming, MemoryClockRegisters, OctalPsramProfile, PsramCommand,
        PsramCommandRegisters, PsramControllerRegisters, PsramOwners, PsramReady, PsramWaits,
        bring_up, density_bytes,
    },
    wait::{BootBudget, BootDeadline, BootWait},
};

const MIB: u32 = 1024 * 1024;

const WINDOW: MmuWindow = MmuWindow {
    virtual_base: 0x5000_0000,
    page_bytes: 0x1_0000,
    first_entry: 0,
    entries: 1024,
    physical_pages: 1024,
};

const CACHE: CacheGeometry<()> = CacheGeometry {
    line_bytes: 64,
    max_range_bytes: (1 << 28) - 64,
    data: (),
    writeback_passes: 2,
    instruction: Some(()),
};

const WAITS: PsramWaits = PsramWaits {
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

const PROFILE: OctalPsramProfile<u8> = OctalPsramProfile {
    supply: 7,
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
    size: 16 * MIB,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Op {
    ControllerClock,
    Reset(bool),
    SelectPll,
    LimitCurrent(bool),
    SupplyLevel(u8),
    Detector,
    PowerSupply { at: Instant },
    References { at: Instant },
    PowerPll,
    KeepCpuPll,
    KeepMemoryPll,
    FunctionClock,
    Divider(u8),
    Dll,
    Pads,
    ChipSelect(ChipSelectTiming),
    OctalMode,
    Configure(PsramCommand),
    Output(u32),
    SelectPsram,
    Start,
    CacheAccess(CacheAccess),
    MmuWrite(u16, MmuEntry),
    SyncRange(u32, u32),
    SyncStart(SyncCommand),
}

/// The PSRAM, its controllers and their owners as one model; the mode
/// registers are bytes the commands read and write.
struct Model {
    clock: Rc<VirtualClock>,
    log: Vec<Op>,
    registers: [u8; 16],
    takes: Option<Duration>,
    running_until: Option<Instant>,
    configured: Option<PsramCommand>,
    output: u32,
    input: u32,
    entries: Vec<Option<MmuEntry>>,
    sync_until: Option<Instant>,
}

type Shared = Rc<RefCell<Model>>;

fn model(clock: &Rc<VirtualClock>, density: u8) -> Shared {
    let mut registers = [0xa5_u8; 16];
    registers[2] = 0b1100_0000 | density;
    Rc::new(RefCell::new(Model {
        clock: Rc::clone(clock),
        log: Vec::new(),
        registers,
        takes: Some(Duration::from_micros(2)),
        running_until: None,
        configured: None,
        output: 0,
        input: 0,
        entries: vec![None; 1024],
        sync_until: None,
    }))
}

struct Owner(Shared);

impl Owner {
    fn record(&self, op: Op) {
        self.0.borrow_mut().log.push(op);
    }
    fn now(&self) -> Instant {
        self.0.borrow().clock.now()
    }
}

impl MemoryClockRegisters for Owner {
    type SupplyLevel = u8;
    fn open_controller_clock(&mut self, _: Access<'_>) {
        self.record(Op::ControllerClock);
    }
    fn reset_controller(&mut self, _: Access<'_>, asserted: bool) {
        self.record(Op::Reset(asserted));
    }
    fn select_memory_pll(&mut self, _: Access<'_>) {
        self.record(Op::SelectPll);
    }
    fn limit_supply_current(&mut self, _: Access<'_>, limited: bool) {
        self.record(Op::LimitCurrent(limited));
    }
    fn set_supply_level(&mut self, _: Access<'_>, level: u8) {
        self.record(Op::SupplyLevel(level));
    }
    fn enable_supply_detector(&mut self, _: Access<'_>) {
        self.record(Op::Detector);
    }
    fn power_supply(&mut self, _: Access<'_>) {
        let at = self.now();
        self.record(Op::PowerSupply { at });
    }
    fn set_memory_pll_references(&mut self, _: Access<'_>) {
        let at = self.now();
        self.record(Op::References { at });
    }
    fn power_memory_pll(&mut self, _: Access<'_>) {
        self.record(Op::PowerPll);
    }
    fn open_function_clock(&mut self, _: Access<'_>) {
        self.record(Op::FunctionClock);
    }
}

impl ActivePllPower for Owner {
    fn keep_cpu_pll(&mut self, _: Access<'_>) {
        self.record(Op::KeepCpuPll);
    }
    fn keep_memory_pll(&mut self, _: Access<'_>) {
        self.record(Op::KeepMemoryPll);
    }
}

impl PsramControllerRegisters for Owner {
    fn set_bus_divider(&mut self, _: Access<'_>, divider: u8) {
        self.record(Op::Divider(divider));
    }
    fn enable_dll(&mut self, _: Access<'_>) {
        self.record(Op::Dll);
    }
    fn configure_pads(&mut self, _: Access<'_>) {
        self.record(Op::Pads);
    }
    fn set_chip_select_timing(&mut self, _: Access<'_>, timing: ChipSelectTiming) {
        self.record(Op::ChipSelect(timing));
    }
    fn configure_cache_access(&mut self, _: Access<'_>, cache: CacheAccess) {
        self.record(Op::CacheAccess(cache));
    }
}

impl PsramCommandRegisters for Owner {
    fn set_octal_ddr_mode(&mut self, _: Access<'_>) {
        self.record(Op::OctalMode);
    }
    fn configure(&mut self, _: Access<'_>, command: PsramCommand) {
        assert!(!self.busy(Access::new()), "phases change only while idle");
        self.record(Op::Configure(command));
        self.0.borrow_mut().configured = Some(command);
    }
    fn set_output(&mut self, _: Access<'_>, word: u32) {
        self.record(Op::Output(word));
        self.0.borrow_mut().output = word;
    }
    fn select_psram(&mut self, _: Access<'_>) {
        self.record(Op::SelectPsram);
    }
    fn start(&mut self, _: Access<'_>) -> Result<(), Busy> {
        if self.busy(Access::new()) {
            return Err(Busy);
        }
        self.record(Op::Start);
        let now = self.now();
        let mut model = self.0.borrow_mut();
        model.running_until = Some(model.takes.map_or(Instant::from_micros(u64::MAX), |takes| {
            now.saturating_add(takes)
        }));
        let command = model.configured.expect("configured before it starts");
        let at = command.address as usize;
        match command.command {
            0x4040 => {
                model.input = u32::from(u16::from_le_bytes([
                    model.registers[at],
                    model.registers[at + 1],
                ]));
            }
            0xc0c0 => {
                let [low, high, ..] = model.output.to_le_bytes();
                model.registers[at] = low;
                model.registers[at + 1] = high;
            }
            other => panic!("unknown command {other:#x}"),
        }
        Ok(())
    }
    fn busy(&self, _: Access<'_>) -> bool {
        let model = self.0.borrow();
        let now = model.clock.now();
        model.running_until.is_some_and(|until| now < until)
    }
    fn input(&self, _: Access<'_>) -> u32 {
        self.0.borrow().input
    }
}

impl MmuRegisters for Owner {
    fn read_entry(&mut self, _: Access<'_>, index: u16) -> Option<MmuEntry> {
        self.0.borrow().entries[usize::from(index)]
    }
    fn write_entry(&mut self, _: Access<'_>, index: u16, entry: MmuEntry) {
        self.record(Op::MmuWrite(index, entry));
        self.0.borrow_mut().entries[usize::from(index)] = Some(entry);
    }
}

impl CacheSyncRegisters for Owner {
    type Map = ();
    fn set_range(&mut self, _: Access<'_>, (): (), range: LineRange) {
        self.record(Op::SyncRange(range.start(), range.size()));
    }
    fn start(&mut self, _: Access<'_>, command: SyncCommand) -> Result<(), Busy> {
        self.record(Op::SyncStart(command));
        let at = self.now().saturating_add(Duration::from_micros(3));
        self.0.borrow_mut().sync_until = Some(at);
        Ok(())
    }
    fn done(&self, _: Access<'_>) -> bool {
        let model = self.0.borrow();
        model
            .sync_until
            .is_some_and(|until| model.clock.now() >= until)
    }
}

fn bring_up_on(model: &Shared) -> Result<PsramReady, BootFailure> {
    let clock = Rc::clone(&model.borrow().clock);
    let ticking = Ticking::new(&clock);
    let budget =
        BootBudget::below(Duration::from_millis(100), Duration::from_secs(9)).expect("below");
    let deadline = BootDeadline::start(&ticking, budget);
    let mut clock_owner = Owner(Rc::clone(model));
    let mut active = Owner(Rc::clone(model));
    let mut psram = Owner(Rc::clone(model));
    let mut cache = Owner(Rc::clone(model));
    bring_up(
        PsramOwners {
            clock: &mut clock_owner,
            active: &mut active,
            psram: &mut psram,
            cache: &mut cache,
        },
        PROFILE,
        WINDOW,
        CACHE,
        WAITS,
        &deadline,
    )
}

fn ops(model: &Shared) -> Vec<Op> {
    model.borrow().log.clone()
}

fn read_command(address: u32) -> PsramCommand {
    PsramCommand {
        command: 0x4040,
        command_bits: 16,
        address,
        address_bits: 32,
        output_bits: 0,
        input_bits: 16,
        dummy_bits: 16,
    }
}

fn write_command(address: u32) -> PsramCommand {
    PsramCommand {
        command: 0xc0c0,
        command_bits: 16,
        address,
        address_bits: 32,
        output_bits: 16,
        input_bits: 0,
        dummy_bits: 0,
    }
}

#[test]
fn a_cold_psram_is_powered_clocked_configured_and_mapped_in_order() {
    let clock = Rc::new(VirtualClock::new());
    let model = model(&clock, 0x5);
    let ready = bring_up_on(&model).expect("comes up");
    assert_eq!(
        ready.mapped(),
        MappedRange {
            virtual_start: 0x5000_0000,
            len: 16 * MIB,
        }
    );
    let ops = ops(&model);
    let Some(Op::PowerSupply { at: powered }) = ops.get(7).copied() else {
        panic!("the supply is powered eighth: {ops:?}");
    };
    let Some(Op::References { at: references }) = ops.get(9).copied() else {
        panic!("the references follow the settle: {ops:?}");
    };
    assert!(references.saturating_duration_since(powered) >= WAITS.supply_settle.bound());
    assert_eq!(
        ops[..7],
        [
            Op::ControllerClock,
            Op::Reset(true),
            Op::Reset(false),
            Op::SelectPll,
            Op::LimitCurrent(true),
            Op::SupplyLevel(7),
            Op::Detector,
        ]
    );
    assert_eq!(ops[8], Op::LimitCurrent(false));
    assert_eq!(
        ops[10..17],
        [
            Op::PowerPll,
            Op::KeepMemoryPll,
            Op::FunctionClock,
            Op::Divider(2),
            Op::Dll,
            Op::Pads,
            Op::ChipSelect(PROFILE.chip_select),
        ]
    );
    // Each command: octal mode, phases, (output), chip select, start.
    let commands: Vec<Op> = ops[17..]
        .iter()
        .copied()
        .filter(|op| matches!(op, Op::Configure(_) | Op::Output(_)))
        .collect();
    let registers = model.borrow().registers;
    assert_eq!(
        commands,
        [
            Op::Configure(read_command(0)),
            Op::Configure(write_command(0)),
            Op::Output(u32::from(u16::from_le_bytes([registers[0], 0xa5]))),
            Op::Configure(read_command(4)),
            Op::Configure(write_command(4)),
            Op::Output(u32::from(u16::from_le_bytes([registers[4], 0xa5]))),
            Op::Configure(read_command(8)),
            Op::Configure(write_command(8)),
            Op::Output(u32::from(u16::from_le_bytes([registers[8], 0xa5]))),
            Op::Configure(read_command(2)),
        ]
    );
    // MR0: read latency 6, fixed latency, drive strength 0; MR4: write
    // latency 3; MR8: burst length 3, row-boundary crossing, X16 and burst
    // type clear; every other bit and every neighbour as read.
    assert_eq!(registers[0], (0xa5 & 0b1100_0000) | (6 << 2) | (1 << 5));
    assert_eq!(registers[4], (0xa5 & 0b0001_1111) | (3 << 5));
    assert_eq!(registers[8], (0xa5 & 0b1011_0000) | 0b11 | (1 << 3));
    for neighbour in [1, 5, 9] {
        assert_eq!(registers[neighbour], 0xa5);
    }
    let tail: Vec<Op> = ops
        .iter()
        .copied()
        .skip_while(|op| !matches!(op, Op::CacheAccess(_)))
        .collect();
    assert_eq!(tail[0], Op::CacheAccess(PROFILE.cache));
    assert_eq!(tail.len(), 1 + 256 + 2);
    assert_eq!(
        tail[1],
        Op::MmuWrite(
            0,
            MmuEntry {
                target: MmuTarget::Psram,
                page: 0,
            }
        )
    );
    assert_eq!(
        tail[257..],
        [
            Op::SyncRange(0x5000_0000, 16 * MIB),
            Op::SyncStart(SyncCommand::Invalidate),
        ]
    );
}

#[test]
fn a_density_the_boot_does_not_know_maps_nothing() {
    let clock = Rc::new(VirtualClock::new());
    for code in [0, 2, 4] {
        let model = model(&clock, code);
        assert_eq!(
            bring_up_on(&model),
            Err(BootFailure::Psram(PsramFailure::UnsupportedDensity(code)))
        );
        assert!(
            !ops(&model)
                .iter()
                .any(|op| matches!(op, Op::CacheAccess(_) | Op::MmuWrite(..))),
        );
    }
}

#[test]
fn a_density_other_than_the_boards_maps_nothing() {
    let clock = Rc::new(VirtualClock::new());
    let model = model(&clock, 0x3);
    assert_eq!(
        bring_up_on(&model),
        Err(BootFailure::Psram(PsramFailure::DensityMismatch {
            found: 8 * MIB
        }))
    );
    assert!(!ops(&model).iter().any(|op| matches!(op, Op::MmuWrite(..))));
}

#[test]
fn a_mode_register_command_that_never_completes_stops_the_boot_unmapped() {
    let clock = Rc::new(VirtualClock::new());
    let model = model(&clock, 0x5);
    model.borrow_mut().takes = None;
    assert_eq!(
        bring_up_on(&model),
        Err(BootFailure::Wait(WaitFailure::TimedOut("psram-command")))
    );
    let ops = ops(&model);
    assert_eq!(
        ops.iter().filter(|op| matches!(op, Op::Start)).count(),
        1,
        "the first read's timeout ends the boot: {ops:?}"
    );
    assert!(!ops.iter().any(|op| matches!(op, Op::MmuWrite(..))));
}

#[test]
fn the_density_codes_name_the_ap_octal_sizes() {
    assert_eq!(density_bytes(0x1), Some(4 * MIB));
    assert_eq!(density_bytes(0x3), Some(8 * MIB));
    assert_eq!(density_bytes(0x5), Some(16 * MIB));
    assert_eq!(density_bytes(0x7), Some(32 * MIB));
    assert_eq!(density_bytes(0x6), Some(64 * MIB));
    for code in [0x0, 0x2, 0x4] {
        assert_eq!(density_bytes(code), None);
    }
}
