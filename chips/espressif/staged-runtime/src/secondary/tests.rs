//! The launch's order and waits over a scripted hart and a ticking virtual
//! clock: each test has its own handshake model.

use core::cell::{Cell, RefCell};
use std::{vec, vec::Vec};

use oer_espressif_boot::wait::{BootBudget, BootDeadline, BootWait, WaitFailure};
use oer_time::{Clock, Duration, Instant};
use oer_time_virtual::VirtualClock;

use super::*;

/// A clock that advances one microsecond at every reading.
struct Ticking(VirtualClock);

impl Clock for Ticking {
    fn now(&self) -> Instant {
        let now = self.0.now();
        self.0
            .advance(Duration::from_micros(1))
            .expect("test time fits");
        now
    }
}

const fn wait(name: &'static str, micros: u64) -> BootWait {
    match BootWait::new(name, Duration::from_micros(micros)) {
        Ok(wait) => wait,
        Err(_) => panic!("a test wait has a nonzero bound"),
    }
}

const WAITS: LaunchWaits = LaunchWaits {
    entered: wait("secondary-entered", 100),
    ready: wait("secondary-ready", 100),
};

/// The hart as the boot hart sees it: it enters after `enter_after` reads of
/// `entered`, and reports ready (or faults) `ready_after` reads of `ready`
/// after the acknowledgement.
struct Hart {
    events: RefCell<Vec<&'static str>>,
    enter_after: Option<u32>,
    reads: Cell<u32>,
    acked: Cell<bool>,
    ready_after: Option<u32>,
    ready_reads: Cell<u32>,
    early_fault: Option<EarlyFault>,
    late_fault: Option<EarlyFault>,
    sp: u32,
    used: bool,
}

impl Hart {
    fn new() -> Self {
        Self {
            events: RefCell::new(Vec::new()),
            enter_after: Some(3),
            reads: Cell::new(0),
            acked: Cell::new(false),
            ready_after: Some(3),
            ready_reads: Cell::new(0),
            early_fault: None,
            late_fault: None,
            sp: 0x5010_0ff0,
            used: false,
        }
    }
}

impl HandshakeView for Hart {
    fn entered(&self) -> u32 {
        if self.used {
            return ENTERED;
        }
        if self.events.borrow().contains(&"release") {
            self.reads.set(self.reads.get() + 1);
        }
        match self.enter_after {
            Some(after) if self.reads.get() > after => ENTERED,
            _ => 0,
        }
    }
    fn ack(&self) -> u32 {
        if self.acked.get() { ACK } else { 0 }
    }
    fn ready(&self) -> u32 {
        if self.events.borrow().contains(&"release") && self.early_fault.is_some() {
            return FAULT;
        }
        if !self.acked.get() {
            return 0;
        }
        self.ready_reads.set(self.ready_reads.get() + 1);
        match self.ready_after {
            Some(after) if self.ready_reads.get() > after => {
                if self.late_fault.is_some() {
                    FAULT
                } else {
                    READY
                }
            }
            _ => 0,
        }
    }
    fn ready_sp(&self) -> u32 {
        self.sp
    }
    fn fault(&self) -> EarlyFault {
        self.early_fault
            .or(self.late_fault)
            .expect("a fault is reported")
    }
    fn acknowledge(&self) {
        self.events.borrow_mut().push("ack");
        self.acked.set(true);
    }
}

fn stacks() -> Stacks {
    Stacks {
        task: 0x5010_0000..0x5010_1000,
        irq: 0x400,
    }
}

fn launch(hart: &Hart, stacks: Stacks) -> Result<(u8, u32), LaunchFailure<u8>> {
    let clock = Ticking(VirtualClock::starting_at(Instant::EPOCH));
    let budget = BootBudget::below(Duration::from_millis(1), Duration::from_secs(9))
        .expect("below the watchdog");
    let deadline = BootDeadline::start(&clock, budget);
    run(
        hart,
        &deadline,
        WAITS,
        stacks,
        &mut (),
        |()| {
            hart.events.borrow_mut().push("release");
            Ok(7_u8)
        },
        |(), released| {
            assert_eq!(*released, 7);
            hart.events.borrow_mut().push("retire");
            Ok(())
        },
        || hart.events.borrow_mut().push("fence"),
    )
}

#[test]
fn a_launch_releases_waits_for_the_entry_retires_fences_acknowledges_and_waits_ready() {
    let hart = Hart::new();
    assert_eq!(launch(&hart, stacks()), Ok((7, 0x5010_0ff0)));
    assert_eq!(*hart.events.borrow(), ["release", "retire", "fence", "ack"]);
}

#[test]
fn an_unprovisioned_hart_or_a_used_handshake_is_refused_before_the_release() {
    let hart = Hart::new();
    let empty = Stacks {
        task: 0x5010_0000..0x5010_0000,
        irq: 0x400,
    };
    assert_eq!(
        launch(&hart, empty),
        Err(LaunchFailure::Unprovisioned {
            task: 0,
            irq: 0x400
        })
    );
    let no_irq = Stacks {
        task: 0x5010_0000..0x5010_1000,
        irq: 0,
    };
    assert_eq!(
        launch(&hart, no_irq),
        Err(LaunchFailure::Unprovisioned {
            task: 0x1000,
            irq: 0
        })
    );
    let mut used = Hart::new();
    used.used = true;
    assert!(matches!(
        launch(&used, stacks()),
        Err(LaunchFailure::HandshakeUsed {
            entered: ENTERED,
            ..
        })
    ));
    assert!(hart.events.borrow().is_empty() && used.events.borrow().is_empty());
}

