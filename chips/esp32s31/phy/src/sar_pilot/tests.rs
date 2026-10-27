use super::*;
use crate::tx::sar::Io;
use oer_esp32s31_hal::{
    domain::transaction::{DetachedRadioDomain, Fault, FaultReason, Operation, TransactionSeat},
    owner::PhyRegistrationEpoch,
    root::RadioHardware,
};
use oer_espressif_interrupt_table::signal::host::{self, Deferred};
use oer_time::Instant;
use oer_time_virtual::VirtualClock;
use std::{
    boxed::Box,
    cell::{Cell, RefCell},
    panic::{AssertUnwindSafe, catch_unwind},
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    task::{Wake, Waker},
    vec::Vec,
};

// Only hardware effects are modeled. Custody, Client::sample, IRQ service,
// the real owned PHY guards, queues and IrqSignal are production code.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Edge {
    Disarm,
    Acknowledge(bool),
    Arm(u64),
    Tone,
    Trigger,
    Ready,
    ClearTone,
    Sample,
}
struct Hardware {
    clock: VirtualClock,
    pending: Cell<bool>,
    enabled: Cell<bool>,
    due: Cell<Option<Instant>>,
    expire_on_arm: Cell<bool>,
    cross_trigger_tick: Cell<bool>,
    cross_next_read: Cell<bool>,
    events: RefCell<Vec<(Edge, Instant)>>,
    before_arm: RefCell<Option<Box<dyn FnOnce()>>>,
}
impl Hardware {
    fn new() -> Rc<Self> {
        Rc::new(Self {
            clock: VirtualClock::new(),
            pending: Cell::new(true), // Startup must clear a stale RAW.
            enabled: Cell::new(true),
            due: Cell::new(None),
            expire_on_arm: Cell::new(false),
            cross_trigger_tick: Cell::new(false),
            cross_next_read: Cell::new(false),
            events: RefCell::new(Vec::new()),
            before_arm: RefCell::new(None),
        })
    }
    fn record(&self, edge: Edge) {
        self.events.borrow_mut().push((edge, self.clock.now()));
    }
    fn expire(&self) {
        let due = self.due.take().expect("an armed one-shot");
        self.clock
            .advance(due.saturating_duration_since(self.clock.now()))
            .unwrap();
        self.pending.set(true);
    }
    fn edges(&self) -> Vec<Edge> {
        self.events.borrow().iter().map(|(e, _)| *e).collect()
    }
}
impl Clock for Hardware {
    fn now(&self) -> Instant {
        let snapshot = self.clock.now();
        if self.cross_next_read.replace(false) {
            // The machine timestamps its trigger just before a us boundary.
            // Any subsequent driver clock read would see the next tick.
            self.clock.advance(Duration::from_micros(1)).unwrap();
        }
        snapshot
    }
}
struct ProgramModel(Rc<Hardware>);
impl Program<()> for ProgramModel {
    fn arm_after(&mut self, delay: Duration, _: &()) -> Result<(), InvalidDelay> {
        if let Some(check) = self.0.before_arm.borrow_mut().take() {
            check();
        }
        self.0.record(Edge::Arm(delay.as_micros()));
        self.0.enabled.set(true);
        self.0
            .due
            .set(Some(self.0.clock.now().checked_add(delay).unwrap()));
        // Like T0, a new expiry during programming latches RAW. Rearm does
        // not consume it. Same-level entry happens only after publication.
        if self.0.expire_on_arm.get() {
            self.0.expire();
        }
        Ok(())
    }
    fn disarm(&mut self) {
        self.0.record(Edge::Disarm);
        self.0.enabled.set(false);
        self.0.due.set(None);
    }
}
struct IrqModel(Rc<Hardware>);
impl Acknowledge for IrqModel {
    fn acknowledge(&mut self) -> Observation {
        let pending = self.0.pending.replace(false);
        self.0.record(Edge::Acknowledge(pending));
        Observation {
            pending,
            asserted: pending && self.0.enabled.get(),
        }
    }
}
struct SarModel(Rc<Hardware>);
impl Io for SarModel {
    fn arm_tone(&mut self) {
        self.0.record(Edge::Tone);
    }
    fn trigger(&mut self) {
        self.0.record(Edge::Trigger);
        self.0.cross_next_read.set(self.0.cross_trigger_tick.get());
    }
    fn ready(&mut self) -> bool {
        self.0.record(Edge::Ready);
        true
    }
    fn clear_tone(&mut self) {
        self.0.record(Edge::ClearTone);
    }
    fn sample(&mut self) -> u16 {
        self.0.record(Edge::Sample);
        73
    }
}
fn report(fault: Fault) -> ! {
    std::panic::panic_any(fault)
}
type Actor = Client<(), Deferred, ProgramModel>;
type Handler = Interrupt<(), Deferred, ProgramModel>;
struct Fixture {
    hardware: Rc<Hardware>,
    client: Actor,
    handler: Rc<RefCell<Handler>>,
    irq: IrqModel,
    signal: &'static IrqSignal<Deferred>,
}
impl Fixture {
    fn new() -> Self {
        let hardware = Hardware::new();
        let cells = Box::leak(Box::new(Cells::new()));
        let signal = host::deferred_signal();
        let mut irq = IrqModel(hardware.clone());
        let (client, handler) = cells.split_with(ProgramModel(hardware.clone()), &mut irq, signal);
        Self {
            hardware,
            client: client.into_client(()),
            handler: Rc::new(RefCell::new(handler)),
            irq,
            signal,
        }
    }
    fn service(&mut self) -> Result<(), SarAlarmWithoutBundle> {
        service(&self.handler, &mut self.irq, &self.hardware)
    }
}
fn service(
    handler: &RefCell<Handler>,
    irq: &mut IrqModel,
    hardware: &Rc<Hardware>,
) -> Result<(), SarAlarmWithoutBundle> {
    handler.borrow_mut().service_with(irq, |engine, _phy| {
        engine.advance(hardware.as_ref(), &mut SarModel(hardware.clone()))
    })
}
type Returned =
    DetachedRadioDomain<crate::test_clocks::TestClockBackend, crate::concurrent::ConcurrentPhy>;
