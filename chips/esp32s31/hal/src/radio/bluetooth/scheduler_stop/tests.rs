use super::{BluetoothSchedulerStop, Control, Progress, Unsettled};
use core::task::{Context, Poll, Waker};
use std::{collections::VecDeque, vec, vec::Vec};
/// What a command engine's status says: set when it is idle.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Engine {
    Busy,
    Idle,
}
/// How the model replies to a request it was given.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Replies {
    /// Every reply is ready at once.
    Immediate,
    /// A request's first poll yields; none is owed now.
    Yield,
    /// A request's first poll yields, and the last request has not yielded.
    YieldOwed,
}
struct Model {
    busy: VecDeque<Option<bool>>,
    c0: Engine,
    c1: Engine,
    trace: Vec<&'static str>,
    replies: Replies,
}
impl Model {
    fn reply(&mut self) -> bool {
        if self.replies == Replies::YieldOwed {
            self.replies = Replies::Yield;
            false
        } else {
            true
        }
    }
    fn begin(&mut self) {
        if self.replies == Replies::Yield {
            self.replies = Replies::YieldOwed;
        }
    }
}
impl Control for Model {
    type Busy = bool;
    type Stopped = ();
    type Error = Unsettled;
    fn begin_busy(&mut self) -> Result<(), Unsettled> {
        self.trace.push("busy");
        self.begin();
        Ok(())
    }
    fn poll_busy(&mut self, _: &mut Context<'_>) -> Poll<Result<bool, Unsettled>> {
        if !self.reply() {
            return Poll::Pending;
        }
        Poll::Ready(
            self.busy
                .pop_front()
                .expect("one observation per request")
                .ok_or(Unsettled),
        )
    }
    fn is_busy(busy: &bool) -> bool {
        *busy
    }
    fn confirm_stopped(&mut self, busy: bool) -> Option<()> {
        if busy {
            None
        } else {
            self.trace.push("fence");
            Some(())
        }
    }
    fn begin_mask(&mut self) -> Result<(), Unsettled> {
        self.trace.push("mask");
        self.begin();
        Ok(())
    }
    fn poll_mask(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Unsettled>> {
        if self.reply() {
            Poll::Ready(Ok(()))
        } else {
            Poll::Pending
        }
    }
    fn disable_run(&mut self) {
        self.trace.extend(["disable-run", "fence"]);
    }
    fn commands_ready(&mut self) -> bool {
        self.trace.push("command-0");
        if self.c0 == Engine::Busy {
            return false;
        }
        self.trace.push("command-1");
        self.c1 == Engine::Idle
    }
    fn request(&mut self) {
        self.trace.extend(["request", "fence"]);
    }
}
fn model(busy: &[bool], c0: Engine, c1: Engine) -> Model {
    Model {
        busy: busy.iter().copied().map(Some).collect(),
        c0,
        c1,
        trace: vec![],
        replies: Replies::Immediate,
    }
}
fn step(stop: &mut BluetoothSchedulerStop, hw: &mut Model) -> Result<Progress<()>, Unsettled> {
    let Poll::Ready(result) = super::poll(stop, hw, &mut Context::from_waker(Waker::noop())) else {
        panic!("immediate model reply")
    };
    result
}
#[test]
fn idle_has_no_lifecycle_side_effect() {
    let mut hw = model(&[false], Engine::Busy, Engine::Busy);
    assert!(matches!(
        step(&mut BluetoothSchedulerStop::default(), &mut hw),
        Ok(Progress::Stopped(()))
    ));
    assert_eq!(hw.trace, ["busy", "fence"]);
}
#[test]
fn silence_stop_waits_for_preamble_and_publishes_only_once() {
    let mut hw = model(&[true, true], Engine::Busy, Engine::Busy);
    let mut stop = BluetoothSchedulerStop::default();
    assert!(matches!(step(&mut stop, &mut hw), Ok(Progress::Pending)));
    assert_eq!(
        hw.trace,
        ["busy", "mask", "disable-run", "fence", "busy", "command-0"]
    );
    hw.c0 = Engine::Idle;
    hw.c1 = Engine::Idle;
    hw.busy.extend([Some(true), Some(true)]);
    hw.trace.clear();
    assert!(matches!(step(&mut stop, &mut hw), Ok(Progress::Pending)));
    assert_eq!(
        hw.trace,
        ["busy", "command-0", "command-1", "request", "fence", "busy"]
    );
    hw.busy.push_back(Some(false));
    hw.trace.clear();
    assert!(matches!(
        step(&mut stop, &mut hw),
        Ok(Progress::Stopped(()))
    ));
    assert_eq!(hw.trace, ["busy", "fence"]);
}
#[test]
fn completion_racing_preamble_skips_command_reads() {
    let mut hw = model(&[true, false, false], Engine::Busy, Engine::Busy);
    assert!(matches!(
        step(&mut BluetoothSchedulerStop::default(), &mut hw),
        Ok(Progress::Stopped(()))
    ));
    assert_eq!(
        hw.trace,
        [
            "busy",
            "mask",
            "disable-run",
            "fence",
            "busy",
            "request",
            "fence",
            "busy",
            "fence"
        ]
    );
}
#[test]
fn second_command_not_ready_keeps_owner_without_request() {
    let mut hw = model(&[true, true], Engine::Idle, Engine::Busy);
    assert!(matches!(
        step(&mut BluetoothSchedulerStop::default(), &mut hw),
        Ok(Progress::Pending)
    ));
    assert!(!hw.trace.contains(&"request"));
}
#[test]
fn unsettled_busy_sample_fails_before_any_request() {
    let mut hw = model(&[true], Engine::Busy, Engine::Busy);
    hw.busy.push_back(None);
    assert!(matches!(
        step(&mut BluetoothSchedulerStop::default(), &mut hw),
        Err(Unsettled)
    ));
    assert_eq!(hw.trace, ["busy", "mask", "disable-run", "fence", "busy"]);
}
#[test]
fn every_pending_p3_reply_resumes_without_republishing_or_reordering() {
    let mut hw = model(&[true, true, false], Engine::Idle, Engine::Idle);
    hw.replies = Replies::Yield;
    let mut stop = BluetoothSchedulerStop::default();
    let mut yields = 0;
    loop {
        // A new borrower per poll; all transaction progress stays in stop/hw.
        match super::poll(&mut stop, &mut hw, &mut Context::from_waker(Waker::noop())) {
            Poll::Pending => {
                yields += 1;
                assert!(stop.is_pending());
                assert!(yields <= 4);
            }
            Poll::Ready(result) => {
                assert!(matches!(result, Ok(Progress::Stopped(()))));
                break;
            }
        }
    }
    assert_eq!(yields, 4);
    assert_eq!(
        hw.trace,
        [
            "busy",
            "mask",
            "disable-run",
            "fence",
            "busy",
            "command-0",
            "command-1",
            "request",
            "fence",
            "busy",
            "fence"
        ]
    );
}
