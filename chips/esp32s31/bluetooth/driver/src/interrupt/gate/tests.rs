use super::*;
use core::{
    pin::pin,
    sync::atomic::{AtomicUsize, Ordering},
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

fn invariant(fault: GateFault) -> (GateFaultReason, GateObservation) {
    match fault {
        GateFault::Invariant {
            reason,
            observation,
        } => (reason, observation),
        GateFault::InterruptPolicy(cause) => panic!("expected gate entry, got {cause:?}"),
    }
}

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
    StoppedTimerStep,
    CloseBank,
}

/// One scripted timer entry.
#[derive(Clone, Copy)]
enum Step {
    Spurious,
    Rearmed,
    Pending,
    Rollover,
    Unknown,
}

/// The hardware the fake owners reach, shared by every owner of one test.
#[derive(Default)]
struct Model {
    ops: Vec<Op>,
    primary: VecDeque<u32>,
    nrt: VecDeque<bool>,
    nrt_fault: Option<bluetooth::BluetoothNrtInterruptFault>,
    channel: VecDeque<bool>,
    policy_fault: Option<bluetooth::BluetoothInterruptPolicyFault>,
    timer: VecDeque<Step>,
    lp_status: u32,
    nrt_status: [u32; 2],
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
struct TimerPending(Shared, bool, bool);
struct StoppedTimer(Shared);
struct Work;
struct Plan;
struct Compare;

impl GateHardware for Fake {
    type Doorbell = ();
    type Status = Shared;
    fn lp_status(status: &Shared) -> u32 {
        status.lock().unwrap().lp_status
    }
    fn nrt_status(bank: &FakeBank) -> [u32; 2] {
        bank.model.lock().unwrap().nrt_status
    }

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
    type StoppedTimer = StoppedTimer;
    type StopProof = Compare;

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

    fn serve_request<C: oer_time::Clock>(
        _: &mut Scheduler,
        bank: &mut FakeBank,
        _: &C,
    ) -> Result<bool, bluetooth::BluetoothInterruptPolicyFault> {
        let mut model = bank.model.lock().expect("one test thread");
        model.ops.push(Op::ChannelRequest);
        if let Some(cause) = model.policy_fault.take() {
            return Err(cause);
        }
        Ok(model.channel.pop_front().unwrap_or(false))
    }

    fn capture_primary<C: oer_time::Clock>(bank: &mut FakeBank, _: &C) -> PrimaryInterruptCapture {
        let mut model = bank.model.lock().expect("one test thread");
        model.ops.push(Op::Primary);
        PrimaryInterruptCapture(model.primary.pop_front().unwrap_or(0))
    }

    fn capture_nrt(bank: &mut FakeBank) -> Result<bool, bluetooth::BluetoothNrtInterruptFault> {
        let mut model = bank.model.lock().expect("one test thread");
        model.ops.push(Op::Nrt);
        match model.nrt_fault.take() {
            Some(fault) => Err(fault),
            None => Ok(model.nrt.pop_front().unwrap_or(false)),
        }
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
            Step::Pending => TimerEntry::SoftwarePending(TimerPending(timer.0, true, false), Work),
            Step::Rollover => {
                TimerEntry::SoftwarePending(TimerPending(timer.0, false, false), Work)
            }
            Step::Unknown => panic!("unknown status only scripted while stopped"),
        }
    }

    fn apply_timer_plan(timer: TimerPending, _: Plan) -> TimerEntry<Timer, TimerPending, Work> {
        record(&timer.0, Op::ApplyPlan);
        if timer.2 {
            TimerEntry::SoftwarePending(TimerPending(timer.0, true, false), Work)
        } else {
            TimerEntry::Rearmed(Timer(timer.0))
        }
    }

    fn observe_timer(timer: &mut TimerPending) {
        let mut model = timer.0.lock().unwrap();
        model.ops.push(Op::ObserveTimer);
        if matches!(model.timer.pop_front(), Some(Step::Pending)) {
            timer.2 = true;
        }
    }

    fn stop_timer(timer: Timer) -> (StoppedTimer, Compare) {
        record(&timer.0, Op::StopTimer);
        (StoppedTimer(timer.0), Compare)
    }

