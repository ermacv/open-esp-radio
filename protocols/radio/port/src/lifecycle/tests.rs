use super::*;
use core::{
    cell::{Cell, RefCell},
    future::pending,
};
use std::{
    boxed::Box,
    collections::VecDeque,
    rc::Rc,
    sync::{Arc, atomic::AtomicUsize},
    task::Wake,
    vec::Vec,
};

#[derive(Clone, Copy, Debug, PartialEq)]
enum Fault {
    Hardware,
    SinkRejected,
}
#[derive(Clone, Copy)]
enum Mode {
    Normal,
    FaultBeforeEnable,
    FaultAfterEnable,
    MissBound,
    StallRetire,
    StallEnable,
}
struct Model {
    mode: Mode,
    stopped: Cell<bool>,
    abandons: Cell<usize>,
    owner_drops: [Cell<usize>; 3],
    loan_ends: Cell<usize>,
    enabled: Cell<bool>,
    configured: Cell<usize>,
    poison: RefCell<PoisonState<Fault>>,
    log: RefCell<Vec<&'static str>>,
    event: Cell<bool>,
    irq_masked: Cell<bool>,
    irq_state: Cell<usize>,
    terminals: RefCell<VecDeque<Terminal>>,
}
impl Model {
    fn new(mode: Mode) -> Self {
        Self {
            mode,
            stopped: Cell::new(true),
            abandons: Cell::new(0),
            owner_drops: core::array::from_fn(|_| Cell::new(0)),
            loan_ends: Cell::new(0),
            enabled: Cell::new(false),
            configured: Cell::new(0),
            poison: RefCell::new(PoisonState::new()),
            log: RefCell::new(Vec::new()),
            event: Cell::new(false),
            irq_masked: Cell::new(false),
            irq_state: Cell::new(0),
            terminals: RefCell::new(VecDeque::new()),
        }
    }
    fn installed(&self) -> Installed<Backend<'_>> {
        Installed::new(Backend(self), Driver(self))
    }
    fn interrupt(&self) {
        if !self.irq_masked.get() {
            self.irq_state.set(self.irq_state.get() + 1);
        }
    }
    fn record(&self, event: &'static str) {
        self.log.borrow_mut().push(event);
    }
}
struct Backend<'a>(&'a Model);
struct Driver<'a>(&'a Model);
struct Completion<'a>(&'a Model);
struct Terminal {
    index: usize,
    drops: Rc<[Cell<usize>; 3]>,
}
impl Drop for Terminal {
    fn drop(&mut self) {
        self.drops[self.index].set(self.drops[self.index].get() + 1);
    }
}
impl Drop for Backend<'_> {
    fn drop(&mut self) {
        self.0.owner_drops[0].set(self.0.owner_drops[0].get() + 1);
    }
}
impl Drop for Driver<'_> {
    fn drop(&mut self) {
        self.0.owner_drops[1].set(self.0.owner_drops[1].get() + 1);
    }
}
impl Drop for Completion<'_> {
    fn drop(&mut self) {
        self.0.owner_drops[2].set(self.0.owner_drops[2].get() + 1);
    }
}
struct DriverCall<'a>(&'a Model);
impl Drop for DriverCall<'_> {
    fn drop(&mut self) {
        self.0.loan_ends.set(self.0.loan_ends.get() + 1);
    }
}
struct Phase<'a>(&'a Model);
struct Setup<'p> {
    held: &'p usize,
    local: &'p mut usize,
}
impl PortPhase for Phase<'_> {
    type Event = ();
    type Fault = Fault;
    async fn next_event(&mut self) -> Result<Result<(), EventsLost>, EventEnd<Fault>> {
        poll_fn(|_| {
            if self.0.event.replace(false) {
                self.0.record("event");
                Poll::Ready(Ok(Ok(())))
            } else if let Some(poisoned) = self.0.poison.borrow().get() {
                Poll::Ready(Err(EventEnd::Drained {
                    cause: poisoned.cause,
                }))
            } else {
                Poll::Pending
            }
        })
        .await
    }
}
impl<'a> PortBackend for Backend<'a> {
    type Pools = ();
    type Event<'id> = ();
    type Terminal<'id> = Terminal;
    type Domain = ();
    type Fault = Fault;
    type Refusal = ();
    type Setup<'p, 'id>
        = Setup<'p>
    where
        Self: 'p,
        'id: 'p;
    type Phase<'p, 'id>
        = Phase<'p>
    where
        Self: 'p,
        'id: 'p;
    type Resources<'id> = ();
    type Held<'id> = usize;
    type Local<'id> = usize;
    type Driver = Driver<'a>;
    type State = Completion<'a>;
    type Uninstall = ();
    type Uninstalled = usize;
    async fn enable<'p, 'id>(
        &'p mut self,
        _: Call,
        held: &'p Self::Held<'id>,
        local: &'p mut Self::Local<'id>,
    ) -> EnableResult<(), Fault>
    where
        'id: 'p,
    {
        if matches!(self.0.mode, Mode::StallEnable) {
            pending::<()>().await;
        }
        if matches!(self.0.mode, Mode::FaultBeforeEnable) {
            poll_fn(|_| {
                if self.0.poison.borrow().get().is_some() {
                    Poll::Ready(())
                } else {
                    Poll::Pending
                }
            })
            .await;
        }
        if let Some(poisoned) = self.0.poison.borrow().get() {
            return Err(poisoned);
        }
        self.0.configured.set(*held + *local);
        self.0.record("enable");
        self.0.enabled.set(true);
        Ok(Ok(()))
    }
    async fn disable<'p, 'id>(
        &'p mut self,
        _: Call,
        _: &'p usize,
        _: &'p mut usize,
        sink: &'p mut dyn FnMut(Terminal) -> core::ops::ControlFlow<Terminal>,
    ) -> Result<(), Poisoned<Fault>>
    where
        'id: 'p,
    {
        self.0.record("disable");
        self.0.enabled.set(false);
        for terminal in core::iter::from_fn(|| self.0.terminals.borrow_mut().pop_front()) {
            if let core::ops::ControlFlow::Break(terminal) = sink(terminal) {
                self.0.terminals.borrow_mut().push_front(terminal);
                let (poisoned, _publication) =
                    self.0.poison.borrow_mut().publish(Fault::SinkRejected);
                return Err(poisoned);
            }
        }
        self.0.poison.borrow().get().map_or(Ok(()), Err)
    }
    async fn now(&self, _: Call) -> PortResult<RadioInstant<()>, ClockError, Fault> {
        Ok(Ok(RadioInstant::from_micros(0)))
    }
    fn resources<'id>(&mut self, mint: Mint<'id, ()>) -> InstallParts<'id, Self> {
        mint.into_pools();
        InstallParts {
            held: 11,
            local: 3,
            resources: (),
        }
    }
    fn setup<'p, 'id>(
        &'p mut self,
        _: Call,
        held: &'p Self::Held<'id>,
        local: &'p mut Self::Local<'id>,
    ) -> Self::Setup<'p, 'id>
    where
        'id: 'p,
    {
        Setup { held, local }
    }
    fn phase<'p, 'id>(&'p mut self, _: Call, _: &'p usize, _: &'p mut usize) -> Phase<'p>
    where
        'id: 'p,
    {
        Phase(self.0)
    }
    async fn drive<'p, 'id>(
        driver: DriverLoan<'p, Driver<'a>>,
        _: &'p usize,
        retire: Retire<'p>,
    ) -> DriverExit<Self::State, Fault> {
        let model = driver.0;
        let _call = DriverCall(model);
        model.record("drive");
        if matches!(model.mode, Mode::FaultBeforeEnable | Mode::FaultAfterEnable) {
            let (cause, poisoned) = model.poison.borrow_mut().publish(Fault::Hardware);
            model.event.set(true);
            model.record("poison");
            return DriverExit::Faulted {
                state: Completion(model),
                cause: cause.cause,
                poisoned: driver.fail_stop(poisoned),
            };
        }
        if matches!(model.mode, Mode::MissBound | Mode::StallRetire) {
            pending::<()>().await;
        }
        let proof = retire.requested(driver).await;
        model.record("retire");
        DriverExit::Retired {
            state: Completion(model),
            proof,
        }
    }

    fn retire_bound(&self, _: Call) -> impl Future<Output = ()> + use<'a> {
        let mode = self.0.mode;
        async move {
            if !matches!(mode, Mode::MissBound) {
                pending::<()>().await;
            }
        }
    }
    fn detach<'id>(&mut self, _: crate::Detach<'_>, _: &usize) {
        self.0.record("detach");
    }
    fn uninstall<'id>(
        self,
        _: usize,
        _: usize,
        retired: Retired<'id, (Self::Driver, Self::State)>,
        _: Detached<'id>,
        _: (),
    ) -> usize {
        self.0.record("uninstall");
        let (driver, state) = retired.into_state();
        assert!(core::ptr::eq(driver.0, self.0));
        assert!(core::ptr::eq(state.0, self.0));
        7
    }
    fn abandon(&mut self, call: crate::Quarantine) -> crate::Abandon {
        self.0.abandons.set(self.0.abandons.get() + 1);
        self.0.irq_masked.set(true);
        self.0.record("abandon");
        if self.0.stopped.get() {
            crate::Abandon::Stopped(call.stopped())
        } else {
            crate::Abandon::Unconfirmed(call)
        }
    }
}

