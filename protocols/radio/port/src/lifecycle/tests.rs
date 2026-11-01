use crate::conformance::registration::{MockCleanup, MockRegistration, guarded};
use crate::*;
use core::{
    cell::Cell,
    future::{Future, pending, poll_fn},
    marker::PhantomData,
    pin::{Pin, pin},
    task::{Context, Poll, Waker},
};
use oer_time::{Clock, Duration, Instant, RadioInstant, Timer};
use oer_time_virtual::VirtualClock;
use std::rc::Rc;

struct Model {
    context: Cell<u8>,
    enables: Cell<usize>,
    disables: Cell<usize>,
    finishes: Cell<usize>,
    abandons: Cell<usize>,
    refuse: bool,
    poison: bool,
    stopped: bool,
    stall: bool,
    late: bool,
    /// Disable drains nothing until its deadline.
    stall_disable: Cell<bool>,
    clock: Rc<VirtualClock<4>>,
}
struct Backend<Id: SlotIdentity> {
    model: Rc<Model>,
    identity: PhantomData<fn(Id) -> Id>,
}
struct Queues<Id: SlotIdentity> {
    token: Option<Slot<'static, (), Root<Id>>>,
    model: Rc<Model>,
}
struct Raw<'s, Id: SlotIdentity> {
    token: &'s mut Option<Slot<'static, (), Root<Id>>>,
    model: Rc<Model>,
}
struct View<'p, Id: SlotIdentity> {
    model: Rc<Model>,
    _loan: BodyLoan<'p, Id, Backend<Id>>,
}
struct Phase;
impl PortPhase for Phase {
    type Event = ();
    type Ready<'a>
        = ()
    where
        Self: 'a;
    type Fault = u8;
    async fn next_ready(&mut self) -> Result<Result<(), EventsLost>, EventEnd<u8>> {
        pending().await
    }
    fn take_ready<'a>(ready: ()) -> Result<(), EventEnd<u8>>
    where
        Self: 'a,
    {
        Ok(ready)
    }
}
impl<Id: SlotIdentity> SlotOwner<Id> for Backend<Id> {
    type Pools = Slots<(), 1>;
    type Queues = Option<Queues<Id>>;
    const VACANT_QUEUES: Self::Queues = None;
    type Vacant = Rc<Model>;
    type Frontend<'s>
        = Raw<'s, Id>
    where
        Self: 's,
        Id: 's;
    type Invocation<'s>
        = ()
    where
        Self: 's,
        Id: 's;
    fn prepare_queues(&mut self, backing: &mut Self::Queues, mint: Mint<'static, Self::Pools, Id>) {
        assert_eq!(self.model.context.get(), 2);
        let [token] = mint.into_pools();
        *backing = Some(Queues {
            token: Some(token),
            model: self.model.clone(),
        });
    }
    fn split<'s>(
        queues: &'s mut Self::Queues,
        _: SlotWakes<'s, Id>,
    ) -> Result<(Raw<'s, Id>, ()), crate::SlotInitError>
    where
        Self: 's,
        Id: 's,
    {
        let queues = queues.as_mut().expect("prepared model queues");
        Ok((
            Raw {
                token: &mut queues.token,
                model: queues.model.clone(),
            },
            (),
        ))
    }
    async fn run<'s>(
        &mut self,
        _: &mut (),
        retire: SlotRetire<'_, Id, Self>,
    ) -> SlotStopped<Id, Self>
    where
        Self: 's,
        Id: 's,
    {
        assert_eq!(self.model.context.get(), 2);
        if self.model.stall {
            pending::<()>().await;
        }
        retire.requested().await
    }

    fn finish<'s>(&mut self, _: &mut Self::Invocation<'s>) -> Rc<Model>
    where
        Self: 's,
        Id: 's,
    {
        assert_eq!(self.model.context.get(), 2);
        self.model.finishes.set(self.model.finishes.get() + 1);
        self.model.clone()
    }
}
impl<Id: SlotIdentity> MockCleanup<Id> for Backend<Id> {
    fn abandon<'s>(&mut self, _: Option<&mut ()>, q: Quarantine) -> Abandon
    where
        Self: 's,
        Id: 's,
    {
        assert_eq!(self.model.context.get(), 2);
        self.model.abandons.set(self.model.abandons.get() + 1);
        if self.model.late {
            self.model
                .clock
                .advance(Duration::from_micros(20))
                .expect("bounded fixture advance");
        }
        if self.model.stopped {
            Abandon::Stopped(q.stopped())
        } else {
            Abandon::Unconfirmed(q)
        }
    }
}

