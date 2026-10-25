use core::cell::{Cell, RefCell};
use std::vec::Vec;

use oer_time::{Clock, Duration, Instant};
use oer_time_virtual::VirtualClock;

use super::Ticking;
use crate::{
    Access, BootFailure, Busy, CacheFailure, WaitFailure,
    cache::{
        CacheGeometry, CacheSyncRegisters, FetchSuspendRegisters, LineRange, SuspendWaits,
        SyncCommand, publish_code, with_fetch_suspended,
    },
    wait::{BootBudget, BootDeadline, BootWait},
};

const SYNC: BootWait = BootWait::new("cache-sync", Duration::from_micros(100));

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Map {
    Data,
    Instruction,
}

const SPLIT: CacheGeometry<Map> = CacheGeometry {
    line_bytes: 64,
    max_range_bytes: 1 << 27,
    data: Map::Data,
    writeback_passes: 2,
    instruction: Some(Map::Instruction),
};

const SHARED: CacheGeometry<Map> = CacheGeometry {
    line_bytes: 32,
    max_range_bytes: 1 << 27,
    data: Map::Data,
    writeback_passes: 2,
    instruction: None,
};

fn deadline<'a>(clock: &'a Ticking<'a>) -> BootDeadline<'a, Ticking<'a>> {
    let budget = BootBudget::below(Duration::from_millis(10), Duration::from_secs(9))
        .expect("below the watchdog");
    BootDeadline::start(clock, budget)
}

#[test]
fn a_range_widens_outward_to_whole_lines() {
    let range = LineRange::cover(0x1001, 2, 64, 1 << 27).expect("covered");
    assert_eq!((range.start(), range.size()), (0x1000, 64));
    let range = LineRange::cover(0x103f, 2, 64, 1 << 27).expect("covered");
    assert_eq!((range.start(), range.size()), (0x1000, 128));
}

#[test]
fn an_empty_overflowing_or_oversized_range_is_refused() {
    assert_eq!(
        LineRange::cover(0x1000, 0, 64, 1 << 27),
        Err(CacheFailure::EmptyRange)
    );
    assert_eq!(
        LineRange::cover(0xffff_ffc8, 0x20, 64, 1 << 27),
        Err(CacheFailure::RangeOverflow)
    );
    assert_eq!(
        LineRange::cover(0, 0x1000, 64, 0x800),
        Err(CacheFailure::RangeTooLarge)
    );
    assert_eq!(
        LineRange::cover(0, 4, 48, 1 << 27),
        Err(CacheFailure::LineSize)
    );
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SyncOp {
    Range(Map, u32, u32),
    Start(SyncCommand),
    DoneSeen(SyncCommand),
}

struct Engine<'a> {
    clock: &'a VirtualClock,
    log: RefCell<Vec<SyncOp>>,
    takes: Option<Duration>,
    running: Option<(SyncCommand, Instant)>,
}

impl<'a> Engine<'a> {
    fn new(clock: &'a VirtualClock) -> Self {
        Self {
            clock,
            log: RefCell::new(Vec::new()),
            takes: Some(Duration::from_micros(3)),
            running: None,
        }
    }
}

impl CacheSyncRegisters for Engine<'_> {
    type Map = Map;
    fn set_range(&mut self, _: Access<'_>, map: Map, range: LineRange) {
        assert!(
            self.running.is_none(),
            "the range changes only between commands"
        );
        self.log
            .borrow_mut()
            .push(SyncOp::Range(map, range.start(), range.size()));
    }
    fn start(&mut self, _: Access<'_>, command: SyncCommand) -> Result<(), Busy> {
        // A command that has not finished refuses the next: commands start
        // alone.
        if let Some((_, at)) = self.running
            && self.clock.now() < at
        {
            return Err(Busy);
        }
        self.log.borrow_mut().push(SyncOp::Start(command));
        self.running = self
            .takes
            .map(|takes| (command, self.clock.now().saturating_add(takes)));
        Ok(())
    }
    fn done(&self, _: Access<'_>) -> bool {
        match self.running {
            Some((command, at)) if self.clock.now() >= at => {
                self.log.borrow_mut().push(SyncOp::DoneSeen(command));
                true
            }
            _ => false,
        }
    }
}

/// Retire a finished command, as the hardware clears its bit.
impl Engine<'_> {
    fn retire(&mut self) {
        if let Some((_, at)) = self.running
            && self.clock.now() >= at
        {
            self.running = None;
        }
    }
}