enum Body {
    Uninstall,
    FailStop,
    Phase,
    Drop,
    Forget,
    Foreign,
    CancelEnable,
}
impl<'a> PortBody<Backend<'a>> for Body {
    type Output = ();

    async fn run<'p, 'id>(
        self,
        owner: Owner<'p, 'id, Backend<'a>, Disabled>,
        _: (),
    ) -> Ended<'id, Backend<'a>, ()> {
        match self {
            Self::Uninstall => crate::Ended::uninstall(owner, (), ()),
            Self::FailStop => crate::Ended::fail_stopped(owner.fail_stop(), ()),
            Self::Drop => {
                drop(owner);
                pending().await
            }
            Self::Forget => {
                core::mem::forget(owner);
                pending().await
            }
            Self::Foreign => {
                pending::<()>().await;
                crate::Ended::uninstall(owner, (), ())
            }
            Self::CancelEnable => {
                let enabling = owner.enable();
                drop(enabling);
                pending().await
            }
            Self::Phase => {
                let mut enabled = match owner.enable().await {
                    Ok(enabled) => enabled,
                    Err(Failed::Poisoned(poisoned)) => {
                        return crate::Ended::uninstall(poisoned, (), ());
                    }
                    Err(failed) => return crate::Ended::fail_stopped(failed.fail_stop(), ()),
                };
                {
                    let mut phase = enabled.phase();
                    assert!(phase.next_event().await.is_ok());
                    assert!(phase.next_event().await.is_err());
                }
                match enabled
                    .disable(&mut |_| core::ops::ControlFlow::Continue(()))
                    .await
                {
                    Ok(disabled) => crate::Ended::uninstall(disabled, (), ()),
                    Err(poisoned) => crate::Ended::uninstall(poisoned, (), ()),
                }
            }
        }
    }
}
fn poll<F: Future>(future: Pin<&mut F>) -> Poll<F::Output> {
    future.poll(&mut Context::from_waker(core::task::Waker::noop()))
}
fn finish<F: Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    for _ in 0..8 {
        if let Poll::Ready(value) = poll(future.as_mut()) {
            return value;
        }
    }
    panic!("model did not finish");
}

