//! The common radio slot's lifecycle, with separate P1 and P2 ownership.
use super::{Environment, Fault, ModelPhase, Queues, queues::*};
use alloc::{collections::VecDeque, rc::Rc, vec::Vec};
use core::{
    cell::{Cell, RefCell},
    future::{Future, poll_fn},
    marker::PhantomData,
    ops::ControlFlow,
    task::Poll,
};
use oer_bluetooth_le_port::{backend::OwnedSession, *};
use oer_radio_port::*;
use oer_time::{Duration, RadioInstant, Timer};

type Composition<Id, X> = PhantomData<fn(Id, X) -> (Id, X)>;
type Seats<'p, Id> = [Seat<'p, CommandCapacity, Child<Root<Id>, 0>>; SEATS];

/// Permanent modeled P2 owner. Context `X` is selected before any body loan.
pub struct Backend<Id: SlotIdentity, X = ()> {
    env: Rc<Environment>,
    pub(crate) radio: usize,
    epoch: u64,
    identity: Composition<Id, X>,
}
/// Stopped modeled hardware, retained in the slot after an ordinary join.
pub struct Vacant<Id: SlotIdentity, X> {
    backend: Backend<Id, X>,
}
impl<Id: SlotIdentity, X> Backend<Id, X> {
    /// Attach an independent radio to shared deterministic ether.
    pub fn new(env: Rc<Environment>) -> Self {
        let radio = env.attach();
        Self {
            env,
            radio,
            epoch: 1,
            identity: PhantomData,
        }
    }
    /// Resume only from the slot's retained hardware; `PortSlot::reinstall`
    /// additionally requires its affine `Joined` authority.
    pub fn resume(vacant: Vacant<Id, X>) -> Result<Self, Fault> {
        let mut backend = vacant.backend;
        backend.epoch = backend.env.next_epoch(backend.radio)?;
        Ok(backend)
    }
    /// Queue a publication-boundary stimulus for this radio, independent of
    /// its P1 frontend. Packet propagation loss belongs to the Environment.
    pub fn inject(&self, stimulus: super::Stimulus) {
        self.env.inject(self.radio, stimulus);
    }
}
/// Complete initial P1 capacity: three waiter seats and one idle test session.
pub struct Resources<'p, Id: SlotIdentity> {
    /// Independent FIFO waiters. The capacity supply holds two command bundles.
    pub seats: [Seat<'p, CommandCapacity, Child<Root<Id>, 0>>; SEATS],
    /// Unique DTM session, returned by successful End.
    pub session: Session<'p, Root<Id>, Idle>,
}
/// Affine P1 queue endpoints. No reference to the registered backend is present.
pub struct Frontend<'s, Id: SlotIdentity, X> {
    shared: &'s Shared<Id, X>,
    seats: &'s mut Option<Seats<'static, Id>>,
    idle: &'s mut Option<OwnedSession<Root<Id>, Idle>>,
}
/// Separate P2 queue endpoints, retained while a P1 wait is cancelled.
pub struct Invocation<'s, Id: SlotIdentity, X> {
    shared: &'s Shared<Id, X>,
}
/// Borrowed body view; its conversion authority remains private to this backend.
pub struct View<'p, Id: SlotIdentity, X> {
    pub(crate) shared: &'p Shared<Id, X>,
    pub(crate) loan: BodyLoan<'p, Id, Backend<Id, X>>,
}
impl<Id: SlotIdentity, X> SlotOwner<Id> for Backend<Id, X> {
    type Pools = (
        Permits<CommandCapacity, CAPACITY, SEATS>,
        Slots<DtmSessionKind, 1>,
    );
    type Queues = Option<Queues<Id, X>>;
    const VACANT_QUEUES: Self::Queues = None;
    type Vacant = Vacant<Id, X>;
    type Frontend<'s>
        = Frontend<'s, Id, X>
    where
        Self: 's,
        Id: 's;
    type Invocation<'s>
        = Invocation<'s, Id, X>
    where
        Self: 's,
        Id: 's;
    fn prepare_queues(&mut self, queues: &mut Self::Queues, mint: Mint<'static, Self::Pools, Id>) {
        let (capacity, [session]) = mint.into_pools();
        *queues = Some(Queues {
            shared: Shared {
                env: self.env.clone(),
                radio: self.radio,
                epoch: self.epoch,
                pool: super::capacity::Pool::new(capacity.supply),
                pending: RefCell::new(VecDeque::new()),
                events: RefCell::new(VecDeque::new()),
                quarantine: RefCell::new(Vec::new()),
                outstanding: RefCell::new(Vec::new()),
                poisoned_requests: RefCell::new(Vec::new()),
                enabled: Cell::new(false),
                draining: Cell::new(false),
                end: Cell::new(None),
                fault: Cell::new(None),
                body: RefCell::new(None),
                driver: RefCell::new(None),
            },
            seats: Some(capacity.seats),
            idle: Some(OwnedSession::new(session, self.epoch)),
        });
    }
    #[allow(
        clippy::expect_used,
        reason = "SlotOwner prepares backing before splitting it"
    )]
    fn split<'s>(
        queues: &'s mut Self::Queues,
        _: SlotWakes<'s, Id>,
    ) -> (Self::Frontend<'s>, Self::Invocation<'s>)
    where
        Self: 's,
        Id: 's,
    {
        let q = queues.as_mut().expect("queues prepared before split");
        (
            Frontend {
                shared: &q.shared,
                seats: &mut q.seats,
                idle: &mut q.idle,
            },
            Invocation { shared: &q.shared },
        )
    }
    async fn run<'s>(
        &mut self,
        invocation: &mut Self::Invocation<'s>,
        retire: SlotRetire<'_, Id, Self>,
    ) -> SlotStopped<Id, Self>
    where
        Self: 's,
        Id: 's,
    {
        let q = invocation.shared;
        poll_fn(|cx| {
            if retire.is_requested() {
                return Poll::Ready(());
            }
            *q.driver.borrow_mut() = Some(cx.waker().clone());
            self.env.register(self.radio, cx.waker());
            q.serve();
            let wait = retire.wait();
            let mut wait = core::pin::pin!(wait);
            wait.as_mut().poll(cx)
        })
        .await;
        if !q.pending.borrow().is_empty()
            || !q.events.borrow().is_empty()
            || !q.quarantine.borrow().is_empty()
            || !q.poisoned_requests.borrow().is_empty()
            || !self.env.is_off(self.radio)
        {
            retire.request_quarantine();
            core::future::pending::<()>().await;
        }
        self.env.detach(self.radio);
        q.body.borrow_mut().take();
        q.driver.borrow_mut().take();
        retire.requested().await
    }
    fn abandon<'s>(
        &mut self,
        invocation: Option<&mut Self::Invocation<'s>>,
        authority: Quarantine,
    ) -> Abandon
    where
        Self: 's,
        Id: 's,
    {
        self.env.detach(self.radio);
        if let Some(invocation) = invocation {
            invocation.shared.poison(Fault::Injected);
        }
        Abandon::Stopped(authority.stopped())
    }
    fn finish<'s>(&mut self, _: &mut Self::Invocation<'s>) -> Self::Vacant
    where
        Self: 's,
        Id: 's,
    {
        Vacant {
            backend: Self {
                env: self.env.clone(),
                radio: self.radio,
                epoch: self.epoch,
                identity: PhantomData,
            },
        }
    }
}
impl<Id: SlotIdentity, X> SlotBodyOwner<Id> for Backend<Id, X> {
    type Body<'p>
        = InstallParts<'p, Id, Self>
    where
        Self: 'p,
        Id: 'p;
    #[allow(
        clippy::expect_used,
        reason = "SlotFrontend issues the initial resources exactly once"
    )]
    fn lend<'p, 's>(f: &'p mut Self::Frontend<'s>, loan: BodyLoan<'p, Id, Self>) -> Self::Body<'p>
    where
        Self: 's,
        Id: 's,
        's: 'p,
    {
        let seats = f
            .seats
            .take()
            .expect("body resources issued once")
            .map(|s| loan.lend_seat(s));
        let session = f.idle.take().expect("one initial DTM session").lend(&loan);
        InstallParts {
            view: View {
                shared: f.shared,
                loan,
            },
            resources: Resources { seats, session },
        }
    }
}
impl<Id: SlotIdentity, X> PortBackend<Id> for Backend<Id, X> {
    type Domain = LeRadio;
    type Fault = Fault;
    type Refusal = Refusal;
    type Event<'p>
        = Event<'p, Root<Id>, X>
    where
        Self: 'p,
        Id: 'p;
    type Terminal<'p>
        = Terminal<'p, Root<Id>, X>
    where
        Self: 'p,
        Id: 'p;
    type View<'p>
        = View<'p, Id, X>
    where
        Self: 'p,
        Id: 'p;
    type Resources<'p>
        = Resources<'p, Id>
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
        = ModelPhase<'a, 'p, Id, X>
    where
        Self: 'p,
        Id: 'p,
        'p: 'a;
    type Uninstall = ();
    fn body_parts<'p>(body: Self::Body<'p>) -> InstallParts<'p, Id, Self>
    where
        Self: 'p,
        Id: 'p,
    {
        body
    }
    async fn enable<'a, 'p>(_: Call, f: &'a mut Self::View<'p>) -> EnableResult<Refusal, Fault>
    where
        Self: 'p,
        Id: 'p,
        'p: 'a,
    {
        if let Some(cause) = f.shared.fault.get() {
            return Err(Poisoned { cause });
        }
        if f.shared.end.get().is_some() {
            return Ok(Err(Refusal::Unavailable));
        }
        f.shared.enabled.set(true);
        Ok(Ok(()))
    }
    async fn disable<'a, 'p>(
        _: Call,
        f: &'a mut Self::View<'p>,
        sink: &'a mut dyn FnMut(Self::Terminal<'p>) -> ControlFlow<Self::Terminal<'p>>,
    ) -> Result<(), Poisoned<Fault>>
    where
        Self: 'p,
        Id: 'p,
        'p: 'a,
    {
        let q = f.shared;
        if let Some(cause) = q.fault.get() {
            return Err(Poisoned { cause });
        }
        q.draining.set(true);
        q.wake_driver();
        let _wait = BodyWait(q);
        poll_fn(|cx| {
            *q.body.borrow_mut() = Some(cx.waker().clone());
            // Only P2 settles admitted requests; this wait never serves them.
            if q.draining.get() {
                return Poll::Pending;
            }
            loop {
                let event = q.events.borrow_mut().pop_front();
                let Some(mut event) = event else {
                    break;
                };
                let id = event.admitted.id;
                let capacity = event.capacity.take();
                if let ControlFlow::Break(terminal) = sink(event.lend(&f.loan)) {
                    let mut retained = OwnedEvent::retain(terminal, &f.loan);
                    retained.capacity = capacity;
                    q.events.borrow_mut().push_front(retained);
                    q.poison(Fault::RejectedTerminal);
                    return Poll::Ready(Err(Poisoned {
                        cause: Fault::RejectedTerminal,
                    }));
                }
                q.delivered(id, capacity);
            }
            Poll::Ready(Ok(()))
        })
        .await
    }
    async fn now<'a, 'p>(
        _: Call,
        f: &'a Self::View<'p>,
    ) -> PortResult<RadioInstant<LeRadio>, ClockError, Fault>
    where
        Self: 'p,
        Id: 'p,
        'p: 'a,
    {
        if let Some(cause) = f.shared.fault.get() {
            Err(Poisoned { cause })
        } else {
            Ok(Ok(RadioInstant::from_micros(
                f.shared.env.now().as_micros(),
            )))
        }
    }
    fn setup<'a, 'p>(_: Call, _: &'a mut Self::View<'p>)
    where
        Self: 'p,
        Id: 'p,
        'p: 'a,
    {
    }
    fn phase<'a, 'p>(_: Call, f: &'a mut Self::View<'p>) -> Self::Phase<'a, 'p>
    where
        Self: 'p,
        Id: 'p,
        'p: 'a,
    {
        ModelPhase { view: f }
    }
    fn request_uninstall<'s>(_: &mut Self::Frontend<'s>, (): ()) -> Result<(), Poisoned<Fault>>
    where
        Self: 's,
        Id: 's,
    {
        Ok(())
    }
    fn shutdown_bound<'s>(f: &Self::Frontend<'s>) -> impl Future<Output = ()> + use<'s, Id, X>
    where
        Self: 's,
        Id: 's,
    {
        let clock = f.shared.env.clone();
        async move {
            // Model profile input, not a promise about S31 hardware latency.
            let Some(deadline) = clock.now().checked_add(Duration::from_micros(1_000)) else {
                return;
            };
            clock.wait_until(deadline).await;
        }
    }
}
impl<Id: SlotIdentity, X> ModelSlotOwner<Id> for Backend<Id, X> {
    fn end_world(&mut self, _: Option<&mut Self::Queues>) {
        self.env.detach(self.radio);
    }
    fn end_vacant_world(vacant: &mut Self::Vacant) {
        vacant.backend.env.detach(vacant.backend.radio);
    }
}