impl<Id: SlotIdentity> ModelSlotOwner<Id> for Backend<Id> {
    fn end_world(&mut self, _: Option<&mut Self::Queues>) {
        assert_eq!(self.model.context.get(), 2);
    }
    fn end_vacant_world(model: &mut Rc<Model>) {
        assert_eq!(model.context.get(), 2);
    }
}
impl<Id: SlotIdentity> SlotBodyOwner<Id> for Backend<Id> {
    type Body<'p>
        = InstallParts<'p, Id, Self>
    where
        Self: 'p,
        Id: 'p;
    fn lend<'p, 's>(
        raw: &'p mut Raw<'s, Id>,
        loan: BodyLoan<'p, Id, Self>,
    ) -> Result<InstallParts<'p, Id, Self>, crate::SlotInitError>
    where
        Self: 's,
        Id: 's,
        's: 'p,
    {
        assert_eq!(raw.model.context.get(), 1);
        Ok(InstallParts {
            view: View {
                model: raw.model.clone(),
                _loan: loan,
            },
            resources: loan.lend_slot(raw.token.take().expect("one capacity")),
        })
    }
}
impl<Id: SlotIdentity> PortBackend<Id> for Backend<Id> {
    type Domain = ();
    type Fault = u8;
    type Refusal = ();
    type Uninstall = ();
    type Event<'p>
        = ()
    where
        Self: 'p,
        Id: 'p;
    type Terminal<'p>
        = ()
    where
        Self: 'p,
        Id: 'p;
    type View<'p>
        = View<'p, Id>
    where
        Self: 'p,
        Id: 'p;
    type Resources<'p>
        = Slot<'p, (), Root<Id>>
    where
        Self: 'p,
        Id: 'p;
    type Setup<'a, 'p>
        = ()
    where
        Self: 'p,
        Id: 'p,
        'p: 'a;
    type Phase<'a, 'p>
        = Phase
    where
        Self: 'p,
        Id: 'p,
        'p: 'a;
    fn body_parts<'p>(body: Self::Body<'p>) -> InstallParts<'p, Id, Self>
    where
        Self: 'p,
        Id: 'p,
    {
        body
    }
    async fn enable<'a, 'p>(_: Call, view: &'a mut View<'p, Id>) -> EnableResult<(), u8>
    where
        Self: 'p,
        Id: 'p,
        'p: 'a,
    {
        assert_eq!(view.model.context.get(), 1);
        view.model.enables.set(view.model.enables.get() + 1);
        if view.model.poison {
            Err(Poisoned { cause: 7 })
        } else if view.model.refuse {
            Ok(Err(()))
        } else {
            Ok(Ok(()))
        }
    }
    async fn disable<'a, 'p, S>(
        _: Call,
        view: &'a mut View<'p, Id>,
        deadline: StopDeadline,
        _: &'a mut S,
    ) -> Result<(), DisableEnd<u8>>
    where
        Self: 'p,
        Id: 'p,
        'p: 'a,
        S: FnMut(()) -> core::ops::ControlFlow<()>,
    {
        assert_eq!(view.model.context.get(), 1);
        let clock = view.model.clock.clone();
        // A deadline that passed already starts no drain.
        if deadline.is_some_and(|deadline| clock.now() >= deadline) {
            return Err(DisableEnd::Expired);
        }
        view.model.disables.set(view.model.disables.get() + 1);
        if !view.model.stall_disable.get() {
            return Ok(());
        }
        match deadline {
            Some(deadline) => {
                clock.wait_until(deadline).await;
                Err(DisableEnd::Expired)
            }
            None => pending().await,
        }
    }
    async fn now<'a, 'p>(
        _: Call,
        _: &'a View<'p, Id>,
    ) -> PortResult<RadioInstant<()>, ClockError, u8>
    where
        Self: 'p,
        Id: 'p,
        'p: 'a,
    {
        Ok(Ok(RadioInstant::from_micros(0)))
    }
    fn setup<'a, 'p>(_: Call, _: &'a mut View<'p, Id>)
    where
        Self: 'p,
        Id: 'p,
        'p: 'a,
    {
    }
    fn phase<'a, 'p>(_: Call, _: &'a mut View<'p, Id>) -> Phase
    where
        Self: 'p,
        Id: 'p,
        'p: 'a,
    {
        Phase
    }
    fn request_uninstall<'s>(_: &mut Raw<'s, Id>, (): ()) -> Result<(), Poisoned<u8>>
    where
        Self: 's,
        Id: 's,
    {
        Ok(())
    }
    fn shutdown_bound<'s>(
        raw: &Self::Frontend<'s>,
        deadline: StopDeadline,
    ) -> impl Future<Output = ()> + use<'s, Id>
    where
        Self: 's,
        Id: 's,
    {
        fn owned_bound(clock: Rc<VirtualClock<4>>, stop: StopDeadline) -> impl Future<Output = ()> {
            async move {
                let deadline = clock
                    .now()
                    .checked_add(Duration::from_micros(10))
                    .expect("bounded fixture instant");
                let deadline = earliest(Some(deadline), stop).expect("a bound");
                clock.wait_until(deadline).await;
            }
        }
        owned_bound(raw.model.clock.clone(), deadline)
    }
}