struct Configure;
impl<'m> PortBody<Backend<'m>> for Configure {
    type Output = ();
    async fn run<'p, 'id>(
        self,
        mut owner: Owner<'p, 'id, Backend<'m>, Disabled>,
        _: (),
    ) -> Ended<'id, Backend<'m>, ()> {
        {
            let mut setup = owner.setup();
            *setup.local = *setup.held * 2;
        }
        let enabled = owner.enable().await.ok().expect("enable");
        let disabled = enabled
            .disable(&mut |_| core::ops::ControlFlow::Continue(()))
            .await
            .ok()
            .expect("disable");
        Ended::uninstall(disabled, (), ())
    }
}

#[test]
fn enable_observes_the_installation_state_borrowed_by_setup() {
    let model = Model::new(Mode::Normal);
    assert!(matches!(
        finish(run_port(model.installed(), Configure)),
        PortEnd::Uninstalled { .. }
    ));
    assert_eq!(model.configured.get(), 33);
}

struct RejectTerminal(usize);
impl<'m> PortBody<Backend<'m>> for RejectTerminal {
    type Output = Vec<Terminal>;
    async fn run<'p, 'id>(
        self,
        owner: Owner<'p, 'id, Backend<'m>, Disabled>,
        _: (),
    ) -> Ended<'id, Backend<'m>, Self::Output> {
        let owner = owner.enable().await.ok().expect("enable");
        let mut accepted = Vec::new();
        let mut offered = 0;
        let poisoned = owner
            .disable(&mut |terminal| {
                offered += 1;
                if terminal.index == self.0 {
                    core::ops::ControlFlow::Break(terminal)
                } else {
                    accepted.push(terminal);
                    core::ops::ControlFlow::Continue(())
                }
            })
            .await
            .err()
            .expect("rejection returns PoisonedOwner");
        assert_eq!(poisoned.cause(), Fault::SinkRejected);
        assert_eq!(offered, self.0 + 1, "delivery stops at the first rejection");
        Ended::uninstall(poisoned, (), accepted)
    }
}

