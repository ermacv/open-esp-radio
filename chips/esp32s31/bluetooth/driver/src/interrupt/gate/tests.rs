use super::*;
use core::{
    pin::pin,
    sync::atomic::AtomicUsize,
    task::{Context, Waker},
};
use oer_interrupt_table::signal::host::{self, Deferred};
use oer_time_virtual::VirtualClock;
use std::{
    boxed::Box,
    collections::VecDeque,
    sync::{Arc, Mutex},
    vec::Vec,
};

/// What the handler did, in order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Op {
    Acknowledge,
    PrepareOutput,
    StartTimer,
    ChannelRequest,
    Primary,
    Nrt,
    TimerStep,
    ApplyPlan,
    ObserveTimer,
    StopTimer,
    CloseBank,
}

/// One scripted timer entry.
#[derive(Clone, Copy)]
enum Step {
    Spurious,
    Rearmed,
    Pending,
}

/// The hardware the fake owners reach, shared by every owner of one test.
#[derive(Default)]
struct Model {
    ops: Vec<Op>,
    primary: VecDeque<u32>,
    nrt: VecDeque<bool>,
    channel: VecDeque<bool>,
    timer: VecDeque<Step>,
}

type Shared = Arc<Mutex<Model>>;

fn record(model: &Shared, op: Op) {
    model.lock().expect("one test thread").ops.push(op);
}

enum Fake {}

struct Setup(Shared);
struct FakeBank {
    model: Shared,
    released: bool,
}
struct Released;
struct ReleaseProof;
struct Scheduler;
struct TimerInitialized(Shared);
struct Timer(Shared);
struct TimerPending(Shared, bool);
struct Work;
struct Plan;
struct Compare;

impl GateHardware for Fake {
    type Doorbell = ();
    type Setup = Setup;
    type Bank = FakeBank;
    type Released = Released;
    type ReleaseProof = ReleaseProof;
    type Scheduler = Scheduler;
    type TimerInitialized = TimerInitialized;
    type Timer = Timer;
    type TimerPending = TimerPending;
    type TimerWork = Work;
    type TimerPlan = Plan;
    type CompareDisabled = Compare;

    fn open_bank(setup: Setup) -> FakeBank {
        record(&setup.0, Op::PrepareOutput);
        FakeBank {
            model: setup.0,
            released: false,
        }
    }

    fn start_timer(timer: TimerInitialized) -> Timer {
        record(&timer.0, Op::StartTimer);
        Timer(timer.0)
    }

    fn serve_request<C: oer_time::Clock>(_: &mut Scheduler, bank: &mut FakeBank, _: &C) -> bool {
        let mut model = bank.model.lock().expect("one test thread");
        model.ops.push(Op::ChannelRequest);
        model.channel.pop_front().unwrap_or(false)
    }

    fn capture_primary<C: oer_time::Clock>(bank: &mut FakeBank, _: &C) -> PrimaryInterruptCapture {
        let mut model = bank.model.lock().expect("one test thread");
        model.ops.push(Op::Primary);
        PrimaryInterruptCapture(model.primary.pop_front().unwrap_or(0))
    }

    fn capture_nrt(bank: &mut FakeBank) -> bool {
        let mut model = bank.model.lock().expect("one test thread");
        model.ops.push(Op::Nrt);
        model.nrt.pop_front().unwrap_or(false)
    }

    fn step_timer(timer: Timer) -> TimerEntry<Timer, TimerPending, Work> {
        let step = {
            let mut model = timer.0.lock().expect("one test thread");
            model.ops.push(Op::TimerStep);
            model.timer.pop_front().unwrap_or(Step::Spurious)
        };
        match step {
            Step::Spurious => TimerEntry::Spurious(timer),
            Step::Rearmed => TimerEntry::Rearmed(timer),
            Step::Pending => TimerEntry::SoftwarePending(TimerPending(timer.0, false), Work),
        }
    }

    fn apply_timer_plan(timer: TimerPending, _: Plan) -> TimerEntry<Timer, TimerPending, Work> {
        record(&timer.0, Op::ApplyPlan);
        if timer.1 {
            TimerEntry::SoftwarePending(TimerPending(timer.0, false), Work)
        } else {
            TimerEntry::Rearmed(Timer(timer.0))
        }
    }

    fn observe_timer(timer: &mut TimerPending) {
        let mut model = timer.0.lock().unwrap();
        model.ops.push(Op::ObserveTimer);
        if matches!(model.timer.pop_front(), Some(Step::Pending)) {
            timer.1 = true;
        }
    }

    fn stop_timer(timer: &mut Timer) -> Compare {
        record(&timer.0, Op::StopTimer);
        Compare
    }

