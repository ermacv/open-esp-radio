use core::cell::RefCell;
use std::{rc::Rc, vec::Vec};

use oer_time::{Clock, Duration, Instant};
use oer_time_virtual::VirtualClock;

use super::Ticking;
use crate::{
    Access, BootFailure, Busy, ClockFailure, WaitFailure,
    clock::{
        ActivePllPower, ClockRates, ClockUpdate, ClockWaits, CpuClockProfile, CpuClockRegisters,
        PllPower, PllRegisters, bring_up,
    },
    wait::{BootBudget, BootDeadline, BootWait},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Root {
    Xtal,
    Pll,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Op {
    SetTarget(u8),
    PowerUp,
    KeepCpuPll,
    KeepMemoryPll,
    StartCalibration,
    StopCalibration { after_done: Duration },
    Stage(Root, u8),
    StartUpdate,
}

const WAITS: ClockWaits = ClockWaits {
    calibration: BootWait::new("pll-calibration", Duration::from_micros(100)),
    calibration_settle: BootWait::new("pll-settle", Duration::from_micros(10)),
    update: BootWait::new("clock-update", Duration::from_micros(100)),
};

const RATES: ClockRates = ClockRates {
    cpu_hz: 160_000_000,
    mem_hz: 160_000_000,
    sys_hz: 80_000_000,
    apb_hz: 40_000_000,
};

const PROFILE: CpuClockProfile<Root, u8, u8> = CpuClockProfile {
    root: Root::Pll,
    pll: Some(8),
    dividers: 1,
    rates: RATES,
};

/// A clock tree whose PLL calibration ends `calibration_takes` after it
/// starts and whose update completes `update_takes` after it starts; `None`
/// never.
struct Tree<'a> {
    clock: &'a VirtualClock,
    log: Rc<RefCell<Vec<Op>>>,
    root: Option<Root>,
    staged: (Root, u8),
    applied_divider: u8,
    target: u8,
    calibration_takes: Option<Duration>,
    calibration_done_at: Option<Instant>,
    update_takes: Option<Duration>,
    update_done_at: Option<Instant>,
    /// An update the bootloader left running until this instant.
    running_until: Option<Instant>,
    ignores_update: bool,
}

impl<'a> Tree<'a> {
    fn new(clock: &'a VirtualClock, root: Root, divider: u8, target: u8) -> Self {
        Self {
            clock,
            log: Rc::new(RefCell::new(Vec::new())),
            root: Some(root),
            staged: (root, divider),
            applied_divider: divider,
            target,
            calibration_takes: Some(Duration::from_micros(30)),
            calibration_done_at: None,
            update_takes: Some(Duration::from_micros(2)),
            update_done_at: None,
            running_until: None,
            ignores_update: false,
        }
    }

    fn record(&self, op: Op) {
        self.log.borrow_mut().push(op);
    }

    fn ops(&self) -> Vec<Op> {
        self.log.borrow().clone()
    }

    /// Whether a completed update applied the staged configuration.
    fn applied(&self) -> bool {
        !self.ignores_update && self.update_done_at.is_some_and(|at| self.clock.now() >= at)
    }
}

impl CpuClockRegisters for Tree<'_> {
    type Root = Root;
    type Dividers = u8;
    fn root(&self, _: Access<'_>) -> Option<Root> {
        if self.applied() {
            Some(self.staged.0)
        } else {
            self.root
        }
    }
    fn dividers(&self, _: Access<'_>) -> u8 {
        if self.applied() {
            self.staged.1
        } else {
            self.applied_divider
        }
    }
    fn stage(&mut self, _: Access<'_>, root: Root, dividers: u8) {
        self.record(Op::Stage(root, dividers));
        self.staged = (root, dividers);
    }
}

impl ClockUpdate for Tree<'_> {
    fn start_update(&mut self, _: Access<'_>) -> Result<(), Busy> {
        let now = self.clock.now();
        let running = self.running_until.is_some_and(|until| now < until)
            || self.update_done_at.is_some_and(|at| now < at);
        if running {
            return Err(Busy);
        }
        self.record(Op::StartUpdate);
        self.update_done_at = self
            .update_takes
            .map(|takes| self.clock.now().saturating_add(takes));
        Ok(())
    }
    fn update_pending(&self, _: Access<'_>) -> bool {
        !self.update_done_at.is_some_and(|at| self.clock.now() >= at)
    }
}

impl PllRegisters for Tree<'_> {
    type Target = u8;
    fn target(&self, _: Access<'_>) -> u8 {
        self.target
    }
    fn set_target(&mut self, _: Access<'_>, target: u8) {
        self.record(Op::SetTarget(target));
        self.target = target;
    }
    fn start_calibration(&mut self, _: Access<'_>) {
        self.record(Op::StartCalibration);
        self.calibration_done_at = self
            .calibration_takes
            .map(|takes| self.clock.now().saturating_add(takes));
    }
    fn calibration_done(&self, _: Access<'_>) -> bool {
        self.calibration_done_at
            .is_some_and(|at| self.clock.now() >= at)
    }
    fn stop_calibration(&mut self, _: Access<'_>) {
        let done = self.calibration_done_at.expect("stopped after it started");
        self.record(Op::StopCalibration {
            after_done: self.clock.now().saturating_duration_since(done),
        });
    }
}

impl PllPower for Tree<'_> {
    fn power_up(&mut self, _: Access<'_>) {
        self.record(Op::PowerUp);
    }
}

/// The PMU's active-state power, logged with the clock tree's operations.
struct Active(Rc<RefCell<Vec<Op>>>);