#[test]
fn rejecting_first_or_middle_terminal_keeps_every_owner_without_drop() {
    for rejected in [0, 1] {
        let model = Model::new(Mode::Normal);
        let drops = Rc::new(core::array::from_fn(|_| Cell::new(0)));
        model
            .terminals
            .borrow_mut()
            .extend((0..3).map(|index| Terminal {
                index,
                drops: drops.clone(),
            }));
        let PortEnd::Uninstalled {
            output: accepted, ..
        } = finish(run_port(model.installed(), RejectTerminal(rejected)))
        else {
            panic!("the poisoned owner uninstalls");
        };
        assert_eq!(accepted.len(), rejected);
        assert_eq!(
            model
                .terminals
                .borrow()
                .iter()
                .map(|terminal| terminal.index)
                .collect::<Vec<_>>(),
            (rejected..3).collect::<Vec<_>>()
        );
        assert!(drops.iter().all(|count| count.get() == 0));
        // Only the sink's accepted owners can be destroyed by that sink.
        drop(accepted);
        for index in 0..3 {
            assert_eq!(drops[index].get(), usize::from(index < rejected));
        }
        drop(model);
        assert!(drops.iter().all(|count| count.get() == 1));
    }
}

#[test]
fn dropping_quarantine_neither_frees_backing_nor_touches_hardware() {
    struct Backing<'a>(&'a Cell<usize>);
    impl Drop for Backing<'_> {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }
    struct Hardware<'a> {
        _backing: Box<Backing<'a>>,
        writes: &'a Cell<usize>,
    }
    impl Drop for Hardware<'_> {
        fn drop(&mut self) {
            self.writes.set(self.writes.get() + 1);
        }
    }
    let freed = Cell::new(0);
    let writes = Cell::new(0);
    let owner = || Hardware {
        _backing: Box::new(Backing(&freed)),
        writes: &writes,
    };
    drop(Quarantined::new(owner()));
    assert_eq!((freed.get(), writes.get()), (0, 0));
    drop(owner());
    assert_eq!((freed.get(), writes.get()), (1, 1));
}

fn owner_drops(model: &Model) -> [usize; 3] {
    core::array::from_fn(|i| model.owner_drops[i].get())
}

#[test]
fn both_abnormal_ends_report_the_stop_outcome_and_retain_every_owner() {
    for stopped in [true, false] {
        for mode in [Mode::Normal, Mode::MissBound] {
            for fail_stop in [true, false] {
                let model = Model::new(mode);
                model.stopped.set(stopped);
                let body = if fail_stop {
                    Body::FailStop
                } else {
                    Body::Drop
                };
                let end = finish(run_port(model.installed(), body));
                let outcome = match &end {
                    PortEnd::FailStopped { outcome, .. } if fail_stop => outcome,
                    PortEnd::Abandoned { outcome, .. } if !fail_stop => outcome,
                    _ => panic!("wrong end"),
                };
                assert_eq!(matches!(outcome, Abandon::Stopped(_)), stopped);
                assert_eq!(owner_drops(&model), [0; 3]);
                assert_eq!(model.loan_ends.get(), 1);
                drop(end);
                assert_eq!(owner_drops(&model), [0; 3]);
            }
        }
    }
}

#[test]
fn ordinary_uninstall_returns_the_driver_and_its_completion_state() {
    let model = Model::new(Mode::Normal);
    assert!(matches!(
        finish(run_port(model.installed(), Body::Uninstall)),
        PortEnd::Uninstalled { uninstalled: 7, .. }
    ));
    assert_eq!(owner_drops(&model), [1; 3]);
}

