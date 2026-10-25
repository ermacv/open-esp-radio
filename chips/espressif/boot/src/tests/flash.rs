use core::cell::RefCell;
use std::{rc::Rc, vec, vec::Vec};

use oer_time::{Clock, Duration, Instant};
use oer_time_virtual::VirtualClock;

use super::Ticking;
use crate::{
    Access, BootFailure, Busy, FlashFailure, WaitFailure,
    cache::{FetchSuspendRegisters, SuspendWaits},
    flash::{
        FlashClockRegisters, FlashCommandRegisters, FlashGeometry, FlashReadCommand,
        FlashReadCommands, FlashReadMode, FlashTiming, FlashTimingRegisters, TUNING_PAGE_BYTES,
        TUNING_SCRATCH_WORDS, TuningOwners, TuningRegion, TuningWaits, XipWindow, page_count, read,
        select_window, tune,
    },
    wait::{BootBudget, BootDeadline, BootWait},
};

const fn command(opcode: u8, address_bits: u8, dummy_cycles: u8) -> FlashReadCommand {
    FlashReadCommand {
        opcode,
        address_bits,
        dummy_cycles,
    }
}

const COMMANDS: FlashReadCommands = FlashReadCommands {
    qio: command(0xeb, 32, 4),
    dio: command(0xbb, 28, 2),
    qout: command(0x6b, 24, 8),
    dout: command(0x3b, 24, 8),
    fast_read: command(0x0b, 24, 8),
    slow_read: command(0x03, 24, 0),
};

const CAPACITY: u32 = 1024 * 1024;

const GEOMETRY: FlashGeometry = FlashGeometry {
    capacity: CAPACITY,
    commands: COMMANDS,
};

const COMMAND: BootWait = BootWait::new("flash-command", Duration::from_micros(50));

const SUSPEND: SuspendWaits = SuspendWaits {
    autoload: BootWait::new("icache-autoload", Duration::from_micros(20)),
    preload: BootWait::new("icache-preload", Duration::from_micros(20)),
    sync: BootWait::new("cache-sync-idle", Duration::from_micros(20)),
    lock: BootWait::new("cache-lock-idle", Duration::from_micros(20)),
    freeze: BootWait::new("icache-freeze", Duration::from_micros(20)),
    thaw: BootWait::new("icache-thaw", Duration::from_micros(20)),
};