    fn stop_pending_timer(timer: TimerPending) -> Result<(StoppedTimer, Compare), TimerPending> {
        if timer.1 || timer.2 {
            return Err(timer);
        }
        Ok(Self::stop_timer(Timer(timer.0)))
    }

    fn step_stopped_timer(
        timer: &mut StoppedTimer,
    ) -> Result<
        bluetooth::BluetoothModemLpTimerStoppedInterruptObservation,
        bluetooth::BluetoothModemLpTimerStoppedInterruptFault,
    > {
        use bluetooth::{
            BluetoothModemLpTimerStoppedInterruptFault as Fault,
            BluetoothModemLpTimerStoppedInterruptObservation as Ack,
        };
        let mut model = timer.0.lock().unwrap();
        model.ops.push(Op::StoppedTimerStep);
        match model.timer.pop_front().unwrap_or(Step::Spurious) {
            Step::Spurious | Step::Rearmed => Ok(Ack::Quiet),
            Step::Rollover => Ok(Ack::RolloverAcknowledged(Default::default())),
            Step::Pending => Err(Fault::SoftwarePending {
                status: model.lp_status,
                pending: Default::default(),
            }),
            Step::Unknown => Err(Fault::UnexpectedStatus {
                status: u32::MAX,
                pending: Default::default(),
            }),
        }
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
    fault: &'static OnceValue<GateFault>,
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
        let fault = Box::leak(Box::new(OnceValue::new()));
        let (ends, part) = storage.split(
            (),
            model.clone(),
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
        let _ = self.part.service(&self.clock);
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
        let _ = self.part.service(&self.clock);
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
    // No timer owner is held before Open; output preparation then starts it.
    assert_eq!(
        harness.ops(),
        [
            Op::Acknowledge,
            Op::PrepareOutput,
            Op::StartTimer,
            Op::ChannelRequest,
            Op::Primary,
            Op::Nrt,
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
    assert_eq!(&ops[..3], &[Op::Acknowledge, Op::ApplyPlan, Op::TimerStep]);
    assert!(!harness.fault.get().is_some());
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
        Ok(Reply::Refused(GateError::TimerDispatchPending))
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
    assert_eq!(
        &harness.ops()[..2],
        &[Op::Acknowledge, Op::StoppedTimerStep]
    );
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
    assert!(harness.fault.get().is_some());
    assert!(harness.port.take_work().is_none());
    assert_eq!(
        harness.fault.get().map(|fault| invariant(fault).0),
        Some(GateFaultReason::StoppedTimer(
            bluetooth::BluetoothModemLpTimerStoppedInterruptFault::SoftwarePending {
                status: 0,
                pending: Default::default()
            }
        ))
    );
    let first = harness.fault.get().unwrap();
    harness.ops();
    assert_eq!(harness.part.service(&harness.clock), Err(first));
    assert!(harness.ops().is_empty());
}

#[test]
fn a_plan_without_outstanding_work_is_retained_as_a_fault() {
    let mut harness = Harness::new();
    harness.open();
    assert!(harness.port.submit_plan(Plan).is_ok());
    harness.enter();
    assert!(harness.fault.get().is_some());
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
        model.clone(),
        Line(model),
        BluetoothInterruptSignals {
            reply: host::foreign_signal(),
            cells,
            fault: Box::leak(Box::new(OnceValue::new())),
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
    assert!(!harness.fault.get().is_some());
}

#[test]
fn stopped_rollover_is_acknowledged_and_the_owner_can_reunite() {
    let mut harness = Harness::new();
    harness.open();
    assert!(matches!(
        harness.exchange(Request::StopTimer),
        Ok(Reply::TimerStopped(_))
    ));
    harness
        .clock
        .advance(oer_time::Duration::from_micros(10))
        .unwrap();
    harness.script(|m| m.timer.push_back(Step::Rollover));
    harness.enter();
    assert!(!harness.fault.get().is_some());
    assert!(harness.port.take_work().is_none());
    assert!(matches!(
        harness.exchange(Request::TakeTimer),
        Ok(Reply::Timer(_))
    ));
}

#[test]
fn stop_accepts_rollover_only_planning_after_the_task_drained() {
    let mut harness = Harness::new();
    harness.open();
    harness
        .clock
        .advance(oer_time::Duration::from_micros(10))
        .unwrap();
    harness.script(|m| m.timer.push_back(Step::Rollover));
    harness.enter();
    assert!(matches!(harness.part.timer, super::Timer::AwaitingPlan(_)));
    // No plan is returned by the already-drained task.
    assert!(matches!(
        harness.exchange(Request::StopTimer),
        Ok(Reply::TimerStopped(_))
    ));
    // Retirement discards only the late rollover snapshot/epoch projection.
    let _rollover = harness.port.take_work().unwrap();
    assert!(!harness.fault.get().is_some());
    assert!(matches!(
        harness.exchange(Request::TakeTimer),
        Ok(Reply::Timer(_))
    ));
    assert!(harness.port.take_work().is_none());
}

#[test]
fn a_dispatch_retained_during_rollover_planning_refuses_stop() {
    let mut harness = Harness::new();
    harness.open();
    harness.script(|m| m.timer.push_back(Step::Rollover));
    harness.enter();
    let _rollover = harness.port.take_work().unwrap();
    harness.script(|m| m.timer.push_back(Step::Pending));
    harness.enter();
    harness.ops();
    assert!(matches!(
        harness.exchange(Request::StopTimer),
        Ok(Reply::Refused(GateError::TimerDispatchPending))
    ));
    assert!(!harness.ops().contains(&Op::StopTimer));
    assert!(matches!(harness.part.timer, super::Timer::AwaitingPlan(_)));
    assert!(harness.port.submit_plan(Plan).is_ok());
    harness.enter();
    assert!(
        harness.port.take_work().is_some(),
        "dispatch remains deliverable"
    );
}

#[test]
fn stopped_unknown_status_uses_the_existing_fault_path() {
    let mut harness = Harness::new();
    harness.open();
    assert!(matches!(
        harness.exchange(Request::StopTimer),
        Ok(Reply::TimerStopped(_))
    ));
    harness.script(|m| m.timer.push_back(Step::Unknown));
    harness.enter();
    assert!(harness.fault.get().is_some());
    assert!(harness.part.retained.stopped.is_some());
    assert_eq!(
        harness.fault.get().map(|fault| invariant(fault).0),
        Some(GateFaultReason::StoppedTimer(
            bluetooth::BluetoothModemLpTimerStoppedInterruptFault::UnexpectedStatus {
                status: u32::MAX,
                pending: Default::default()
            }
        ))
    );
    harness.enter();
    assert_eq!(
        harness.fault.get().map(|fault| invariant(fault).0),
        Some(GateFaultReason::StoppedTimer(
            bluetooth::BluetoothModemLpTimerStoppedInterruptFault::UnexpectedStatus {
                status: u32::MAX,
                pending: Default::default()
            }
        ))
    );
    let first = harness.fault.get().unwrap();
    harness.ops();
    assert_eq!(harness.part.service(&harness.clock), Err(first));
    assert!(harness.ops().is_empty());
}

#[test]
fn retirement_discards_a_same_entry_rollover_snapshot_only_after_stop() {
    let mut harness = Harness::new();
    harness.open();
    harness
        .clock
        .advance(oer_time::Duration::from_micros(10))
        .unwrap();
    harness.script(|m| m.timer.push_back(Step::Rollover));
    let mut cx = Context::from_waker(Waker::noop());
    {
        let mut retiring = pin!(stop_timer_for_retirement(&mut harness.gate, &harness.port));
        assert!(retiring.as_mut().poll(&mut cx).is_pending());
        harness.part.service(&harness.clock);
        assert!(matches!(harness.part.timer, super::Timer::Stopped(_)));
        assert_eq!(
            harness.port.pending.len(),
            1,
            "late snapshot is still queued before retire resumes"
        );
        assert!(matches!(
            retiring.as_mut().poll(&mut cx),
            Poll::Ready(Ok(Compare))
        ));
    }
    assert!(
        harness.port.take_work().is_none(),
        "retire drained the late rollover projection"
    );
    assert!(!harness.fault.get().is_some());
}

#[test]
fn retirement_refuses_a_same_entry_dispatch_without_draining_its_work() {
    let mut harness = Harness::new();
    harness.open();
    harness.ops();
    harness.script(|m| m.timer.push_back(Step::Pending));
    let mut cx = Context::from_waker(Waker::noop());
    {
        let mut retiring = pin!(stop_timer_for_retirement(&mut harness.gate, &harness.port));
        assert!(retiring.as_mut().poll(&mut cx).is_pending());
        harness.part.service(&harness.clock);
        assert!(matches!(
            retiring.as_mut().poll(&mut cx),
            Poll::Ready(Err(GateError::TimerDispatchPending))
        ));
    }
    assert!(matches!(harness.part.timer, super::Timer::AwaitingPlan(_)));
    assert!(
        harness.port.take_work().is_some(),
        "dispatch remains available to its worker"
    );
    assert!(!harness.ops().contains(&Op::StopTimer));
    assert!(!harness.fault.get().is_some());
}

#[test]
fn vacant_timer_with_lp_cause_faults_without_acknowledging_or_returning_owners() {
    let mut harness = Harness::new();
    harness.script(|model| model.lp_status = 0x38);
    let expected = GateFault::Invariant {
        reason: GateFaultReason::VacantTimer,
        observation: GateObservation {
            lp_status: 0x38,
            nrt_status: None,
            timer: GateTimerState::Vacant,
        },
    };
    assert_eq!(harness.part.service(&harness.clock), Err(expected));
    assert_eq!(harness.ops(), [Op::Acknowledge]);
    harness.script(|model| model.lp_status = 0);
    assert_eq!(harness.part.service(&harness.clock), Err(expected));
    assert!(harness.ops().is_empty());
}

#[test]
fn fault_preserves_raw_lp_and_both_nrt_causes_and_the_timer_phase() {
    let mut harness = Harness::new();
    harness.open();
    harness.ops();
    harness.script(|model| {
        model.lp_status = 0x40002;
        model.nrt_status = [0x80001234, 0xfedcba98];
    });
    assert!(harness.port.submit_plan(Plan).is_ok());
    let fault = harness.part.service(&harness.clock).unwrap_err();
    assert_eq!(invariant(fault).0, GateFaultReason::PlanWithoutPendingTimer);
    assert_eq!(
        invariant(fault).1,
        GateObservation {
            lp_status: 0x40002,
            nrt_status: Some([0x80001234, 0xfedcba98]),
            timer: GateTimerState::Ready,
        }
    );
    assert!(matches!(harness.part.timer, super::Timer::Ready(_)));
    assert!(harness.part.is_open());
    assert!(harness.part.retained.plan.is_some());
    assert_eq!(harness.ops(), [Op::Acknowledge]);
}

#[test]
fn a_full_work_slot_retains_its_work_and_timer_without_publishing_or_servicing_the_bank() {
    let mut harness = Harness::new();
    harness.open();
    harness.ops();
    assert!(harness.part.pending.try_send(Work).is_ok());
    harness.script(|model| model.timer.push_back(Step::Pending));
    let fault = harness.part.service(&harness.clock).unwrap_err();
    assert_eq!(invariant(fault).0, GateFaultReason::WorkOccupied);
    assert!(harness.part.retained.work.is_some());
    assert!(matches!(harness.part.timer, super::Timer::AwaitingPlan(_)));
    assert_eq!(harness.ops(), [Op::Acknowledge, Op::TimerStep]);
    assert_eq!(is_pending(harness.cells), (false, false));
}

#[test]
fn a_full_reply_slot_retains_its_reply_and_stopped_timer() {
    let mut harness = Harness::new();
    harness.open();
    harness.ops();
    assert!(
        harness
            .part
            .replies
            .try_send(Reply::Refused(GateError::NotOpen))
            .is_ok()
    );
    assert!(harness.gate.requests.try_send(Request::StopTimer).is_ok());
    let fault = harness.part.service(&harness.clock).unwrap_err();
    assert_eq!(invariant(fault).0, GateFaultReason::ReplyOccupied);
    assert!(matches!(
        harness.part.retained.reply,
        Some(Reply::TimerStopped(_))
    ));
    assert!(matches!(harness.part.timer, super::Timer::Stopped(_)));
    assert_eq!(
        harness.ops(),
        [Op::Acknowledge, Op::TimerStep, Op::StopTimer]
    );
}

#[test]
fn primary_overflow_is_counted_before_the_measured_empty_nrt_snapshot() {
    let mut harness = Harness::new();
    harness.open();
    harness.script(|model| {
        model.ops.clear();
        model
            .primary
            .push_back(PrimaryInterruptCapture::CONTROLLER_TIME_OVERFLOW);
        // SOURCE: HW12_S31_BLE_OVERFLOW_ACK_20261026: NRT=0/0 and 133 low
        // while the overflow appears in primary. NRT acknowledges no overflow.
        model.nrt.push_back(false);
    });
    harness.enter();
    assert_eq!(
        harness.cells.scheduler_wake().controller_time_overflows().0,
        1
    );
    let ops = harness.ops();
    assert!(
        ops.iter().position(|op| *op == Op::Primary) < ops.iter().position(|op| *op == Op::Nrt)
    );
    let batch = harness
        .cells
        .scheduler_wake()
        .take()
        .unwrap()
        .unwrap()
        .classify()
        .unwrap();
    assert!(!batch.has_scheduler_work());
}

#[test]
fn overflow_counter_exhaustion_quarantines_the_gate_with_the_original_observation() {
    let mut harness = Harness::new();
    harness.open();
    harness
        .cells
        .scheduler_wake()
        .exhaust_controller_time_for_test();
    harness.script(|model| {
        model.ops.clear();
        model
            .primary
            .push_back(PrimaryInterruptCapture::CONTROLLER_TIME_OVERFLOW);
    });
    let fault = harness.part.service(&harness.clock).unwrap_err();
    assert_eq!(
        invariant(fault).0,
        GateFaultReason::ControllerTimeOverflow(super::super::ControllerTimeOverflowExhausted {
            overflows: u32::MAX,
            captured_inputs: PrimaryInterruptCapture::CONTROLLER_TIME_OVERFLOW,
        })
    );
    assert_eq!(invariant(fault).1.timer, GateTimerState::Ready);
    assert_eq!(invariant(fault).1.nrt_status, Some([0, 0]));
    let ops = harness.ops();
    assert!(!ops.contains(&Op::Nrt));
    assert!(
        harness.part.is_open(),
        "the bank remains quarantined in its handler"
    );
    assert_eq!(harness.part.service(&harness.clock), Err(fault));
    assert_eq!(
        harness.ops(),
        [],
        "absorbing fault performs no further register access"
    );
}

#[test]
fn the_latch_barrier_counts_hardware_pending_before_p2_chooses_the_samples_epoch() {
    use crate::controller_time::{ControllerSchedulerEpoch, ControllerTimeSample};
    use bluetooth::BluetoothControllerHalInitConfig;
    let mut harness = Harness::new();
    harness.open();
    let wake = harness.cells.scheduler_wake();
    let before = wake.controller_time_overflows();
    // Hardware has wrapped and completed the latch, but P3 has not entered yet.
    let sample = ControllerTimeSample::for_host_test(0);
    assert_eq!(wake.controller_time_overflows(), before);
    harness.script(|model| {
        model.ops.clear();
        model.channel.push_back(true); // the fresh barrier request
        model
            .primary
            .push_back(PrimaryInterruptCapture::CONTROLLER_TIME_OVERFLOW);
    });
    harness.enter();
    let ops = harness.ops();
    assert!(
        ops.iter().position(|op| *op == Op::ChannelRequest)
            < ops.iter().position(|op| *op == Op::Primary)
    );
    let after = wake.controller_time_overflows();
    let scale = BluetoothControllerHalInitConfig::reviewed_standalone().controller_time_scale();
    assert_eq!(
        ControllerSchedulerEpoch::from_counted_live_update(&sample, before, after, scale),
        Ok(None),
        "a latch completed before the IRQ entry must be retried, not assigned the old epoch"
    );
    assert_eq!(
        ControllerSchedulerEpoch::from_counted_live_update(&sample, after, after, scale)
            .unwrap()
            .unwrap()
            .1,
        1_u64 << 31
    );
}

#[test]
fn unexpected_nrt_overflow_quarantines_the_bank_and_retains_the_exact_later_pair() {
    let mut harness = Harness::new();
    harness.open();
    let cause = bluetooth::BluetoothNrtInterruptFault {
        snapshots: [0x87654321, 0xa5000002],
    };
    harness.script(|model| {
        model.ops.clear();
        model.nrt_fault = Some(cause);
    });
    let fault = harness.part.service(&harness.clock).unwrap_err();
    assert_eq!(
        invariant(fault).0,
        GateFaultReason::NrtControllerTimeOverflow(cause)
    );
    assert_eq!(
        invariant(fault).1.nrt_status,
        Some([0, 0]),
        "entry observation must not replace the later exact sample"
    );
    assert!(
        harness.part.is_open(),
        "the bank remains quarantined with its handler"
    );
    let ops = harness.ops();
    assert!(ops.contains(&Op::Nrt));
    assert_eq!(harness.part.service(&harness.clock), Err(fault));
    assert!(
        harness.ops().is_empty(),
        "a retained fault permits no further hardware accesses"
    );
}

#[test]
fn an_admission_policy_fault_is_absorbing_before_any_handler_register_access() {
    use bluetooth::{
        BluetoothInterruptObservation, BluetoothInterruptPolicyFault,
        BluetoothInterruptPolicyObservation, BluetoothTaskInterruptObservation,
    };
    let mut harness = Harness::new();
    let observation = BluetoothInterruptPolicyObservation {
        primary: BluetoothInterruptObservation {
            enable_0: 0x01000000,
            enable_1: 0x5,
            status_0: 0x12345678,
            status_1: 0x87654321,
            nrt_0: 0xdeadbeef,
            nrt_1: 0xbadcaffe,
        },
        task: BluetoothTaskInterruptObservation {
            enable: 0x20000,
            opaque_state: 0x12345678,
        },
    };
    let cause = observation.admit().unwrap_err();
    assert_eq!(
        cause,
        BluetoothInterruptPolicyFault::ForeignEnable(observation)
    );
    let fault = GateFault::InterruptPolicy(cause);
    assert!(harness.fault.set(fault).is_ok());
    assert_eq!(harness.part.service(&harness.clock), Err(fault));
    assert_eq!(harness.part.service(&harness.clock), Err(fault));
    assert!(harness.ops().is_empty());
    assert!(!harness.part.is_open());
}

#[test]
fn a_release_policy_fault_keeps_the_open_bank_and_stops_primary_and_nrt_service() {
    let mut harness = Harness::new();
    harness.open();
    harness.ops();
    let cause = bluetooth::BluetoothInterruptPolicyFault::ReleaseNotQuiet(
        bluetooth::BluetoothInterruptPolicyObservation {
            primary: bluetooth::BluetoothInterruptObservation {
                enable_0: 0,
                enable_1: 0,
                status_0: 0,
                status_1: 0x10,
                nrt_0: 0,
                nrt_1: 0,
            },
            task: bluetooth::BluetoothTaskInterruptObservation {
                enable: 0,
                opaque_state: 0,
            },
        },
    );
    harness.script(|model| model.policy_fault = Some(cause));
    assert_eq!(
        harness.part.service(&harness.clock),
        Err(GateFault::InterruptPolicy(cause))
    );
    assert_eq!(
        harness.ops(),
        [Op::Acknowledge, Op::TimerStep, Op::ChannelRequest]
    );
    assert!(harness.part.is_open());
    assert_eq!(
        harness.part.service(&harness.clock),
        Err(GateFault::InterruptPolicy(cause))
    );
    assert!(harness.ops().is_empty());
}