enum Body {
    Normal,
    FailStop,
    DroppedOwner,
    HoldingOwner,
    /// Enable, then disable and uninstall by this stop deadline.
    StopBy(Instant),
}
impl<Id: SlotIdentity> PortBody<Id, Backend<Id>> for Body {
    type Output = u8;
    async fn run<'a, 'p>(
        self,
        owner: Owner<'a, 'p, Id, Backend<Id>, Disabled>,
        resource: Slot<'p, (), Root<Id>>,
    ) -> Ended<Id, Backend<Id>, u8>
    where
        Backend<Id>: 'p,
        Id: 'p,
        'p: 'a,
    {
        let output = resource.index() + 42;
        match self {
            Self::FailStop => Ended::fail_stop(owner.fail_stop(), output),
            Self::DroppedOwner => {
                drop(owner);
                pending().await
            }
            Self::HoldingOwner => {
                let _owner = owner;
                pending().await
            }
            Self::Normal => {
                let owner = match owner.enable().await {
                    Ok(o) => o,
                    Err(e) => return Ended::fail_stop(e.fail_stop(), output),
                };
                let owner = match owner
                    .disable(None, &mut |_| core::ops::ControlFlow::Continue(()))
                    .await
                {
                    Ok(o) => o,
                    Err(e) => return Ended::fail_stop(e.fail_stop(), output),
                };
                Ended::uninstall(owner, (), output)
            }
            Self::StopBy(deadline) => {
                let owner = match owner.enable().await {
                    Ok(o) => o,
                    Err(e) => return Ended::fail_stop(e.fail_stop(), output),
                };
                match owner
                    .disable(
                        Some(deadline),
                        &mut |_| core::ops::ControlFlow::Continue(()),
                    )
                    .await
                {
                    Ok(owner) => {
                        assert_eq!(owner.deadline(), Some(deadline));
                        Ended::uninstall(owner, (), output)
                    }
                    Err(failed) => {
                        let request = failed.fail_stop();
                        assert_eq!(request.deadline(), Some(deadline));
                        Ended::fail_stop(request, output)
                    }
                }
            }
        }
    }
}
fn model(refuse: bool, poison: bool, stopped: bool, stall: bool, late: bool) -> Model {
    Model {
        context: Cell::new(2),
        enables: Cell::new(0),
        disables: Cell::new(0),
        finishes: Cell::new(0),
        abandons: Cell::new(0),
        refuse,
        poison,
        stopped,
        stall,
        late,
        stall_disable: Cell::new(false),
        clock: Rc::new(VirtualClock::starting_at(Instant::EPOCH)),
    }
}
fn poll<F: Future>(f: Pin<&mut F>) -> Poll<F::Output> {
    f.poll(&mut Context::from_waker(Waker::noop()))
}
fn finish<F: Future>(f: F) -> F::Output {
    let mut f = pin!(f);
    match poll(f.as_mut()) {
        Poll::Ready(x) => x,
        _ => panic!("synchronous fixture"),
    }
}