#[test]
fn an_entry_that_never_reports_times_out_without_an_acknowledgement() {
    let mut hart = Hart::new();
    hart.enter_after = None;
    assert_eq!(
        launch(&hart, stacks()),
        Err(LaunchFailure::NotEntered(WaitFailure::TimedOut(
            "secondary-entered"
        )))
    );
    assert_eq!(*hart.events.borrow(), ["release"]);
}

#[test]
fn an_early_fault_is_reported_with_its_observations_before_the_boot_word_retires() {
    let mut hart = Hart::new();
    let fault = EarlyFault {
        kind: KIND_PMA,
        cause: 0x103,
        pc: 0x0b40_0000,
        value: 0,
    };
    hart.early_fault = Some(fault);
    assert_eq!(
        launch(&hart, stacks()),
        Err(LaunchFailure::EarlyFault(fault))
    );
    assert_eq!(*hart.events.borrow(), ["release"]);
}

#[test]
fn a_hart_that_never_reports_ready_or_faults_after_the_acknowledgement_fails() {
    let mut hart = Hart::new();
    hart.ready_after = None;
    assert_eq!(
        launch(&hart, stacks()),
        Err(LaunchFailure::NotReady(WaitFailure::TimedOut(
            "secondary-ready"
        )))
    );
    let mut hart = Hart::new();
    let fault = EarlyFault {
        kind: KIND_INTERRUPT_STACK,
        cause: 2,
        pc: 0,
        value: 0,
    };
    hart.late_fault = Some(fault);
    assert_eq!(
        launch(&hart, stacks()),
        Err(LaunchFailure::EarlyFault(fault))
    );
    assert_eq!(*hart.events.borrow(), ["release", "retire", "fence", "ack"]);
}

#[test]
fn a_ready_stack_pointer_outside_the_task_stack_is_refused() {
    for sp in [0x5010_1004, 0x500f_fff0, 0] {
        let mut hart = Hart::new();
        hart.sp = sp;
        assert_eq!(
            launch(&hart, stacks()),
            Err(LaunchFailure::StackOutside { sp })
        );
    }
}

#[test]
fn refused_releases_and_retirements_keep_their_platform_errors() {
    let hart = Hart::new();
    let clock = Ticking(VirtualClock::starting_at(Instant::EPOCH));
    let budget = BootBudget::below(Duration::from_millis(1), Duration::from_secs(9))
        .expect("below the watchdog");
    let deadline = BootDeadline::start(&clock, budget);
    let refused = run(
        &hart,
        &deadline,
        WAITS,
        stacks(),
        &mut (),
        |()| Err::<u8, u8>(3),
        |(), _| Ok(()),
        || (),
    );
    assert_eq!(refused, Err(LaunchFailure::Start(3)));
    let kept = run(
        &hart,
        &deadline,
        WAITS,
        stacks(),
        &mut (),
        |()| {
            hart.events.borrow_mut().push("release");
            Ok::<u8, u8>(1)
        },
        |(), _| Err(9),
        || panic!("no fence after a kept boot word"),
    );
    assert_eq!(kept, Err(LaunchFailure::BootWord(9)));
    assert!(
        !hart.acked.get(),
        "no acknowledgement after a kept boot word"
    );
}

#[test]
fn each_failure_keeps_its_stage_and_observations_in_the_retained_words() {
    use crate::panic::FaultReason;
    let observe = |error: u8| [u32::from(error), 0xaa, 0xbb];
    let fault = EarlyFault {
        kind: 1,
        cause: 2,
        pc: 3,
        value: 4,
    };
    let cases: Vec<(LaunchFailure<u8>, FaultReason, [u32; 4])> = vec![
        (
            LaunchFailure::EarlyFault(fault),
            FaultReason::SecondaryEarlyFault,
            [1, 2, 3, 4],
        ),
        (
            LaunchFailure::Unprovisioned { task: 5, irq: 6 },
            FaultReason::SecondaryLaunch,
            [1, 5, 6, 0],
        ),
        (
            LaunchFailure::Start(7),
            FaultReason::SecondaryLaunch,
            [4, 7, 0xaa, 0xbb],
        ),
        (
            LaunchFailure::NotEntered(WaitFailure::DeadlineExceeded("secondary-entered")),
            FaultReason::SecondaryLaunch,
            [5, 2, 0, 0],
        ),
        (
            LaunchFailure::BootWord(8),
            FaultReason::SecondaryLaunch,
            [6, 8, 0xaa, 0xbb],
        ),
        (
            LaunchFailure::StackOutside { sp: 9 },
            FaultReason::SecondaryLaunch,
            [8, 9, 0, 0],
        ),
    ];
    for (failure, reason, words) in cases {
        assert_eq!(failure.diagnostic(observe), (reason, words));
    }
    assert_eq!(
        FaultReason::SecondaryEarlyFault.class(),
        oer_diagnostics::fault::Class::UnexpectedHardware
    );
    assert_eq!(
        FaultReason::SecondaryLaunch.class(),
        oer_diagnostics::fault::Class::ConfigurationBoot
    );
}

#[test]
fn the_handshake_reports_a_fault_after_its_observations() {
    let handshake = Handshake::new();
    handshake.report_fault(KIND_PMP, 1, 0, 0x1f);
    assert_eq!(HandshakeView::ready(&handshake), FAULT);
    assert_eq!(
        HandshakeView::fault(&handshake),
        EarlyFault {
            kind: KIND_PMP,
            cause: 1,
            pc: 0,
            value: 0x1f
        }
    );
    handshake.acknowledge();
    assert_eq!(HandshakeView::ack(&handshake), ACK);
}