#[test]
fn cancelling_before_first_poll_poisoned_and_retained_the_installation() {
    let model = Model::new(Mode::Normal);
    model.stopped.set(false);
    drop(run_port(model.installed(), Body::Foreign));
    assert_eq!(&*model.log.borrow(), &["abandon"]);
    assert_eq!(owner_drops(&model), [0; 3]);
}

#[test]
fn cancellation_retains_running_and_already_faulted_driver_owners() {
    for mode in [Mode::Normal, Mode::FaultBeforeEnable] {
        let model = Model::new(mode);
        model.stopped.set(false);
        let mut port = Box::pin(run_port(model.installed(), Body::Foreign));
        assert!(poll(port.as_mut()).is_pending());
        drop(port);
        assert_eq!(model.loan_ends.get(), 1);
        assert_eq!(owner_drops(&model), [0; 3]);
        assert_eq!(model.log.borrow().last(), Some(&"abandon"));
    }
}

#[test]
fn cancelling_abandon_retirement_keeps_the_synchronous_safety_net() {
    let model = Model::new(Mode::StallRetire);
    model.stopped.set(false);
    let mut port = Box::pin(run_port(model.installed(), Body::Drop));
    assert!(poll(port.as_mut()).is_pending());
    drop(port);
    assert_eq!(&*model.log.borrow(), &["abandon", "drive"]);
    assert_eq!(model.abandons.get(), 1);
    assert_eq!(owner_drops(&model), [0; 3]);
}
#[test]
fn uninstall_retires_detaches_and_returns_owners_in_order() {
    let m = Model::new(Mode::Normal);
    assert!(matches!(
        finish(run_port(m.installed(), Body::Uninstall)),
        PortEnd::Uninstalled {
            uninstalled: 7,
            output: ()
        }
    ));
    assert_eq!(
        &*m.log.borrow(),
        &["drive", "retire", "detach", "uninstall"]
    );
}
#[test]
fn fault_before_enable_retains_state_for_poisoned_uninstall() {
    let m = Model::new(Mode::FaultBeforeEnable);
    assert!(matches!(
        finish(run_port(m.installed(), Body::Phase)),
        PortEnd::Uninstalled {
            uninstalled: 7,
            output: ()
        }
    ));
    assert_eq!(
        &*m.log.borrow(),
        &["drive", "poison", "detach", "uninstall"]
    );
}
#[test]
fn fault_after_enable_lets_body_drain_before_uninstall() {
    let m = Model::new(Mode::FaultAfterEnable);
    assert!(matches!(
        finish(run_port(m.installed(), Body::Phase)),
        PortEnd::Uninstalled {
            uninstalled: 7,
            output: ()
        }
    ));
    assert_eq!(
        &*m.log.borrow(),
        &[
            "enable",
            "drive",
            "poison",
            "event",
            "disable",
            "detach",
            "uninstall"
        ]
    );
}
#[test]
fn missed_retire_bound_fail_stops_without_detach_or_uninstall() {
    let m = Model::new(Mode::MissBound);
    assert!(matches!(
        finish(run_port(m.installed(), Body::Uninstall)),
        PortEnd::FailStopped { output: (), .. }
    ));
    assert_eq!(&*m.log.borrow(), &["drive", "abandon"]);
}
#[test]
fn dropped_owner_and_cancelled_transition_retire_asynchronously() {
    for body in [Body::Drop, Body::CancelEnable] {
        let m = Model::new(Mode::Normal);
        assert!(matches!(
            finish(run_port(m.installed(), body)),
            PortEnd::Abandoned { .. }
        ));
        assert_eq!(&*m.log.borrow(), &["abandon", "drive", "retire", "detach"]);
    }
}
#[test]
fn driver_progresses_beside_foreign_wait_and_core_cancellation_abandons_once() {
    let m = Model::new(Mode::Normal);
    let mut port = Box::pin(run_port(m.installed(), Body::Foreign));
    assert!(poll(port.as_mut()).is_pending());
    assert_eq!(&*m.log.borrow(), &["drive"]);
    drop(port);
    assert_eq!(&*m.log.borrow(), &["drive", "abandon"]);
}
#[test]
fn forgotten_owner_stays_owned_by_core_until_cancellation() {
    let m = Model::new(Mode::Normal);
    let mut port = Box::pin(run_port(m.installed(), Body::Forget));
    assert!(poll(port.as_mut()).is_pending());
    drop(port);
    assert_eq!(&*m.log.borrow(), &["drive", "abandon"]);
}
struct Wakes(AtomicUsize);
impl Wake for Wakes {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}
#[test]
fn retire_request_wakes_the_registered_driver_without_a_poll() {
    let request = Signal::new();
    let latch = AbandonLatch::new();
    let wakes = Arc::new(Wakes(AtomicUsize::new(0)));
    let waker = wakes.clone().into();
    let mut driver = ();
    let mut future = pin!(
        Retire {
            request: &request,
            latch: &latch
        }
        .requested(DriverLoan {
            driver: &mut driver
        })
    );
    assert!(
        future
            .as_mut()
            .poll(&mut Context::from_waker(&waker))
            .is_pending()
    );
    request.set();
    assert_eq!(wakes.0.load(Ordering::Relaxed), 1);
    assert!(poll(future.as_mut()).is_ready());
}
#[test]
fn abandon_wakes_both_core_and_driver_and_stays_latched() {
    let latch = AbandonLatch::new();
    let core = Arc::new(Wakes(AtomicUsize::new(0)));
    let driver = Arc::new(Wakes(AtomicUsize::new(0)));
    let cw = core.clone().into();
    let dw = driver.clone().into();
    assert!(latch.core.poll(&mut Context::from_waker(&cw)).is_pending());
    let mut future = pin!(latch.abandoned());
    assert!(
        future
            .as_mut()
            .poll(&mut Context::from_waker(&dw))
            .is_pending()
    );
    latch.set();
    assert_eq!(core.0.load(Ordering::Relaxed), 1);
    assert_eq!(driver.0.load(Ordering::Relaxed), 1);
    assert!(poll(future.as_mut()).is_ready());
    assert!(latch.core.poll(&mut Context::from_waker(&cw)).is_ready());
}