#[test]
fn frontend_supervisor_owns_no_p2_state_and_returns_join_or_typed_stop_outcome() {
    for (body, refuse, poison, stopped, late) in [
        (Body::Normal, false, false, true, false),
        (Body::Normal, true, false, true, false),
        (Body::Normal, false, true, true, false),
        (Body::FailStop, false, false, true, false),
        (Body::FailStop, false, false, false, false),
        (Body::FailStop, false, false, true, true),
        (Body::DroppedOwner, false, false, true, false),
        (
            Body::StopBy(Instant::EPOCH.saturating_add(Duration::from_secs(1))),
            false,
            false,
            true,
            false,
        ),
    ] {
        let m = Rc::new(model(refuse, poison, stopped, false, late));
        let dropped = matches!(body, Body::DroppedOwner);
        let normal = matches!(body, Body::Normal | Body::StopBy(_)) && !refuse && !poison;
        finish(with_local_slot(async |claim| {
            let mut slot = claim.place();
            let backend = Backend {
                model: m.clone(),
                identity: PhantomData,
            };
            let outer = LocalExecution.scope(
                guarded(
                    (&mut slot)
                        .register(backend)
                        .ok()
                        .expect("register before body"),
                ),
                async |frontend| {
                    let mut p1 = pin!(run_port(frontend, body));
                    poll_fn(|cx| {
                        m.context.set(1);
                        let r = p1.as_mut().poll(cx);
                        m.context.set(2);
                        r
                    })
                    .await
                },
            );
            let result = finish(outer);
            match result {
                PortEnd::Uninstalled { joined, output, .. } => {
                    assert!(normal);
                    assert_eq!(output, 42);
                    assert_eq!(m.finishes.get(), 1);
                    let (frontend, p2) = slot
                        .reinstall(joined, |model| Backend {
                            model,
                            identity: PhantomData,
                        })
                        .ok()
                        .expect("same vacant hardware");
                    drop(frontend);
                    drop(MockRegistration::new(p2));
                }
                PortEnd::FailStopped { outcome, output } => {
                    assert!(!normal && !dropped);
                    assert_eq!(output, 42);
                    assert_eq!(
                        matches!(outcome.outcome(), Abandon::Stopped(_)),
                        stopped && !late
                    );
                }
                PortEnd::Abandoned { outcome, .. } => {
                    assert!(dropped);
                    assert!(matches!(outcome.outcome(), Abandon::Stopped(_)));
                }
            }
            assert_eq!(m.abandons.get(), 1);
            slot.end_model_world();
        }));
        assert_eq!(Rc::strong_count(&m), 1);
    }
}

#[test]
fn cancelling_run_port_before_or_after_first_poll_only_requests_p2_cleanup() {
    for poll_p1 in [false, true] {
        let model = Rc::new(model(false, false, true, false, false));
        finish(with_local_slot(async |claim| {
            let mut slot = claim.place();
            let backend = Backend {
                model: model.clone(),
                identity: PhantomData,
            };
            let outer = LocalExecution.scope(
                guarded(
                    (&mut slot)
                        .register(backend)
                        .ok()
                        .expect("registered before frontend publication"),
                ),
                async |frontend| {
                    let mut p1 = std::boxed::Box::pin(run_port(frontend, Body::HoldingOwner));
                    model.context.set(1);
                    if poll_p1 {
                        assert!(poll(p1.as_mut()).is_pending());
                    }
                    drop(p1);
                    assert_eq!(model.abandons.get(), 0, "P1 cannot run the cleanup guard");
                    assert_eq!(model.finishes.get(), 0);
                    model.context.set(2);
                },
            );
            finish(outer);
            assert_eq!(model.abandons.get(), 1);
            assert_eq!(model.finishes.get(), 0);
            slot.end_model_world();
        }));
        assert_eq!(Rc::strong_count(&model), 1);
    }
}

#[test]
fn ordinary_retirement_expiry_preserves_output_and_never_issues_joined() {
    let model = Rc::new(model(false, false, true, true, false));
    finish(with_local_slot(async |claim| {
        let mut slot = claim.place();
        let backend = Backend {
            model: model.clone(),
            identity: PhantomData,
        };
        let outer = LocalExecution.scope(
            guarded((&mut slot).register(backend).ok().expect("registered P2")),
            async |frontend| {
                let mut p1 = pin!(run_port(frontend, Body::Normal));
                poll_fn(|cx| {
                    model.context.set(1);
                    let result = p1.as_mut().poll(cx);
                    if let Poll::Ready(PortEnd::FailStopped { outcome, output }) = &result {
                        assert!(matches!(outcome.outcome(), Abandon::Unconfirmed(_)));
                        assert_eq!(*output, 42);
                        assert_eq!(model.abandons.get(), 0, "P1's timeout leaves cleanup to P2");
                    }
                    model.context.set(2);
                    result
                })
                .await
            },
        );
        let mut outer = std::boxed::Box::pin(outer);
        assert!(poll(outer.as_mut()).is_pending());
        model
            .clock
            .advance(Duration::from_micros(10))
            .expect("virtual deadline");
        let result = match poll(outer.as_mut()) {
            Poll::Ready(result) => result,
            _ => panic!("bounded stop"),
        };
        assert!(matches!(result, PortEnd::FailStopped { output: 42, .. }));
        assert_eq!(model.abandons.get(), 1);
        assert_eq!(model.finishes.get(), 0);
        drop(outer);
        slot.end_model_world();
    }));
    assert_eq!(Rc::strong_count(&model), 1);
}

