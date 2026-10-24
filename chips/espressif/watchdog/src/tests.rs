//! The sequences over a recording fake of the register traits: each test
//! checks which fields are written in which order, never a word's address.

use core::{cell::RefCell, num::NonZeroU32};
use std::vec::Vec;

use oer_time::Duration;
use oer_time_virtual::VirtualClock;

use super::{
    LOCK, MainConfig, MainStageAction, MainWatchdog, MainWatchdogRegisters, MicrosecondTick,
    Microseconds, ResetLength, RtcWatchdog, RtcWatchdogRegisters, SuperWatchdog,
    SuperWatchdogRegisters, WRITE_KEY, WriteProtected,
    deadline::{DeadlineBudget, DeadlineError, DeadlineWatchdog},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Op {
    Key(u32),
    Configure(MainConfig),
    Running { enabled: bool, flash_boot: bool },
    Prescale(u16),
    ResetDivider,
    Stage0(u32),
    Feed,
    Latch,
    AutoFeed(bool),
}

/// The registers' writes, shared with the test that reads them.
#[derive(Default)]
struct Log(RefCell<Vec<Op>>);

impl Log {
    fn take(&self) -> Vec<Op> {
        self.0.take()
    }
}

struct Recorder<'a>(&'a Log);

impl Recorder<'_> {
    fn record(&mut self, op: Op) {
        self.0.0.borrow_mut().push(op);
    }
}

impl WriteProtected for Recorder<'_> {
    fn store_write_key(&mut self, key: u32) {
        self.record(Op::Key(key));
    }
}

impl MainWatchdogRegisters for Recorder<'_> {
    fn configure(&mut self, config: MainConfig) {
        self.record(Op::Configure(config));
    }
    fn set_running(&mut self, enabled: bool, flash_boot: bool) {
        self.record(Op::Running {
            enabled,
            flash_boot,
        });
    }
    fn set_prescale(&mut self, prescale: u16) {
        self.record(Op::Prescale(prescale));
    }
    fn reset_divider(&mut self) {
        self.record(Op::ResetDivider);
    }
    fn set_stage0_timeout(&mut self, ticks: u32) {
        self.record(Op::Stage0(ticks));
    }
    fn feed(&mut self) {
        self.record(Op::Feed);
    }
    fn latch(&mut self) {
        self.record(Op::Latch);
    }
}

impl RtcWatchdogRegisters for Recorder<'_> {
    fn set_running(&mut self, enabled: bool, flash_boot: bool) {
        self.record(Op::Running {
            enabled,
            flash_boot,
        });
    }
}

impl SuperWatchdogRegisters for Recorder<'_> {
    fn set_auto_feed(&mut self, enabled: bool) {
        self.record(Op::AutoFeed(enabled));
    }
}

const STOP: [Op; 4] = [
    Op::Key(WRITE_KEY),
    Op::Running {
        enabled: false,
        flash_boot: false,
    },
    Op::Latch,
    Op::Key(LOCK),
];

fn tick() -> MicrosecondTick {
    MicrosecondTick::from_source_hz(40_000_000).expect("40 MHz is whole megahertz")
}

fn owned(log: &Log) -> MainWatchdog<Recorder<'_>> {
    let watchdog = MainWatchdog::new(Recorder(log), tick());
    assert_eq!(log.take(), STOP, "taking the watchdog stops it");
    watchdog
}

const ARM: [Op; 9] = [
    Op::Key(WRITE_KEY),
    Op::Prescale(40),
    Op::ResetDivider,
    Op::Stage0(100),
    Op::Latch,
    Op::Feed,
    Op::Configure(MainConfig::SYSTEM_RESET_ON_STAGE0),
    Op::Latch,
    Op::Key(LOCK),
];

#[test]
fn a_main_watchdog_is_stopped_latched_and_locked_when_taken() {
    let log = Log::default();
    let _watchdog = owned(&log);
}

