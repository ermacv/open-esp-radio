//! The common radio slot's lifecycle, with separate P1 and P2 ownership.
use super::{Environment, Fault, ModelPhase, Queues, queues::*};
use alloc::rc::Rc;
use core::{
    cell::Cell,
    future::{Future, poll_fn},
    marker::PhantomData,
    ops::ControlFlow,
    task::Poll,
};
use oer_bluetooth_le_port::{backend::OwnedSession, *};
use oer_radio_port::*;
use oer_time::{Clock as _, Duration, RadioInstant, Timer};

type Composition<Id, X> = PhantomData<fn(Id, X) -> (Id, X)>;

/// Permanent modeled P2 owner. Context `X` is selected before any body loan.
pub struct Backend<Id: SlotIdentity, X = ()> {
    env: Rc<Environment>,
    pub(crate) radio: Rc<super::air::Radio>,
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
        backend.epoch = backend.env.next_epoch(&backend.radio)?;
        Ok(backend)
    }
    /// Queue a publication-boundary stimulus for this radio, independent of
    /// its P1 frontend. Packet propagation loss belongs to the Environment.
    pub fn inject(&self, stimulus: super::Stimulus) {
        self.env.inject(&self.radio, stimulus);
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
    initial: &'s mut Option<Initial<Id>>,
}
/// Separate P2 queue endpoints, retained while a P1 wait is cancelled.
pub struct Invocation<'s, Id: SlotIdentity, X> {
    shared: &'s Shared<Id, X>,
}
/// Borrowed body view; its conversion authority remains private to this backend.
pub struct View<'p, Id: SlotIdentity, X> {
    pub(crate) shared: &'p Shared<Id, X>,
    pub(crate) pool: &'p super::queues::Pool<Id>,
    pub(crate) loan: BodyLoan<'p, Id, Backend<Id, X>>,
}
impl<Id: SlotIdentity, X> SlotOwner<Id> for Backend<Id, X> {
    type Pools = (
        Permits<CommandCapacity, CAPACITY, SEATS>,
        Slots<DtmSessionKind, 1>,
    );
    type Queues = Queues<Id, X>;
    const VACANT_QUEUES: Self::Queues = Queues {
        shared: Shared::vacant(),
        initial: None,
    };
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
        *queues = Queues {
            shared: Shared {
                attachment: Some(Attachment {
                    env: self.env.clone(),
                    radio: self.radio.clone(),
                }),
                epoch: self.epoch,
                pool: Some(FifoPermitPool::new(capacity.supply, capacity.line)),
                end: Cell::new(None),
                fault: Cell::new(None),
                ..Shared::vacant()
            },
            initial: Some(Initial {
                seats: capacity.seats,
                idle: OwnedSession::new(session, self.epoch),
            }),
        };
    }
    fn split<'s>(
        queues: &'s mut Self::Queues,
        _: SlotWakes<'s, Id>,
    ) -> Result<(Self::Frontend<'s>, Self::Invocation<'s>), SlotInitError>
    where
        Self: 's,
        Id: 's,
    {
        let q = queues;
        if q.shared.attachment.is_none() {
            return Err(SlotInitError::QueuesUnprepared);
        }
        Ok((
            Frontend {
                shared: &q.shared,
                initial: &mut q.initial,
            },
            Invocation { shared: &q.shared },
        ))
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
            q.driver.register(cx.waker());
            self.env.register(&self.radio, cx.waker());
            q.serve();
            let wait = retire.wait();
            let mut wait = core::pin::pin!(wait);
            wait.as_mut().poll(cx)
        })
        .await;
        if !q.pending.with(|values| values.is_empty())
            || !q.events.with(|values| values.is_empty())
            || !q.quarantine.with(|values| values.is_empty())
            || !q.poisoned_requests.with(|values| values.is_empty())
            || !self.env.is_off(&self.radio)
        {
            retire.request_failure(oer_radio_port::LifecycleError::StopUnconfirmed);
            core::future::pending::<()>().await;
        }
        self.env.detach(&self.radio);
        q.body.clear();
        q.driver.clear();
        retire.requested().await
    }

    fn finish<'s>(&mut self, _: &mut Self::Invocation<'s>) -> Self::Vacant
    where
        Self: 's,
        Id: 's,
    {
        Vacant {
            backend: Self {
                env: self.env.clone(),
                radio: self.radio.clone(),
                epoch: self.epoch,
                identity: PhantomData,
            },
        }
    }
}
#[cfg(any(test, feature = "test-support"))]
impl<Id: SlotIdentity, X> oer_radio_port::conformance::registration::MockCleanup<Id>
    for Backend<Id, X>
{
    fn abandon<'s>(
        &mut self,
        invocation: Option<&mut Self::Invocation<'s>>,
        authority: Quarantine,
    ) -> Abandon
    where
        Self: 's,
        Id: 's,
    {
        self.env.detach(&self.radio);
        if let Some(invocation) = invocation {
            invocation.shared.poison(Fault::Injected);
        }
        Abandon::Stopped(authority.stopped())
    }
}