/// Run `body` to its end on `model`, advancing the model's clock by
/// `advance` after the first pending poll; the end and the published cause.
fn stopped_by(
    model: &Rc<Model>,
    body: Body,
    advance: Option<Duration>,
) -> (bool, Option<LifecycleError>) {
    let mut ended = (false, None);
    finish(with_local_slot(async |claim| {
        let mut slot = claim.place();
        let backend = Backend {
            model: model.clone(),
            identity: PhantomData,
        };
        let outer = LocalExecution.scope(
            guarded((&mut slot).register(backend).ok().expect("registered P2")),
            async |frontend| {
                let mut p1 = pin!(run_port(frontend, body));
                poll_fn(|cx| {
                    model.context.set(1);
                    let result = p1.as_mut().poll(cx);
                    model.context.set(2);
                    result
                })
                .await
            },
        );
        let mut outer = std::boxed::Box::pin(outer);
        let result = match poll(outer.as_mut()) {
            Poll::Ready(result) => result,
            Poll::Pending => {
                model
                    .clock
                    .advance(advance.expect("a pending stop advances"))
                    .expect("virtual deadline");
                match poll(outer.as_mut()) {
                    Poll::Ready(result) => result,
                    Poll::Pending => panic!("the stop ends at its deadline"),
                }
            }
        };
        ended = match result {
            PortEnd::Uninstalled { deadline, .. } => {
                assert!(deadline.is_some());
                (true, None)
            }
            PortEnd::FailStopped { outcome, output } => {
                assert_eq!(output, 42);
                // A stop that missed its deadline proves no stop.
                assert!(matches!(outcome.outcome(), Abandon::Unconfirmed(_)));
                (false, Some(outcome.cause()))
            }
            PortEnd::Abandoned { .. } => panic!("the body ended"),
        };
        drop(outer);
        slot.end_model_world();
    }));
    ended
}

#[test]
fn a_stop_deadline_passed_before_disable_publishes_deadline_without_starting_it() {
    let model = Rc::new(model(false, false, true, false, false));
    let (joined, cause) = stopped_by(&model, Body::StopBy(Instant::EPOCH), None);
    assert!(!joined);
    assert_eq!(cause, Some(LifecycleError::Deadline), "not Cancelled");
    assert_eq!(
        model.disables.get(),
        0,
        "no drain started past its deadline"
    );
    // No new full bound starts after the expiry: the supervisor's wait
    // ends at the same deadline, and P2 cleans up once.
    assert_eq!(model.abandons.get(), 1);
    assert_eq!(
        model.finishes.get(),
        0,
        "no join, no reuse after quarantine"
    );
    assert_eq!(Rc::strong_count(&model), 1);
}

#[test]
fn a_stop_deadline_during_disable_publishes_deadline_and_issues_no_disabled_owner() {
    let model = Rc::new(model(false, false, true, false, false));
    model.stall_disable.set(true);
    let deadline = Instant::EPOCH.saturating_add(Duration::from_micros(5));
    let (joined, cause) = stopped_by(
        &model,
        Body::StopBy(deadline),
        Some(Duration::from_micros(5)),
    );
    assert!(!joined);
    assert_eq!(cause, Some(LifecycleError::Deadline));
    assert_eq!(model.disables.get(), 1);
    assert_eq!(model.abandons.get(), 1, "one cleanup, no double return");
    assert_eq!(model.finishes.get(), 0);
}

#[test]
fn a_join_answer_after_the_stop_deadline_issues_no_joined() {
    // P2 never answers the uninstall in time; the stop's deadline, earlier
    // than the backend's own bound, ends the wait, and a ready answer after
    // it cannot produce Joined.
    let model = Rc::new(model(false, false, true, true, false));
    let deadline = Instant::EPOCH.saturating_add(Duration::from_micros(3));
    let (joined, cause) = stopped_by(
        &model,
        Body::StopBy(deadline),
        Some(Duration::from_micros(3)),
    );
    assert!(!joined);
    assert_eq!(cause, Some(LifecycleError::Deadline));
    assert_eq!(model.disables.get(), 1);
    assert_eq!(model.abandons.get(), 1);
    assert_eq!(model.finishes.get(), 0, "no reuse after quarantine");
}

#[test]
fn a_late_quarantine_answer_after_the_stop_deadline_proves_no_stop() {
    // The P2 cleanup answers only past the deadline: it stays unconfirmed.
    let model = Rc::new(model(false, false, true, false, true));
    let (joined, cause) = stopped_by(&model, Body::StopBy(Instant::EPOCH), None);
    assert!(!joined);
    assert_eq!(cause, Some(LifecycleError::Deadline));
    assert_eq!(model.abandons.get(), 1);
}