#[test]
fn arming_latches_the_timeout_before_the_feed_and_the_enable_after_it() {
    let log = Log::default();
    let mut watchdog = owned(&log);
    watchdog.arm_system_reset(Microseconds::new(NonZeroU32::new(100).expect("nonzero")));
    assert_eq!(log.take(), ARM);
    let config = MainConfig::SYSTEM_RESET_ON_STAGE0;
    assert!(config.enabled && !config.flash_boot);
    assert_eq!(
        config.stages.map(MainStageAction::code),
        [3, 0, 0, 0],
        "stage 0 resets the system, the others do nothing"
    );
    assert_eq!(config.cpu_reset_length, ResetLength::Longest);
    assert_eq!(config.system_reset_length.code(), 7);
}

#[test]
fn disabling_latches_the_stop_on_every_chip() {
    let log = Log::default();
    let mut watchdog = owned(&log);
    watchdog.disable();
    assert_eq!(log.take(), STOP);
}

#[test]
fn an_unlocked_watchdog_locks_when_dropped_without_a_latch() {
    let log = Log::default();
    let mut watchdog = owned(&log);
    {
        let mut unlocked = watchdog.unlock();
        unlocked.feed();
    }
    assert_eq!(log.take(), [Op::Key(WRITE_KEY), Op::Feed, Op::Key(LOCK)]);
    watchdog.unlock().lock();
    assert_eq!(log.take(), [Op::Key(WRITE_KEY), Op::Key(LOCK)]);
}

#[test]
fn the_rtc_watchdog_stops_by_field_and_relocks() {
    let log = Log::default();
    let mut watchdog = RtcWatchdog::new(Recorder(&log));
    assert!(log.take().is_empty(), "taking it writes nothing");
    watchdog.disable();
    assert_eq!(
        log.take(),
        [
            Op::Key(WRITE_KEY),
            Op::Running {
                enabled: false,
                flash_boot: false,
            },
            Op::Key(LOCK),
        ]
    );
}

#[test]
fn the_super_watchdog_sets_only_its_automatic_feed() {
    let log = Log::default();
    let mut watchdog = SuperWatchdog::new(Recorder(&log));
    assert!(log.take().is_empty(), "taking it writes nothing");
    watchdog.feed_automatically();
    assert_eq!(
        log.take(),
        [Op::Key(WRITE_KEY), Op::AutoFeed(true), Op::Key(LOCK)]
    );
}

#[test]
fn a_microsecond_tick_needs_whole_megahertz_the_prescaler_holds() {
    let prescale = |hz| MicrosecondTick::from_source_hz(hz).map(MicrosecondTick::prescale);
    assert_eq!(prescale(40_000_000), Some(40));
    assert_eq!(prescale(48_000_000), Some(48));
    assert_eq!(prescale(26_500_000), None);
    assert_eq!(prescale(500_000), None);
    assert_eq!(prescale(0), None);
}

fn budget(micros: u32) -> DeadlineBudget {
    DeadlineBudget::from_micros(NonZeroU32::new(micros).expect("nonzero"))
}

#[test]
fn a_timely_completion_disarms_and_returns_the_watchdog() {
    let log = Log::default();
    let clock = VirtualClock::<1>::new();
    let service = DeadlineWatchdog::new(owned(&log), &clock);
    let lease = service.arm(budget(100)).expect("first arm");
    assert_eq!(log.take(), ARM);
    clock
        .advance(Duration::from_micros(99))
        .expect("the virtual clock advances");
    let service = lease.complete().expect("completion before the deadline");
    assert_eq!(log.take(), STOP, "completion stops and latches");
    let _lease = service
        .arm(budget(100))
        .expect("the returned owner arms again");
    assert_eq!(log.take(), ARM);
}

#[test]
fn a_late_completion_leaves_reset_armed() {
    let log = Log::default();
    let clock = VirtualClock::<1>::new();
    let service = DeadlineWatchdog::new(owned(&log), &clock);
    let lease = service.arm(budget(100)).expect("first arm");
    assert_eq!(log.take(), ARM);
    clock
        .advance(Duration::from_micros(100))
        .expect("the virtual clock advances");
    let (_lease, error) = lease.complete().expect_err("the deadline passed");
    assert_eq!(error, DeadlineError::Expired);
    assert!(log.take().is_empty(), "the watchdog stays armed");
}