impl ActivePllPower for Active {
    fn keep_cpu_pll(&mut self, _: Access<'_>) {
        self.0.borrow_mut().push(Op::KeepCpuPll);
    }
    fn keep_memory_pll(&mut self, _: Access<'_>) {
        self.0.borrow_mut().push(Op::KeepMemoryPll);
    }
}

fn bring_up_on(tree: &mut Tree<'_>, clock: &VirtualClock) -> Result<ClockRates, BootFailure> {
    let ticking = Ticking::new(clock);
    let budget = BootBudget::below(Duration::from_millis(10), Duration::from_secs(9))
        .expect("below the watchdog");
    let deadline = BootDeadline::start(&ticking, budget);
    let mut active = Active(Rc::clone(&tree.log));
    bring_up(tree, &mut active, PROFILE, WAITS, &deadline)
}

#[test]
fn a_cold_pll_is_configured_powered_calibrated_settled_and_stopped_before_one_update() {
    let clock = VirtualClock::new();
    let mut tree = Tree::new(&clock, Root::Xtal, 0, 15);
    assert_eq!(bring_up_on(&mut tree, &clock), Ok(RATES));
    let ops = tree.ops();
    assert_eq!(
        &ops[..4],
        &[
            Op::SetTarget(8),
            Op::PowerUp,
            Op::KeepCpuPll,
            Op::StartCalibration
        ]
    );
    let Some(Op::StopCalibration { after_done }) = ops.get(4).copied() else {
        panic!("the calibration stops fifth: {ops:?}");
    };
    assert!(
        after_done >= WAITS.calibration_settle.bound(),
        "{after_done:?}"
    );
    assert_eq!(&ops[5..], &[Op::Stage(Root::Pll, 1), Op::StartUpdate]);
}

#[test]
fn a_live_pll_is_never_recalibrated() {
    let clock = VirtualClock::new();
    let mut tree = Tree::new(&clock, Root::Pll, 3, 8);
    assert_eq!(bring_up_on(&mut tree, &clock), Ok(RATES));
    // Only the dividers change, through one update.
    assert_eq!(tree.ops(), [Op::Stage(Root::Pll, 1), Op::StartUpdate]);
}

#[test]
fn a_clock_already_at_its_profile_is_not_written() {
    let clock = VirtualClock::new();
    let mut tree = Tree::new(&clock, Root::Pll, 1, 8);
    assert_eq!(bring_up_on(&mut tree, &clock), Ok(RATES));
    assert_eq!(tree.ops(), []);
}

#[test]
fn a_live_pll_in_another_configuration_stops_the_boot_unwritten() {
    let clock = VirtualClock::new();
    let mut tree = Tree::new(&clock, Root::Pll, 1, 15);
    assert_eq!(
        bring_up_on(&mut tree, &clock),
        Err(BootFailure::Clock(ClockFailure::LivePllMismatch))
    );
    assert_eq!(tree.ops(), []);
}

#[test]
fn an_unknown_root_stops_the_boot_unwritten() {
    let clock = VirtualClock::new();
    let mut tree = Tree::new(&clock, Root::Xtal, 0, 15);
    tree.root = None;
    assert_eq!(
        bring_up_on(&mut tree, &clock),
        Err(BootFailure::Clock(ClockFailure::UnknownRoot))
    );
    assert_eq!(tree.ops(), []);
}

#[test]
fn a_calibration_that_never_ends_is_a_named_timeout_before_any_divider() {
    let clock = VirtualClock::new();
    let mut tree = Tree::new(&clock, Root::Xtal, 0, 15);
    tree.calibration_takes = None;
    assert_eq!(
        bring_up_on(&mut tree, &clock),
        Err(BootFailure::Wait(WaitFailure::TimedOut("pll-calibration")))
    );
    assert_eq!(
        tree.ops(),
        [
            Op::SetTarget(8),
            Op::PowerUp,
            Op::KeepCpuPll,
            Op::StartCalibration
        ]
    );
}

#[test]
fn an_update_that_never_completes_is_a_named_timeout() {
    let clock = VirtualClock::new();
    let mut tree = Tree::new(&clock, Root::Pll, 3, 8);
    tree.update_takes = None;
    assert_eq!(
        bring_up_on(&mut tree, &clock),
        Err(BootFailure::Wait(WaitFailure::TimedOut("clock-update")))
    );
}

#[test]
fn a_configuration_that_does_not_read_back_stops_the_boot() {
    let clock = VirtualClock::new();
    let mut tree = Tree::new(&clock, Root::Pll, 3, 8);
    tree.ignores_update = true;
    assert_eq!(
        bring_up_on(&mut tree, &clock),
        Err(BootFailure::Clock(ClockFailure::Readback))
    );
}

#[test]
fn an_update_starts_once_the_running_one_completes_and_never_over_it() {
    let clock = VirtualClock::new();
    let mut tree = Tree::new(&clock, Root::Pll, 3, 8);
    tree.running_until = Some(clock.now().saturating_add(Duration::from_micros(20)));
    assert_eq!(bring_up_on(&mut tree, &clock), Ok(RATES));
    assert_eq!(tree.ops(), [Op::Stage(Root::Pll, 1), Op::StartUpdate]);
}

#[test]
fn an_update_that_never_lets_the_next_start_is_a_named_timeout() {
    let clock = VirtualClock::new();
    let mut tree = Tree::new(&clock, Root::Pll, 3, 8);
    tree.running_until = Some(Instant::from_micros(u64::MAX));
    assert_eq!(
        bring_up_on(&mut tree, &clock),
        Err(BootFailure::Wait(WaitFailure::TimedOut("clock-update")))
    );
    assert_eq!(tree.ops(), [Op::Stage(Root::Pll, 1)]);
}