fn transaction() -> (Returned, PhyTransaction, FaultHandle, PhyRegistrationEpoch) {
    let seat = Box::leak(Box::new(TransactionSeat::for_host(report)));
    let (mut radio, _) = RadioHardware::for_host_test()
        .split::<crate::test_clocks::TestClockBackend, _>(
            crate::concurrent::ConcurrentPhy::new(),
            seat.claim().unwrap(),
        );
    let epoch = radio.begin_registration_for_host_test().epoch();
    let diagnostic = radio.fault_handle();
    let (returned, phy) = radio.detach(Operation::Maintain).split();
    (returned, phy, diagnostic, epoch)
}
#[derive(Default)]
struct Wakes(AtomicUsize);
impl Wake for Wakes {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn real_client_and_interrupt_clear_before_step_arm_before_deposit_and_preserve_new_expiry() {
    let mut f = Fixture::new();
    assert_eq!(f.hardware.edges(), [Edge::Disarm, Edge::Acknowledge(true)]);
    assert!(!f.hardware.enabled.get() && !f.hardware.pending.get());
    let (returned, phy, _, _) = transaction();
    let handler = f.handler.clone();
    *f.hardware.before_arm.borrow_mut() = Some(Box::new(move || {
        let handler = handler.borrow();
        assert_eq!(handler.receive.len(), 0, "arm must precede deposit");
        assert!(handler.active.is_none());
    }));
    f.hardware.expire_on_arm.set(true);
    let wakes = Arc::new(Wakes::default());
    let waker = Waker::from(wakes.clone());
    let mut cx = Context::from_waker(&waker);
    let mut sample = Box::pin(f.client.sample(phy, true));
    assert!(sample.as_mut().poll(&mut cx).is_pending());
    assert_eq!(f.handler.borrow().receive.len(), 1);
    assert!(
        f.hardware.pending.get(),
        "expiry before deposit remains pending"
    );
    // An unrelated/stale signal may wake the actor before the reply exists.
    f.signal.raise(2);
    f.signal.service();
    assert!(sample.as_mut().poll(&mut cx).is_pending());
    let before_reply = wakes.0.load(Ordering::Relaxed);
    for _ in 0..3 {
        service(&f.handler, &mut f.irq, &f.hardware).unwrap();
    }
    assert_eq!(
        f.handler.borrow().send.len(),
        1,
        "reply published before signal service"
    );
    assert!(f.handler.borrow().active.is_none());
    assert!(!f.hardware.enabled.get() && f.hardware.due.get().is_none());
    assert!(f.signal.has_pending().unwrap());
    assert_eq!(
        wakes.0.load(Ordering::Relaxed),
        before_reply,
        "handler calls no task waker"
    );
    f.signal.service();
    assert_eq!(wakes.0.load(Ordering::Relaxed), before_reply + 1);
    let Poll::Ready((phy, result)) = sample.as_mut().poll(&mut cx) else {
        panic!("reply ready")
    };
    assert_eq!(result.unwrap().value, 73);
    drop(sample);
    assert!(f.client.alarm.is_some());
    assert_eq!(f.client.receive.len(), 0);
    assert!(returned.reunite(phy).is_ok());
    assert_eq!(
        f.hardware.edges(),
        [
            Edge::Disarm,
            Edge::Acknowledge(true),
            Edge::Arm(1),
            Edge::Acknowledge(true),
            Edge::Tone,
            Edge::Arm(1),
            Edge::Acknowledge(true),
            Edge::Trigger,
            Edge::Arm(2),
            Edge::Acknowledge(true),
            Edge::Ready,
            Edge::ClearTone,
            Edge::Sample,
            Edge::Disarm,
        ]
    );
    // No pending event after terminal disarm: an entry with RAW=0 is inert.
    f.service().unwrap();
    assert_eq!(f.hardware.edges().last(), Some(&Edge::Acknowledge(false)));
}

#[test]
fn trigger_crossing_a_microsecond_boundary_still_arms_two_full_microseconds() {
    let mut f = Fixture::new();
    f.hardware.cross_trigger_tick.set(true);
    let (returned, phy, _, _) = transaction();
    let mut cx = Context::from_waker(Waker::noop());
    let mut sample = Box::pin(f.client.sample(phy, false));
    assert!(sample.as_mut().poll(&mut cx).is_pending());
    for _ in 0..2 {
        f.hardware.expire();
        service(&f.handler, &mut f.irq, &f.hardware).unwrap();
    }
    let events = f.hardware.events.borrow();
    let trigger = events.iter().find(|(e, _)| *e == Edge::Trigger).unwrap().1;
    let (interval, armed) = *events.last().unwrap();
    assert_eq!(
        armed,
        trigger.checked_add(Duration::from_micros(1)).unwrap()
    );
    assert_eq!(interval, Edge::Arm(2), "do not turn at-now into a 1-us arm");
    drop(events);
    f.hardware.expire();
    service(&f.handler, &mut f.irq, &f.hardware).unwrap();
    let ready = f
        .hardware
        .events
        .borrow()
        .iter()
        .find(|(e, _)| *e == Edge::Ready)
        .unwrap()
        .1;
    assert!(ready.saturating_duration_since(trigger) >= Duration::from_micros(2));
    let Poll::Ready((phy, result)) = sample.as_mut().poll(&mut cx) else {
        panic!("ready")
    };
    assert!(result.is_ok());
    drop(sample);
    assert!(returned.reunite(phy).is_ok());
}

#[test]
fn cancellation_of_the_real_waiting_future_quarantines_bundle_at_every_custody_location() {
    for services in [0, 1, 3] {
        let mut f = Fixture::new();
        f.hardware.expire_on_arm.set(true);
        let (returned, phy, diagnostic, epoch) = transaction();
        let mut lifecycle = Box::pin(async {
            let (phy, result) = f.client.sample(phy, false).await;
            (returned.reunite(phy).ok().unwrap(), result)
        });
        assert!(
            lifecycle
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop()))
                .is_pending()
        );
        for _ in 0..services {
            service(&f.handler, &mut f.irq, &f.hardware).unwrap();
        }
        let writes = f.hardware.edges();
        let stopped = catch_unwind(AssertUnwindSafe(|| drop(lifecycle))).unwrap_err();
        let expected = Fault {
            reason: FaultReason::Cancelled(Operation::Maintain),
            registration: Some(epoch),
        };
        assert_eq!(*stopped.downcast::<Fault>().unwrap(), expected);
        assert_eq!(diagnostic.fault(), Some(expected));
        let handler = f.handler.borrow();
        assert_eq!(handler.receive.len(), usize::from(services == 0));
        assert_eq!(handler.active.is_some(), services == 1);
        assert_eq!(handler.send.len(), usize::from(services == 3));
        // Cancellation records/resets without recovering or dropping queued
        // authority. An in-flight alarm stays quarantined until chip reset;
        // a completed conversion already stopped/masked it before reply.
        assert_eq!(f.hardware.enabled.get(), services != 3);
        assert_eq!(
            f.hardware.edges(),
            writes,
            "no silent cleanup on cancellation"
        );
        assert!(
            f.client.alarm.is_none(),
            "program remains in the real bundle"
        );
    }
}