/// A sync engine whose finished command retires before the next starts.
struct Retiring<'a>(Engine<'a>);

impl CacheSyncRegisters for Retiring<'_> {
    type Map = Map;
    fn set_range(&mut self, access: Access<'_>, map: Map, range: LineRange) {
        self.0.retire();
        self.0.set_range(access, map, range);
    }
    fn start(&mut self, access: Access<'_>, command: SyncCommand) -> Result<(), Busy> {
        self.0.retire();
        self.0.start(access, command)
    }
    fn done(&self, access: Access<'_>) -> bool {
        self.0.done(access)
    }
}

#[test]
fn code_is_written_back_twice_then_invalidated_from_the_instruction_caches() {
    let clock = VirtualClock::new();
    let ticking = Ticking::new(&clock);
    let mut engine = Retiring(Engine::new(&clock));
    let published = publish_code(
        &mut engine,
        SPLIT,
        0x5000_0123,
        0x100,
        SYNC,
        &deadline(&ticking),
    )
    .expect("published");
    assert_eq!(published.range().start(), 0x5000_0100);
    assert_eq!(published.range().size(), 0x140);
    use SyncCommand::{Invalidate, Writeback};
    assert_eq!(
        engine.0.log.take(),
        [
            SyncOp::Range(Map::Data, 0x5000_0100, 0x140),
            SyncOp::Start(Writeback),
            SyncOp::DoneSeen(Writeback),
            SyncOp::Start(Writeback),
            SyncOp::DoneSeen(Writeback),
            SyncOp::Range(Map::Instruction, 0x5000_0100, 0x140),
            SyncOp::Start(Invalidate),
            SyncOp::DoneSeen(Invalidate),
        ]
    );
}

#[test]
fn a_shared_cache_is_only_written_back() {
    let clock = VirtualClock::new();
    let ticking = Ticking::new(&clock);
    let mut engine = Retiring(Engine::new(&clock));
    let _published = publish_code(
        &mut engine,
        SHARED,
        0x4200_0000,
        0x40,
        SYNC,
        &deadline(&ticking),
    )
    .expect("published");
    assert!(
        !engine
            .0
            .log
            .take()
            .contains(&SyncOp::Start(SyncCommand::Invalidate))
    );
}

#[test]
fn a_command_that_never_finishes_is_a_named_timeout_and_nothing_follows() {
    let clock = VirtualClock::new();
    let ticking = Ticking::new(&clock);
    let mut engine = Retiring(Engine::new(&clock));
    engine.0.takes = None;
    assert_eq!(
        publish_code(
            &mut engine,
            SPLIT,
            0x5000_0000,
            0x40,
            SYNC,
            &deadline(&ticking)
        ),
        Err(BootFailure::Wait(WaitFailure::TimedOut("cache-sync")))
    );
    assert_eq!(
        engine.0.log.take(),
        [
            SyncOp::Range(Map::Data, 0x5000_0000, 0x40),
            SyncOp::Start(SyncCommand::Writeback),
        ]
    );
}

#[test]
fn a_refused_range_starts_no_command() {
    let clock = VirtualClock::new();
    let ticking = Ticking::new(&clock);
    let mut engine = Retiring(Engine::new(&clock));
    assert_eq!(
        publish_code(
            &mut engine,
            SPLIT,
            0x5000_0000,
            0,
            SYNC,
            &deadline(&ticking)
        ),
        Err(BootFailure::Cache(CacheFailure::EmptyRange))
    );
    assert!(engine.0.log.take().is_empty());
}