#[test]
fn cancellation_while_transition_or_retirement_waits_abandons_once() {
    for (mode, body) in [
        (Mode::StallRetire, Body::Uninstall),
        (Mode::StallEnable, Body::Phase),
    ] {
        let model = Model::new(mode);
        let mut port = Box::pin(run_port(model.installed(), body));
        assert!(poll(port.as_mut()).is_pending());
        drop(port);
        assert_eq!(&*model.log.borrow(), &["drive", "abandon"]);
    }
}

struct StopThenWait<'a>(&'a Signal);
impl<'a, 'm> PortBody<Backend<'m>> for StopThenWait<'a> {
    type Output = &'static str;
    async fn run<'p, 'id>(
        self,
        owner: Owner<'p, 'id, Backend<'m>, Disabled>,
        _: (),
    ) -> Ended<'id, Backend<'m>, Self::Output> {
        let proof = owner.fail_stop();
        poll_fn(|cx| self.0.poll(cx)).await;
        Ended::fail_stopped(proof, "body result")
    }
}

#[test]
fn fail_stop_masks_irqs_and_retires_while_body_waits_then_detaches_once() {
    let model = Model::new(Mode::Normal);
    let returning = Signal::new();
    model.interrupt();
    let mut port = Box::pin(run_port(model.installed(), StopThenWait(&returning)));
    assert!(poll(port.as_mut()).is_pending());
    model.interrupt();
    assert_eq!(
        model.irq_state.get(),
        1,
        "an IRQ cannot reach quarantined state"
    );
    assert_eq!(&*model.log.borrow(), &["abandon", "drive", "retire"]);
    returning.set();
    assert!(matches!(
        poll(port.as_mut()),
        Poll::Ready(PortEnd::FailStopped {
            output: "body result",
            ..
        })
    ));
    assert_eq!(
        &*model.log.borrow(),
        &["abandon", "drive", "retire", "detach"]
    );
    drop(port);
    assert_eq!(
        model
            .log
            .borrow()
            .iter()
            .filter(|event| **event == "detach")
            .count(),
        1
    );
}

#[test]
fn fail_stop_then_cancellation_abandons_once_while_body_or_retire_waits() {
    for mode in [Mode::Normal, Mode::StallRetire, Mode::MissBound] {
        let model = Model::new(mode);
        model.stopped.set(false);
        let returning = Signal::new();
        let mut port = Box::pin(run_port(model.installed(), StopThenWait(&returning)));
        assert!(poll(port.as_mut()).is_pending());
        assert_eq!(model.abandons.get(), 1);
        drop(port);
        model.interrupt();
        assert_eq!(model.irq_state.get(), 0);
        assert_eq!(owner_drops(&model), [0; 3]);
        assert_eq!(model.abandons.get(), 1);
    }
}