    fn close_bank(bank: FakeBank, _: ReleaseProof) -> Result<Released, FakeBank> {
        record(&bank.model, Op::CloseBank);
        if bank.released {
            Ok(Released)
        } else {
            Err(bank)
        }
    }
}

struct Line(Shared);

impl DoorbellLine for Line {
    fn acknowledge(&mut self) {
        record(&self.0, Op::Acknowledge);
    }
}

#[derive(Clone)]
struct Ring(Arc<AtomicUsize>);

impl RadioDoorbell for Ring {
    const HANDLER: oer_interrupt_table::Context =
        oer_interrupt_table::Context { level: 3, hart: 0 };
    const EXECUTOR: oer_interrupt_table::Context =
        oer_interrupt_table::Context { level: 2, hart: 0 };
    fn ring(&self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

struct Harness {
    model: Shared,
    rings: Arc<AtomicUsize>,
    gate: BluetoothInterruptGate<Fake, Deferred, Ring>,
    port: ModemLpTimerPort<Fake, Ring>,
    part: BluetoothInterruptsOf<Fake, Deferred, Line>,
    cells: &'static ControllerEventCells<Deferred>,
    fault: &'static AtomicBool,
    clock: VirtualClock,
}

impl Harness {
    fn new() -> Self {
        let model = Shared::default();
        let rings = Arc::new(AtomicUsize::new(0));
        let storage = Box::leak(Box::new(BluetoothInterruptStorage::<Fake>::new()));
        let cells = Box::leak(Box::new(ControllerEventCells::new(
            host::deferred_signal(),
            host::deferred_signal(),
        )));
        let fault = Box::leak(Box::new(AtomicBool::new(false)));
        let (ends, part) = storage.split(
            (),
            Line(model.clone()),
            BluetoothInterruptSignals {
                reply: host::deferred_signal(),
                cells,
                fault,
            },
        );
        let (gate, port) = ends.with_ring(Ring(rings.clone()));
        Self {
            model,
            rings,
            gate,
            port,
            part,
            cells,
            fault,
            clock: VirtualClock::new(),
        }
    }

    /// One entry of the line.
    fn enter(&mut self) {
        self.part.service(&self.clock);
    }

    /// The handler's operations since the last call.
    fn ops(&self) -> Vec<Op> {
        core::mem::take(&mut self.model.lock().expect("one test thread").ops)
    }

    fn script(&self, edit: impl FnOnce(&mut Model)) {
        edit(&mut self.model.lock().expect("one test thread"));
    }

    fn owners(&self) -> (Setup, Scheduler, TimerInitialized) {
        (
            Setup(self.model.clone()),
            Scheduler,
            TimerInitialized(self.model.clone()),
        )
    }

    /// Exchange `request`: the task sends it and rings, the handler serves
    /// one entry, the task takes the reply.
    fn exchange(&mut self, request: Request<Fake>) -> Result<Reply<Fake>, GateError> {
        let mut cx = Context::from_waker(Waker::noop());
        let mut exchange = pin!(self.gate.exchange(request));
        assert!(
            exchange.as_mut().poll(&mut cx).is_pending(),
            "the reply waits for the handler"
        );
        self.part.service(&self.clock);
        match exchange.as_mut().poll(&mut cx) {
            Poll::Ready(reply) => reply,
            Poll::Pending => panic!("the handler replied"),
        }
    }

    fn open(&mut self) {
        let (setup, scheduler, timer) = self.owners();
        assert!(matches!(
            self.exchange(Request::Open {
                setup,
                scheduler,
                timer
            }),
            Ok(Reply::Opened)
        ));
    }
}

fn is_pending<X: SignalExecutor>(cells: &ControllerEventCells<X>) -> (bool, bool) {
    (
        cells
            .scheduler_wake()
            .is_pending()
            .expect("the test's executor"),
        cells
            .timer_wake()
            .is_pending()
            .expect("the test's executor"),
    )
}

#[test]
fn the_composite_serves_the_bank_timer_and_doorbell_sources() {
    use oer_interrupt_table::InterruptSources;
    assert_eq!(
        <BluetoothInterrupts<Deferred, Line> as InterruptSources>::SOURCES,
        [124, 133, 127, 68]
    );
}

#[test]
fn a_closed_gate_touches_no_register_owner() {
    let mut harness = Harness::new();
    harness.enter();
    harness.enter();
    assert_eq!(harness.ops(), [Op::Acknowledge, Op::Acknowledge]);
    assert_eq!(is_pending(harness.cells), (false, false));
}

#[test]
fn opening_prepares_the_output_then_starts_the_timer_in_the_handler() {
    let mut harness = Harness::new();
    harness.open();
    assert_eq!(harness.rings.load(Ordering::Relaxed), 1);
    // The doorbell first, then the gate's request, then this entry's
    // sample of every source.
    assert_eq!(
        harness.ops(),
        [
            Op::Acknowledge,
            Op::PrepareOutput,
            Op::StartTimer,
            Op::ChannelRequest,
            Op::Primary,
            Op::Nrt,
            Op::TimerStep,
        ]
    );
    assert!(harness.part.is_open());
}

#[test]
fn a_second_opening_returns_the_owners_untouched() {
    let mut harness = Harness::new();
    harness.open();
    harness.ops();
    let (setup, scheduler, timer) = harness.owners();
    assert!(matches!(
        harness.exchange(Request::Open {
            setup,
            scheduler,
            timer
        }),
        Ok(Reply::OpenRefused { .. })
    ));
    assert!(!harness.ops().contains(&Op::PrepareOutput));
}

#[test]
fn clear_statuses_publish_nothing_and_a_capture_reaches_its_worker() {
    let mut harness = Harness::new();
    harness.open();
    assert_eq!(is_pending(harness.cells), (false, false));
    harness.script(|model| model.primary.push_back(1 << 1));
    harness.enter();
    assert_eq!(is_pending(harness.cells), (true, false));
}

#[test]
fn a_served_channel_request_raises_the_reply_signal() {
    let mut harness = Harness::new();
    harness.open();
    harness.script(|model| model.channel.push_back(true));
    harness.enter();
    assert_eq!(
        harness
            .part
            .signals
            .reply
            .take()
            .expect("the test's executor"),
        1
    );
}

#[test]
fn the_timer_hands_its_software_work_to_the_task_and_comes_back() {
    let mut harness = Harness::new();
    harness.open();
    harness.ops();
    harness.script(|model| model.timer.push_back(Step::Pending));
    harness.enter();
    assert_eq!(is_pending(harness.cells), (false, true));
    let _work = harness.port.take_work().expect("the handler handed it on");
    // The handler retains the register owner while the task holds only Work.
    assert!(
        matches!(&harness.part.timer, super::Timer::AwaitingPlan(owner) if Arc::ptr_eq(&owner.0, &harness.model))
    );
    harness.ops();
    harness.enter();
    let ops = harness.ops();
    assert!(!ops.contains(&Op::TimerStep));
    assert!(ops.contains(&Op::ObserveTimer));
    assert!(harness.port.submit_plan(Plan).is_ok());
    assert_eq!(harness.rings.load(Ordering::Relaxed), 2);
    assert!(!harness.ops().contains(&Op::ApplyPlan));
    harness.enter();
    let ops = harness.ops();
    assert!(ops.contains(&Op::ApplyPlan));
    assert_eq!(ops.last(), Some(&Op::TimerStep));
    assert!(!harness.fault.load(Ordering::Acquire));
}

#[test]
fn a_rearmed_timer_stays_with_its_handler() {
    let mut harness = Harness::new();
    harness.open();
    harness.script(|model| model.timer.push_back(Step::Rearmed));
    harness.enter();
    assert!(harness.port.take_work().is_none());
    assert_eq!(is_pending(harness.cells), (false, false));
}

#[test]
fn the_bank_leaves_only_released() {
    let mut harness = Harness::new();
    harness.open();
    harness.ops();
    assert!(matches!(
        harness.exchange(Request::Close(ReleaseProof)),
        Ok(Reply::Refused(GateError::OutputNotReleased))
    ));
    assert!(harness.part.is_open());
    if let Bank::Open { registers, .. } = &mut harness.part.bank {
        registers.released = true;
    }
    assert!(matches!(
        harness.exchange(Request::Close(ReleaseProof)),
        Ok(Reply::Closed { .. })
    ));
    assert!(!harness.part.is_open());
    harness.ops();
    harness.enter();
    // Closed: the bank is not sampled; the timer still is.
    assert_eq!(harness.ops(), [Op::Acknowledge, Op::TimerStep]);
    assert!(matches!(
        harness.exchange(Request::Close(ReleaseProof)),
        Ok(Reply::Refused(GateError::NotOpen))
    ));
}

#[test]
fn the_timer_leaves_only_stopped_and_on_request() {
    let mut harness = Harness::new();
    harness.open();
    assert!(matches!(
        harness.exchange(Request::TakeTimer),
        Ok(Reply::Refused(GateError::TimerNotStopped))
    ));
    harness.script(|model| model.timer.push_back(Step::Pending));
    harness.enter();
    let _work = harness.port.take_work().expect("the handler handed it on");
    // The handler retains the owner but cannot stop until the plan arrives.
    assert!(matches!(
        harness.exchange(Request::StopTimer),
        Ok(Reply::Refused(GateError::TimerNotReady))
    ));
    assert!(harness.port.submit_plan(Plan).is_ok());
    harness.ops();
    assert!(matches!(
        harness.exchange(Request::StopTimer),
        Ok(Reply::TimerStopped(Compare))
    ));
    assert!(harness.ops().contains(&Op::StopTimer));
    // Stopped, the handler still quiets the timer's status.
    harness.enter();
    assert_eq!(harness.ops().last(), Some(&Op::TimerStep));
    assert!(matches!(
        harness.exchange(Request::TakeTimer),
        Ok(Reply::Timer(_))
    ));
    harness.ops();
    harness.enter();
    assert!(!harness.ops().contains(&Op::TimerStep));
}

#[test]
fn stopped_timer_work_is_retained_as_a_fault() {
    let mut harness = Harness::new();
    harness.open();
    assert!(matches!(
        harness.exchange(Request::StopTimer),
        Ok(Reply::TimerStopped(_))
    ));
    harness.script(|model| model.timer.push_back(Step::Pending));
    harness.enter();
    assert!(harness.fault.load(Ordering::Acquire));
    assert!(harness.port.take_work().is_none());
    assert!(matches!(
        harness.exchange(Request::TakeTimer),
        Ok(Reply::Refused(GateError::TimerNotStopped))
    ));
}

#[test]
fn a_plan_without_outstanding_work_is_retained_as_a_fault() {
    let mut harness = Harness::new();
    harness.open();
    assert!(harness.port.submit_plan(Plan).is_ok());
    harness.enter();
    assert!(harness.fault.load(Ordering::Acquire));
}

#[test]
fn an_abandoned_request_closes_the_gate_to_the_task() {
    let mut harness = Harness::new();
    {
        let mut cx = Context::from_waker(Waker::noop());
        let mut exchange = pin!(harness.gate.exchange(Request::StopTimer));
        assert!(exchange.as_mut().poll(&mut cx).is_pending());
    }
    let mut cx = Context::from_waker(Waker::noop());
    let mut next = pin!(harness.gate.exchange(Request::TakeTimer));
    assert!(matches!(
        next.as_mut().poll(&mut cx),
        Poll::Ready(Err(GateError::Abandoned))
    ));
}

#[test]
fn a_foreign_executor_cannot_await_a_reply() {
    let model = Shared::default();
    let storage = Box::leak(Box::new(BluetoothInterruptStorage::<Fake>::new()));
    let cells = Box::leak(Box::new(ControllerEventCells::new(
        host::foreign_signal(),
        host::foreign_signal(),
    )));
    let (ends, _part) = storage.split(
        (),
        Line(model),
        BluetoothInterruptSignals {
            reply: host::foreign_signal(),
            cells,
            fault: Box::leak(Box::new(AtomicBool::new(false))),
        },
    );
    let (mut gate, _port) = ends.with_ring(Ring(Arc::new(AtomicUsize::new(0))));
    let mut cx = Context::from_waker(Waker::noop());
    let mut exchange = pin!(gate.exchange(Request::StopTimer));
    assert!(matches!(
        exchange.as_mut().poll(&mut cx),
        Poll::Ready(Err(GateError::ForeignContext))
    ));
}

#[test]
fn an_event_between_plan_and_application_waits_for_the_retained_owner() {
    let mut harness = Harness::new();
    harness.open();
    harness.script(|model| model.timer.push_back(Step::Pending));
    harness.enter();
    let _work = harness.port.take_work().unwrap();
    harness.cells.timer_wake().take().unwrap();
    harness.ops();
    // The handler acknowledges an arrival after the snapshot and retains its work.
    harness
        .clock
        .advance(oer_time::Duration::from_micros(10))
        .unwrap();
    harness.script(|model| model.timer.push_back(Step::Pending));
    harness.enter();
    let ops = harness.ops();
    assert!(!ops.contains(&Op::TimerStep));
    assert!(ops.contains(&Op::ObserveTimer));
    assert!(harness.port.submit_plan(Plan).is_ok());
    assert!(!harness.ops().contains(&Op::ApplyPlan));
    harness.enter();
    let ops = harness.ops();
    assert_eq!(ops[1], Op::ApplyPlan);
    assert!(harness.port.take_work().is_some());
    assert_eq!(is_pending(harness.cells), (false, true));
    assert!(!harness.fault.load(Ordering::Acquire));
}