const SUSPEND: SuspendWaits = SuspendWaits {
    autoload: BootWait::new("icache-autoload", Duration::from_micros(50)),
    preload: BootWait::new("icache-preload", Duration::from_micros(50)),
    sync: BootWait::new("cache-sync-idle", Duration::from_micros(50)),
    lock: BootWait::new("cache-lock-idle", Duration::from_micros(50)),
    freeze: BootWait::new("icache-freeze", Duration::from_micros(50)),
    thaw: BootWait::new("icache-thaw", Duration::from_micros(50)),
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FetchOp {
    Shut(bool),
    Autoload(bool),
    Idle(&'static str),
    FreezeModeZero,
    Freeze(bool),
    FreezeSeen(bool),
}

struct Fetch {
    log: RefCell<Vec<FetchOp>>,
    autoload: bool,
    frozen: bool,
    freeze_responds: bool,
}

impl Fetch {
    fn new(autoload: bool) -> Self {
        Self {
            log: RefCell::new(Vec::new()),
            autoload,
            frozen: false,
            freeze_responds: true,
        }
    }

    fn record(&self, op: FetchOp) {
        self.log.borrow_mut().push(op);
    }

    fn idle(&self, name: &'static str) -> bool {
        self.record(FetchOp::Idle(name));
        true
    }
}

impl FetchSuspendRegisters for Fetch {
    fn shut_fetch(&mut self, _: Access<'_>, shut: bool) {
        self.record(FetchOp::Shut(shut));
    }
    fn autoload_enabled(&self, _: Access<'_>) -> bool {
        self.autoload
    }
    fn set_autoload(&mut self, _: Access<'_>, enabled: bool) {
        self.record(FetchOp::Autoload(enabled));
        self.autoload = enabled;
    }
    fn autoload_done(&self, _: Access<'_>) -> bool {
        self.idle("autoload")
    }
    fn preload_done(&self, _: Access<'_>) -> bool {
        self.idle("preload")
    }
    fn sync_done(&self, _: Access<'_>) -> bool {
        self.idle("sync")
    }
    fn lock_done(&self, _: Access<'_>) -> bool {
        self.idle("lock")
    }
    fn select_freeze_mode_zero(&mut self, _: Access<'_>) {
        self.record(FetchOp::FreezeModeZero);
    }
    fn set_freeze(&mut self, _: Access<'_>, frozen: bool) {
        self.record(FetchOp::Freeze(frozen));
        if self.freeze_responds {
            self.frozen = frozen;
        }
    }
    fn freeze_done(&self, _: Access<'_>) -> bool {
        self.record(FetchOp::FreezeSeen(self.frozen));
        self.frozen
    }
}

#[test]
fn fetch_is_suspended_in_the_rom_s_order_and_resumed_after_the_operation() {
    let clock = VirtualClock::new();
    let ticking = Ticking::new(&clock);
    let mut fetch = Fetch::new(true);
    let ran = Cell::new(false);
    let value = with_fetch_suspended(&mut fetch, SUSPEND, &deadline(&ticking), || {
        ran.set(true);
        5
    });
    assert_eq!(value, Ok(5));
    assert!(ran.get());
    assert_eq!(
        fetch.log.take(),
        [
            FetchOp::Shut(true),
            FetchOp::Autoload(false),
            FetchOp::Idle("autoload"),
            FetchOp::Idle("preload"),
            FetchOp::Idle("sync"),
            FetchOp::Idle("lock"),
            FetchOp::FreezeModeZero,
            FetchOp::Freeze(true),
            FetchOp::FreezeSeen(true),
            FetchOp::Freeze(false),
            FetchOp::FreezeSeen(false),
            FetchOp::Shut(false),
            FetchOp::Autoload(true),
        ]
    );
}

#[test]
fn autoload_that_did_not_run_is_not_started() {
    let clock = VirtualClock::new();
    let ticking = Ticking::new(&clock);
    let mut fetch = Fetch::new(false);
    with_fetch_suspended(&mut fetch, SUSPEND, &deadline(&ticking), || ()).expect("suspended");
    let ops = fetch.log.take();
    assert!(!ops.contains(&FetchOp::Autoload(false)));
    assert!(!ops.contains(&FetchOp::Autoload(true)));
    assert_eq!(ops.last(), Some(&FetchOp::Shut(false)));
}

#[test]
fn a_freeze_that_never_takes_effect_is_undone_and_the_operation_never_runs() {
    let clock = VirtualClock::new();
    let ticking = Ticking::new(&clock);
    let mut fetch = Fetch::new(true);
    fetch.freeze_responds = false;
    let ran = Cell::new(false);
    let result = with_fetch_suspended(&mut fetch, SUSPEND, &deadline(&ticking), || ran.set(true));
    assert_eq!(
        result,
        Err(BootFailure::Wait(WaitFailure::TimedOut("icache-freeze")))
    );
    assert!(!ran.get());
    let ops = fetch.log.take();
    assert_eq!(
        &ops[ops.len() - 3..],
        &[
            FetchOp::Freeze(false),
            FetchOp::Shut(false),
            FetchOp::Autoload(true)
        ]
    );
}