#[test]
fn cancellation_after_fail_stopped_body_returns_only_quarantines() {
    let model = Model::new(Mode::StallRetire);
    model.stopped.set(false);
    let mut port = Box::pin(run_port(model.installed(), Body::FailStop));
    assert!(poll(port.as_mut()).is_pending());
    drop(port);
    assert_eq!(model.abandons.get(), 1);
    assert_eq!(owner_drops(&model), [0; 3]);
}

#[test]
fn uninstall_timeout_then_cancellation_abandons_once() {
    let model = Model::new(Mode::MissBound);
    model.stopped.set(false);
    let mut port = Box::pin(run_port(model.installed(), Body::Uninstall));
    let Poll::Ready(end @ PortEnd::FailStopped { .. }) = poll(port.as_mut()) else {
        panic!("uninstall times out and quarantines in the same poll");
    };
    assert_eq!(model.abandons.get(), 1);
    // There is no suspension between a timeout's abandon and the result.
    // The executor can drop the returned receipt and completed frame here.
    drop(end);
    drop(port);
    assert_eq!(model.abandons.get(), 1);
    assert_eq!(owner_drops(&model), [0; 3]);
}

#[test]
fn fail_stop_then_cancellation_preserves_the_first_latch_cause() {
    let model = Model::new(Mode::Normal);
    let install = Install::<Backend<'_>> {
        held: ManuallyDrop::new(11),
        latch: AbandonLatch::new(),
        retiring: Signal::new(),
    };
    let mut local = 3;
    let mut backend = guard(Backend(&model), |backend| drop(Quarantined::new(backend)));
    let mut armed = guard(&mut *backend, |backend| {
        abandon_on_cancel(backend, &install.latch)
    });
    let owner = Owner::<_, Disabled>::new(&mut **armed, &mut local, &install);
    let proof = owner.fail_stop();
    drop(armed);
    assert!(install.latch.is_fail_stopped());
    assert_eq!(
        install.latch.cause.load(Ordering::Acquire),
        AbandonLatch::FAIL_STOPPED
    );
    assert_eq!(model.abandons.get(), 1);
    drop(proof);
    drop(backend);
    assert_eq!(owner_drops(&model), [0; 3]);
}

#[test]
fn latch_cause_only_transitions_from_live_and_both_waiters_stay_set() {
    for fail_stop_first in [true, false] {
        let latch = AbandonLatch::new();
        if fail_stop_first {
            latch.fail_stopped();
            latch.set();
        } else {
            latch.set();
            latch.fail_stopped();
        }
        assert_eq!(latch.is_fail_stopped(), fail_stop_first);
        assert!(latch.core.is_set());
        assert!(latch.driver.is_set());
    }
}

#[test]
fn retire_timeout_does_not_wait_for_the_fail_stopped_body() {
    let model = Model::new(Mode::MissBound);
    let returning = Signal::new();
    let mut port = Box::pin(run_port(model.installed(), StopThenWait(&returning)));
    assert!(poll(port.as_mut()).is_pending());
    assert!(model.irq_masked.get());
    returning.set();
    assert!(matches!(
        poll(port.as_mut()),
        Poll::Ready(PortEnd::FailStopped {
            output: "body result",
            ..
        })
    ));
    assert_eq!(&*model.log.borrow(), &["abandon", "drive"]);
}

#[test]
fn forgetting_the_whole_port_cannot_return_owners_or_run_drop_cleanup() {
    // Hardware backing remains durable even if the executor leaks the frame.
    let model = Box::leak(Box::new(Model::new(Mode::Normal)));
    model.stopped.set(false);
    let mut port = Box::pin(run_port(model.installed(), Body::Foreign));
    assert!(poll(port.as_mut()).is_pending());
    core::mem::forget(port);
    assert_eq!(&*model.log.borrow(), &["drive"]);
    assert_eq!(owner_drops(model), [0; 3]);
    assert_eq!(model.loan_ends.get(), 0);
    model.interrupt();
    assert_eq!(
        model.irq_state.get(),
        1,
        "backing still exists for hardware"
    );
}