impl<Id: SlotIdentity, X> SlotBodyOwner<Id> for Backend<Id, X> {
    type Body<'p>
        = InstallParts<'p, Id, Self>
    where
        Self: 'p,
        Id: 'p;
    fn lend<'p, 's>(
        f: &'p mut Self::Frontend<'s>,
        loan: BodyLoan<'p, Id, Self>,
    ) -> Result<Self::Body<'p>, SlotInitError>
    where
        Self: 's,
        Id: 's,
        's: 'p,
    {
        let pool = f
            .shared
            .pool
            .as_ref()
            .ok_or(SlotInitError::QueuesUnprepared)?;
        let resources = f.shared.lend_initial(f.initial, &loan)?;
        Ok(InstallParts {
            view: View {
                shared: f.shared,
                pool,
                loan,
            },
            resources,
        })
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
    async fn disable<'a, 'p, S>(
        _: Call,
        f: &'a mut Self::View<'p>,
        deadline: StopDeadline,
        sink: &'a mut S,
    ) -> Result<(), DisableEnd<Fault>>
    where
        Self: 'p,
        Id: 'p,
        'p: 'a,
        S: FnMut(Self::Terminal<'p>) -> ControlFlow<Self::Terminal<'p>>,
    {
        let q = f.shared;
        if let Some(cause) = q.fault.get() {
            return Err(DisableEnd::Poisoned(Poisoned { cause }));
        }
        let clock = q
            .attachment
            .as_ref()
            .map(|attachment| attachment.env.clone());
        // A deadline that passed already starts no drain.
        if let (Some(clock), Some(deadline)) = (&clock, deadline)
            && clock.now() >= deadline
        {
            return Err(DisableEnd::Expired);
        }
        q.draining.set(true);
        q.wake_driver();
        let _wait = BodyWait(q);
        let mut alarm = core::pin::pin!(async {
            match (clock, deadline) {
                (Some(clock), Some(deadline)) => clock.wait_until(deadline).await,
                _ => core::future::pending().await,
            }
        });
        poll_fn(|cx| {
            q.body.register(cx.waker());
            // Checked before the drain: a drain that completes only after
            // the deadline issues no disabled owner.
            if alarm.as_mut().poll(cx).is_ready() {
                return Poll::Ready(Err(DisableEnd::Expired));
            }
            // Only P2 settles admitted requests; this wait never serves them.
            if q.draining.get() {
                // The drain stays with P2: at the deadline only the wait ends.
                return match alarm.as_mut().poll(cx) {
                    Poll::Ready(()) => Poll::Ready(Err(DisableEnd::Expired)),
                    Poll::Pending => Poll::Pending,
                };
            }
            loop {
                let event = q.events.with(alloc::collections::VecDeque::pop_front);
                let Some(mut event) = event else {
                    break;
                };
                let id = event.admitted.id;
                let capacity = event.capacity.take();
                if let ControlFlow::Break(terminal) = sink(event.lend(&f.loan)) {
                    let mut retained = OwnedEvent::retain(terminal, &f.loan);
                    retained.capacity = capacity;
                    q.events.with(|events| events.push_front(retained));
                    q.poison(Fault::RejectedTerminal);
                    return Poll::Ready(Err(DisableEnd::Poisoned(Poisoned {
                        cause: Fault::RejectedTerminal,
                    })));
                }
                q.delivered(f.pool, id, capacity);
            }
            // A sink may run past the deadline: then no disabled owner.
            if alarm.as_mut().poll(cx).is_ready() {
                return Poll::Ready(Err(DisableEnd::Expired));
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
            let attachment = f.shared.attachment().map_err(|cause| Poisoned { cause })?;
            Ok(Ok(RadioInstant::from_micros(
                attachment.env.now().as_micros(),
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
    fn shutdown_bound<'s>(
        f: &Self::Frontend<'s>,
        stop: StopDeadline,
    ) -> impl Future<Output = ()> + use<'s, Id, X>
    where
        Self: 's,
        Id: 's,
    {
        let clock = f
            .shared
            .attachment
            .as_ref()
            .map(|attachment| attachment.env.clone());
        async move {
            let Some(clock) = clock else {
                return;
            };
            // Model profile input, not a promise about S31 hardware latency.
            let Some(deadline) = clock.now().checked_add(Duration::from_micros(1_000)) else {
                return;
            };
            let Some(deadline) = oer_radio_port::earliest(Some(deadline), stop) else {
                return;
            };
            clock.wait_until(deadline).await;
        }
    }
}
impl<Id: SlotIdentity, X> ModelSlotOwner<Id> for Backend<Id, X> {
    fn end_world(&mut self, _: Option<&mut Self::Queues>) {
        self.env.detach(&self.radio);
    }
    fn end_vacant_world(vacant: &mut Self::Vacant) {
        vacant.backend.env.detach(&vacant.backend.radio);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct InspectInitial;
    impl<Id: SlotIdentity> SlotBody<Id, Backend<Id>> for InspectInitial {
        type Output = (SlotInitError, SlotInitError);
        async fn run<'p>(self, parts: InstallParts<'p, Id, Backend<Id>>) -> Self::Output
        where
            Backend<Id>: 'p,
            Id: 'p,
        {
            let InstallParts { view, resources } = parts;
            let Resources { seats, session } = resources;
            let mut initial = Some(Initial {
                seats: seats.map(|seat| view.loan.reclaim_seat(seat)),
                idle: OwnedSession::retain(session, &view.loan),
            });
            let vacant = Shared::<Id, ()>::vacant();
            let unprepared = vacant
                .lend_initial(&mut initial, &view.loan)
                .err()
                .expect("vacant queues refuse the loan");
            assert_eq!(unprepared, SlotInitError::QueuesUnprepared);
            assert!(
                initial.is_some(),
                "a refused loan retains every initial owner"
            );
            let resources = view
                .shared
                .lend_initial(&mut initial, &view.loan)
                .expect("prepared loan");
            assert_eq!(resources.seats.map(|seat| seat.index()), [0, 1, 2]);
            assert_eq!(resources.session.index(), 0);
            let exhausted = view
                .shared
                .lend_initial(&mut initial, &view.loan)
                .err()
                .expect("resources issued once");
            assert_eq!(
                exhausted,
                SlotInitError::MissingResource("BLE initial resources")
            );
            assert!(initial.is_none());
            (unprepared, exhausted)
        }
    }
    #[test]
    fn unprepared_and_repeated_initial_loans_return_faults_without_losing_owners() {
        crate::tests::run(with_local_slot(async |claim| {
            let air = Rc::new(Environment::new(core::num::NonZeroU64::MIN));
            let mut slot = claim.place();
            let (mut frontend, registration) = slot
                .register(Backend::new(air))
                .ok()
                .expect("fresh model slot");
            assert_eq!(
                frontend.run_body(InspectInitial).await,
                Ok((
                    SlotInitError::QueuesUnprepared,
                    SlotInitError::MissingResource("BLE initial resources")
                ))
            );
            drop(frontend);
            drop(registration);
            slot.end_model_world();
        }));
    }
}