#[test]
fn pending_without_bundle_is_a_named_fault_with_source_observations() {
    for completed in [false, true] {
        let mut f = Fixture::new();
        let mut kept_return = None;
        if completed {
            let (returned, phy, _, _) = transaction();
            let mut cx = Context::from_waker(Waker::noop());
            let mut sample = Box::pin(f.client.sample(phy, false));
            assert!(sample.as_mut().poll(&mut cx).is_pending());
            for _ in 0..3 {
                f.hardware.expire();
                service(&f.handler, &mut f.irq, &f.hardware).unwrap();
            }
            // Keep the actual reply bundle quarantined in the reply cell.
            core::mem::drop(sample);
            kept_return = Some(returned);
        }
        assert!(!f.hardware.enabled.get());
        f.hardware.pending.set(true);
        let before = f.hardware.edges();
        assert_eq!(
            f.service(),
            Err(SarAlarmWithoutBundle {
                observation: Observation {
                    pending: true,
                    asserted: false,
                },
                requests: 0,
                replies: usize::from(completed),
            })
        );
        let mut expected_edges = before;
        expected_edges.push(Edge::Acknowledge(true));
        assert_eq!(f.hardware.edges(), expected_edges, "no recovery writes");
        assert!(f.handler.borrow().active.is_none());
        assert_eq!(f.handler.borrow().receive.len(), 0);
        assert_eq!(f.client.alarm.is_some(), !completed);
        assert!(!f.hardware.pending.get(), "acknowledgement precedes fault");
        assert_eq!(f.handler.borrow().send.len(), usize::from(completed));
        // The image resets with custody still in the reply cell. Reunite
        // only in this harness to avoid a second cancellation fault.
        if let Some(returned) = kept_return {
            let phy = f.client.receive.try_receive().ok().unwrap().phy;
            assert!(returned.reunite(phy).is_ok());
        }
    }
}