const WAITS: TuningWaits = TuningWaits {
    command: COMMAND,
    suspend: SUSPEND,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Op {
    Configure(FlashReadCommand),
    Transfer(u32, u32),
    Start,
    SelectClock,
    SelectInput,
    Apply(u8, bool),
    RestoreClock,
    RestoreTiming,
    Suspend,
    Resume,
}

/// The flash, its controllers, its clock and the cache's fetch, one model
/// the owners share: a direct read at a candidate timing outside
/// `direct_good`, or a cached read at one outside `cache_good`, returns
/// corrupted words.
struct Model {
    clock: Rc<VirtualClock>,
    memory: Vec<u32>,
    mode: FlashReadMode,
    status_mode_bits: bool,
    log: Vec<Op>,
    transfer: (u32, u32),
    takes: Option<Duration>,
    running_until: Option<Instant>,
    buffer: [u32; 16],
    tuned_clock: bool,
    port_timing: Option<u8>,
    cache_timing: Option<u8>,
    direct_good: u16,
    cache_good: u16,
    fetch_suspended: bool,
    frozen: bool,
    xip_reads: Vec<usize>,
    /// The suspensions begun so far.
    suspends: usize,
    /// Every suspension after the first never sees the cache sync idle.
    fail_resuspend: bool,
}

impl Model {
    /// Whether a command of the command word runs.
    fn running(&self) -> bool {
        let now = self.clock.now();
        self.running_until.is_some_and(|until| now < until)
    }
}

type Shared = Rc<RefCell<Model>>;

fn model(clock: &Rc<VirtualClock>) -> Shared {
    let memory = (0..CAPACITY / 4)
        .map(|index| index.wrapping_mul(0x9e37_79b9) ^ 0x5a5a_5a5a)
        .collect();
    Rc::new(RefCell::new(Model {
        clock: Rc::clone(clock),
        memory,
        mode: FlashReadMode::Qio,
        status_mode_bits: false,
        log: Vec::new(),
        transfer: (0, 0),
        takes: Some(Duration::from_micros(2)),
        running_until: None,
        buffer: [0; 16],
        tuned_clock: false,
        port_timing: None,
        cache_timing: None,
        direct_good: 0,
        cache_good: 0,
        fetch_suspended: false,
        frozen: false,
        xip_reads: Vec::new(),
        suspends: 0,
        fail_resuspend: false,
    }))
}

fn corrupt(word: u32, timing: Option<u8>, good: u16, tuned: bool) -> u32 {
    match timing {
        Some(index) if tuned && good & (1 << index) == 0 => !word,
        _ => word,
    }
}

struct Port(Shared);

impl FlashCommandRegisters for Port {
    fn read_mode(&self, _: Access<'_>) -> FlashReadMode {
        self.0.borrow().mode
    }
    fn mode_bits_from_status(&self, _: Access<'_>) -> bool {
        self.0.borrow().status_mode_bits
    }
    fn configure_read(&mut self, _: Access<'_>, command: FlashReadCommand) {
        let mut model = self.0.borrow_mut();
        assert!(
            !model.running(),
            "the phases change under a running command"
        );
        model.log.push(Op::Configure(command));
    }
    fn set_transfer(&mut self, _: Access<'_>, address: u32, bytes: u32) {
        let mut model = self.0.borrow_mut();
        assert!(
            !model.running(),
            "the transfer changes under a running command"
        );
        model.log.push(Op::Transfer(address, bytes));
        model.transfer = (address, bytes);
    }
    fn start(&mut self, _: Access<'_>) -> Result<(), Busy> {
        let mut model = self.0.borrow_mut();
        let now = model.clock.now();
        if model.running_until.is_some_and(|until| now < until) {
            return Err(Busy);
        }
        model.log.push(Op::Start);
        model.running_until = Some(model.takes.map_or(Instant::from_micros(u64::MAX), |takes| {
            now.saturating_add(takes)
        }));
        let (address, bytes) = model.transfer;
        let first = (address / 4) as usize;
        for index in 0..(bytes / 4) as usize {
            let word = model.memory[first + index];
            model.buffer[index] = corrupt(
                word,
                model.port_timing,
                model.direct_good,
                model.tuned_clock,
            );
        }
        Ok(())
    }
    fn busy(&self, _: Access<'_>) -> bool {
        self.0.borrow().running()
    }
    fn data(&self, _: Access<'_>) -> [u32; 16] {
        self.0.borrow().buffer
    }
}

impl FlashTimingRegisters for Port {
    type Timing = u8;
    type Saved = ();
    fn save_timing(&self, _: Access<'_>) {}
    fn select_tuning_input(&mut self, _: Access<'_>) {
        self.0.borrow_mut().log.push(Op::SelectInput);
    }
    fn apply_timing(&mut self, _: Access<'_>, timing: u8, cache: bool) {
        let mut model = self.0.borrow_mut();
        model.log.push(Op::Apply(timing, cache));
        model.port_timing = Some(timing);
        if cache {
            model.cache_timing = Some(timing);
        }
    }
    fn restore_timing(&mut self, _: Access<'_>, (): ()) {
        let mut model = self.0.borrow_mut();
        assert!(
            model.fetch_suspended,
            "the timing changes with fetch running"
        );
        model.log.push(Op::RestoreTiming);
        model.port_timing = None;
        model.cache_timing = None;
    }
}

struct FlashClock(Shared);

impl FlashClockRegisters for FlashClock {
    type Saved = ();
    fn save_clock(&self, _: Access<'_>) {}
    fn select_tuning_clock(&mut self, _: Access<'_>) {
        let mut model = self.0.borrow_mut();
        model.log.push(Op::SelectClock);
        model.tuned_clock = true;
    }
    fn restore_clock(&mut self, _: Access<'_>, (): ()) {
        let mut model = self.0.borrow_mut();
        assert!(
            model.fetch_suspended,
            "the clock changes with fetch running"
        );
        model.log.push(Op::RestoreClock);
        model.tuned_clock = false;
    }
}

/// The cache's fetch: every idle flag reads idle at once, except the
/// cache sync after the first suspension when `fail_resuspend` is set.
struct Fetch(Shared);

impl FetchSuspendRegisters for Fetch {
    fn shut_fetch(&mut self, _: Access<'_>, shut: bool) {
        let mut model = self.0.borrow_mut();
        model.log.push(if shut { Op::Suspend } else { Op::Resume });
        model.fetch_suspended = shut;
        if shut {
            model.suspends += 1;
        }
    }
    fn autoload_enabled(&self, _: Access<'_>) -> bool {
        false
    }
    fn set_autoload(&mut self, _: Access<'_>, _: bool) {}
    fn autoload_done(&self, _: Access<'_>) -> bool {
        true
    }
    fn preload_done(&self, _: Access<'_>) -> bool {
        true
    }
    fn sync_done(&self, _: Access<'_>) -> bool {
        let model = self.0.borrow();
        !(model.fail_resuspend && model.suspends > 1)
    }
    fn lock_done(&self, _: Access<'_>) -> bool {
        true
    }
    fn select_freeze_mode_zero(&mut self, _: Access<'_>) {}
    fn set_freeze(&mut self, _: Access<'_>, frozen: bool) {
        self.0.borrow_mut().frozen = frozen;
    }
    fn freeze_done(&self, _: Access<'_>) -> bool {
        self.0.borrow().frozen
    }
}

/// The tuning region's XIP mapping at physical `base`; a word is read
/// only with fetch running, and once.
struct Xip {
    model: Shared,
    base: u32,
}

impl XipWindow for Xip {
    fn word(&self, index: usize) -> u32 {
        let mut model = self.model.borrow_mut();
        assert!(
            !model.fetch_suspended,
            "XIP is read only with fetch running"
        );
        assert!(
            !model.xip_reads.contains(&index),
            "each XIP word is read once, cold"
        );
        model.xip_reads.push(index);
        let word = model.memory[(self.base / 4) as usize + index];
        corrupt(
            word,
            model.cache_timing,
            model.cache_good,
            model.tuned_clock,
        )
    }
}

fn budget() -> BootBudget {
    BootBudget::below(Duration::from_millis(500), Duration::from_secs(9)).expect("below")
}

fn ops(model: &Shared) -> Vec<Op> {
    model.borrow().log.clone()
}

fn read_words(model: &Shared, address: u32, words: usize) -> Result<Vec<u32>, BootFailure> {
    let clock = Rc::clone(&model.borrow().clock);
    let ticking = Ticking::new(&clock);
    let deadline = BootDeadline::start(&ticking, budget());
    let mut port = Port(Rc::clone(model));
    let mut destination = vec![0; words];
    read(
        &mut port,
        GEOMETRY,
        address,
        &mut destination,
        COMMAND,
        &deadline,
    )
    .map(|()| destination)
}

#[test]
fn a_read_sends_its_modes_command_once_then_one_user_command_per_sixty_four_bytes() {
    let clock = Rc::new(VirtualClock::new());
    let model = model(&clock);
    model.borrow_mut().mode = FlashReadMode::Dio;
    let words = read_words(&model, 0x100, 40).expect("reads");
    assert_eq!(words, model.borrow().memory[0x40..0x40 + 40]);
    assert_eq!(
        ops(&model),
        [
            Op::Configure(COMMANDS.dio),
            Op::Transfer(0x100, 64),
            Op::Start,
            Op::Transfer(0x140, 64),
            Op::Start,
            Op::Transfer(0x180, 32),
            Op::Start,
        ]
    );
}

#[test]
fn a_read_outside_the_flash_misaligned_or_with_status_mode_bits_writes_nothing() {
    let clock = Rc::new(VirtualClock::new());
    let model = model(&clock);
    assert_eq!(
        read_words(&model, 2, 1),
        Err(BootFailure::Flash(FlashFailure::Misaligned))
    );
    assert_eq!(
        read_words(&model, CAPACITY - 4, 2),
        Err(BootFailure::Flash(FlashFailure::OutsideFlash))
    );
    assert_eq!(
        read_words(&model, u32::MAX - 3, 2),
        Err(BootFailure::Flash(FlashFailure::OutsideFlash))
    );
    model.borrow_mut().status_mode_bits = true;
    assert_eq!(
        read_words(&model, 0, 1),
        Err(BootFailure::Flash(FlashFailure::StatusModeBits))
    );
    assert_eq!(ops(&model), []);
}

#[test]
fn a_read_waits_for_a_running_command_and_a_command_that_never_ends_is_a_named_timeout() {
    let clock = Rc::new(VirtualClock::new());
    let model = model(&clock);
    model.borrow_mut().running_until = Some(clock.now().saturating_add(Duration::from_micros(30)));
    assert_eq!(
        read_words(&model, 0, 2).expect("reads after the running command"),
        model.borrow().memory[..2]
    );
    model.borrow_mut().takes = None;
    assert_eq!(
        read_words(&model, 0, 2),
        Err(BootFailure::Wait(WaitFailure::TimedOut("flash-command")))
    );
    // A command that never ends before the read: nothing is configured.
    let before = ops(&model).len();
    assert_eq!(
        read_words(&model, 0, 2),
        Err(BootFailure::Wait(WaitFailure::TimedOut("flash-command")))
    );
    assert_eq!(ops(&model).len(), before, "no word of the read is written");
}

#[test]
fn the_tuning_region_holds_whole_pages_inside_the_flash_and_the_address_space() {
    let region = |physical_start, virtual_start, bytes| TuningRegion {
        physical_start,
        virtual_start,
        bytes,
    };
    let capacity = 16 * 1024 * 1024;
    assert_eq!(
        page_count(region(0x10000, 0x4201_0000, 16 * 4096), capacity, 12),
        Some(16)
    );
    assert_eq!(
        page_count(region(0, 0x4200_0000, 15 * 4096 - 1), capacity, 12),
        None
    );
    assert_eq!(
        page_count(region(0, 0x4200_0000, 15 * 4096 + 128), capacity, 12),
        Some(15)
    );
    assert_eq!(
        page_count(region(0, 0x4200_0000, u32::MAX), capacity, 12),
        Some(31)
    );
    assert_eq!(
        page_count(region(4096, 0x4200_0000, 15 * 4096), 15 * 4096, 12),
        None
    );
    assert_eq!(
        page_count(region(1, 0x4200_0000, 16 * 4096), capacity, 12),
        None
    );
    assert_eq!(
        page_count(region(0, 0x4200_0001, 16 * 4096), capacity, 12),
        None
    );
    assert_eq!(page_count(region(0, !4095, 16 * 4096), capacity, 12), None);
    assert_eq!(
        page_count(region(!4095, 0x4200_0000, 16 * 4096), u32::MAX, 12),
        None
    );
}

#[test]
fn the_window_is_the_middle_of_the_first_longest_run_of_three() {
    assert_eq!(select_window(0, 12), None);
    assert_eq!(select_window(0b110_1101_1011, 12), None);
    assert_eq!(select_window(0xf000, 12), None, "beyond the candidates");
    assert_eq!(select_window(0b111, 12), Some(1));
    assert_eq!(select_window(0b1111 << 8, 12), Some(10));
    assert_eq!(select_window(0b1111_0111, 12), Some(6));
    assert_eq!(select_window(0b111_0111, 12), Some(1));
}

const TIMINGS: [u8; 12] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];
const REGION_BASE: u32 = 0x2_0000;
const REGION: TuningRegion = TuningRegion {
    physical_start: REGION_BASE,
    virtual_start: 0x4202_0000,
    bytes: 16 * TUNING_PAGE_BYTES,
};

fn tune_on(model: &Shared, region: TuningRegion) -> Result<FlashTiming, BootFailure> {
    let clock = Rc::clone(&model.borrow().clock);
    let ticking = Ticking::new(&clock);
    let deadline = BootDeadline::start(&ticking, budget());
    let mut port = Port(Rc::clone(model));
    let mut flash_clock = FlashClock(Rc::clone(model));
    let mut fetch = Fetch(Rc::clone(model));
    let xip = Xip {
        model: Rc::clone(model),
        base: region.physical_start,
    };
    let mut scratch = vec![0; TUNING_SCRATCH_WORDS];
    tune(
        TuningOwners {
            flash: &mut port,
            clock: &mut flash_clock,
            cache: &mut fetch,
            xip: &xip,
        },
        GEOMETRY,
        region,
        &TIMINGS,
        &mut scratch,
        WAITS,
        &deadline,
    )
}

fn without_reads(ops: &[Op]) -> Vec<Op> {
    ops.iter()
        .copied()
        .filter(|op| !matches!(op, Op::Configure(_) | Op::Transfer(..) | Op::Start))
        .collect()
}

#[test]
fn the_sweep_applies_the_middle_of_the_candidates_that_pass_both_reads() {
    let clock = Rc::new(VirtualClock::new());
    let model = model(&clock);
    model.borrow_mut().direct_good = 0b1_1111_1000; // 3..=8
    model.borrow_mut().cache_good = 0b11_1111_0000; // 4..=9
    assert_eq!(
        tune_on(&model, REGION),
        Ok(FlashTiming {
            index: 6,
            direct_mask: 0b1_1111_1000,
            cache_mask: 0b1_1111_0000,
        })
    );
    let ops = without_reads(&ops(&model));
    let mut expected = vec![Op::Suspend, Op::SelectClock, Op::SelectInput];
    expected.extend(TIMINGS.iter().map(|&timing| Op::Apply(timing, false)));
    for candidate in 3..=8 {
        expected.extend([Op::Apply(candidate, true), Op::Resume, Op::Suspend]);
    }
    expected.extend([Op::Apply(6, true), Op::Resume]);
    assert_eq!(ops, expected);
    let model = model.borrow();
    assert!(!model.fetch_suspended);
    assert!(model.tuned_clock, "the tuned clock stays");
    // Each candidate that passed the direct read read its own page (the
    // one that fails through the cache stops at its first word); the four
    // pages no candidate read were verified at the chosen timing.
    assert_eq!(model.xip_reads.len(), (5 + 4) * 1024 + 1);
}

#[test]
fn no_stable_window_restores_the_clock_and_the_timing_and_resumes_fetch() {
    let clock = Rc::new(VirtualClock::new());
    let model = model(&clock);
    model.borrow_mut().direct_good = 0b11;
    model.borrow_mut().cache_good = 0b11;
    assert_eq!(
        tune_on(&model, REGION),
        Err(BootFailure::Flash(FlashFailure::NoStableWindow))
    );
    let ops = without_reads(&ops(&model));
    assert_eq!(
        &ops[ops.len() - 3..],
        [Op::RestoreClock, Op::RestoreTiming, Op::Resume]
    );
    let model = model.borrow();
    assert!(!model.fetch_suspended);
    assert!(!model.tuned_clock);
    assert_eq!(model.cache_timing, None);
}

#[test]
fn a_page_that_reads_back_otherwise_at_the_chosen_timing_rolls_back() {
    let clock = Rc::new(VirtualClock::new());
    let model = model(&clock);
    model.borrow_mut().direct_good = 0b1111_1111_1111;
    model.borrow_mut().cache_good = 0b1111_1111_1111;
    // The first verification page's flash changes after the reference
    // read: it no longer matches through the cache.
    let clock_for_hook = Rc::clone(&clock);
    let region = REGION;
    let result = {
        let ticking = Ticking::new(&clock_for_hook);
        let deadline = BootDeadline::start(&ticking, budget());
        let mut port = Port(Rc::clone(&model));
        let mut flash_clock = FlashClock(Rc::clone(&model));
        let mut fetch = Fetch(Rc::clone(&model));
        let xip = Changed(Xip {
            model: Rc::clone(&model),
            base: region.physical_start,
        });
        let mut scratch = vec![0; TUNING_SCRATCH_WORDS];
        tune(
            TuningOwners {
                flash: &mut port,
                clock: &mut flash_clock,
                cache: &mut fetch,
                xip: &xip,
            },
            GEOMETRY,
            region,
            &TIMINGS,
            &mut scratch,
            WAITS,
            &deadline,
        )
    };
    assert_eq!(result, Err(BootFailure::Flash(FlashFailure::CacheMismatch)));
    let ops = without_reads(&ops(&model));
    assert_eq!(
        &ops[ops.len() - 4..],
        [Op::Suspend, Op::RestoreClock, Op::RestoreTiming, Op::Resume]
    );
    assert!(!model.borrow().tuned_clock);
}

/// An XIP window whose first page reads one word otherwise.
struct Changed(Xip);

impl XipWindow for Changed {
    fn word(&self, index: usize) -> u32 {
        let word = self.0.word(index);
        if index == 0 { !word } else { word }
    }
}

#[test]
fn a_region_without_a_page_per_candidate_and_three_more_is_refused_unswept() {
    let clock = Rc::new(VirtualClock::new());
    let model = model(&clock);
    let small = TuningRegion {
        bytes: 14 * TUNING_PAGE_BYTES,
        ..REGION
    };
    assert_eq!(
        tune_on(&model, small),
        Err(BootFailure::Flash(FlashFailure::TuningRegion))
    );
    assert_eq!(ops(&model), []);
}

#[test]
fn a_rollback_that_cannot_suspend_fetch_again_leaves_the_clock_and_the_timing_and_fails() {
    let clock = Rc::new(VirtualClock::new());
    let model = model(&clock);
    model.borrow_mut().direct_good = 0xfff;
    model.borrow_mut().cache_good = 0xfff;
    // The suspension after the first cached read times out, and so does
    // the rollback's.
    model.borrow_mut().fail_resuspend = true;
    assert_eq!(
        tune_on(&model, REGION),
        Err(BootFailure::Flash(FlashFailure::RollbackUnsuspended))
    );
    let ops = without_reads(&ops(&model));
    assert!(
        !ops.iter()
            .any(|op| matches!(op, Op::RestoreClock | Op::RestoreTiming)),
        "{ops:?}"
    );
    let model = model.borrow();
    assert!(model.tuned_clock, "the clock stays at the sweep's");
    assert!(model.suspends >= 3, "the rollback tried to suspend fetch");
}
