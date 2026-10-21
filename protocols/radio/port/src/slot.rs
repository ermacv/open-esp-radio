//! Affine construction and permanent ownership of a registered P2 side.

use core::{
    future::{Future, poll_fn},
    marker::PhantomData,
    mem::ManuallyDrop,
    pin::pin,
    sync::atomic::{AtomicU8, Ordering},
    task::{Context, Poll},
};

use atomic_waker::AtomicWaker;

use crate::{Abandon, LocalId, Quarantine, SlotIdentity};

/// The registered P2 state and its separate queue storage.
/// None of its operations run on the frontend.
pub trait SlotOwner<Id: SlotIdentity>: Sized {
    /// Capacity layout issued once per joined installation.
    type Pools: crate::PoolSet;
    /// Fixed queue backing, retained with the slot on every abnormal path.
    /// Normal retirement drains all payload owners before returning its proof.
    type Queues;
    /// Stopped hardware owners retained on P2 between joined installations.
    /// These never travel through the frontend result or join authority.
    type Vacant;
    /// Affine P1 queue endpoints; these cannot borrow the P2 owner.
    type Frontend<'s>
    where
        Self: 's,
        Id: 's;
    /// Affine P2 endpoints, borrowed only from the separate queue backing.
    type Invocation<'s>
    where
        Self: 's,
        Id: 's;

    /// Distribute the single owned capacity mint and prepare queue storage.
    /// This does not publish a frontend or start device work.
    fn queues(&mut self, mint: crate::Mint<'static, Self::Pools, Id>) -> Self::Queues;

    /// Split each queue exactly once, transferring its affine endpoints.
    fn split<'s>(
        queues: &'s mut Self::Queues,
        wakes: SlotWakes<'s, Id>,
    ) -> (Self::Frontend<'s>, Self::Invocation<'s>)
    where
        Self: 's,
        Id: 's;

    /// Drive until retirement, then stop and detach before returning its proof.
    /// Every poll is bounded. Cancellation ends only this operation's loan;
    /// progress and admitted owners stay in `self` or `invocation`.
    fn run<'s>(
        &mut self,
        invocation: &mut Self::Invocation<'s>,
        retire: SlotRetire<'_, Id, Self>,
    ) -> impl Future<Output = SlotStopped<Id, Self>>
    where
        Self: 's,
        Id: 's;

    /// Quarantine without releasing any hardware-reachable owner. Invocation
    /// retains its endpoints through cancellation, so cleanup can also erase
    /// secrets in queued payloads. It is absent only during initialization.
    fn abandon<'s>(
        &mut self,
        invocation: Option<&mut Self::Invocation<'s>>,
        authority: Quarantine,
    ) -> Abandon
    where
        Self: 's,
        Id: 's;

    /// Complete ordinary destruction on P2, retaining stopped hardware there.
    fn finish(self) -> Self::Vacant;
}

/// Backend construction of public views under a fresh frontend loan.
/// The body never receives the loan-conversion authority itself.
pub trait SlotBodyOwner<Id: SlotIdentity>: SlotOwner<Id> {
    /// Body resources and queue views, invariant in their separate loan.
    type Body<'p>
    where
        Self: 'p,
        Id: 'p;

    /// Lend public resources while retaining conversion authority privately.
    /// Only owned Id payloads may be published to the P2 queues.
    fn lend<'p, 's>(
        frontend: &'p mut Self::Frontend<'s>,
        loan: crate::BodyLoan<'p, Id, Self>,
    ) -> Self::Body<'p>
    where
        Self: 's,
        Id: 's,
        's: 'p;
}

/// One body whose output is fixed before its frontend loan is introduced.
pub trait SlotBody<Id: SlotIdentity, S: SlotBodyOwner<Id>> {
    /// Result independent of the body's loan, including for a static Id.
    type Output;

    /// Consume the public views; their fresh loan cannot escape in Output.
    fn run<'p>(self, views: S::Body<'p>) -> impl Future<Output = Self::Output>
    where
        S: 'p,
        Id: 'p;
}

/// A local model whose entire asynchronous hardware world can be ended.
/// Physical hardware backends do not implement this trait.
pub trait ModelSlotOwner<Id: SlotIdentity>: SlotOwner<Id> {
    /// Remove every modeled asynchronous reachability before destroying a
    /// retained owner or queue. Called on P2 after its registration ended.
    fn end_world(&mut self, queues: Option<&mut Self::Queues>);

    /// End a world whose ordinary stop retained only its vacant hardware.
    fn end_vacant_world(vacant: &mut Self::Vacant);
}

/// P2's affine authority for stopping and detaching this registration.
#[must_use = "complete stop and detach before returning its proof"]
pub struct SlotRetire<'s, Id: SlotIdentity, S: SlotOwner<Id>> {
    owner: PhantomData<fn(S) -> S>,
    signals: &'s Signals,
    identity: PhantomData<fn(Id) -> Id>,
}

impl<Id: SlotIdentity, S: SlotOwner<Id>> SlotRetire<'_, Id, S> {
    /// Whether the frontend has closed and requested retirement.
    pub fn is_requested(&self) -> bool {
        self.signals.state.load(Ordering::Acquire) != RUNNING
    }

    /// Wait for the request while keeping the authority for final handoff.
    pub async fn wait(&self) {
        poll_fn(|cx| {
            self.signals.p2.register(cx.waker());
            if self.is_requested() {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
        .await;
    }

    /// Complete the consumed retirement handoff, after stopping and detaching.
    /// A proof cannot precede the frontend's actual request.
    pub async fn requested(self) -> SlotStopped<Id, S> {
        self.wait().await;
        SlotStopped {
            identity: self.identity,
            owner: self.owner,
        }
    }
}

/// Proof of stopping and detachment, tied to its exact slot identity.
#[must_use = "consume the proof during P2 destruction"]
pub struct SlotStopped<Id: SlotIdentity, S: SlotOwner<Id>> {
    owner: PhantomData<fn(S) -> S>,
    identity: PhantomData<fn(Id) -> Id>,
}

/// One construction authority, consumed to create exactly one backing slot.
///
/// Local scopes issue this value generatively. Static placement adapters may
/// construct its zero-sized representation only once per nominal declaration;
/// they must establish that uniqueness before publishing any slot or endpoint.
/// There is no safe generic constructor, clone or identity conversion.
#[repr(C)]
#[must_use = "consume the authority to place one slot"]
pub struct SlotClaim<Id: SlotIdentity> {
    identity: PhantomData<fn(Id) -> Id>,
}

impl<Id: SlotIdentity> SlotClaim<Id> {
    /// Create the backing before any registration or frontend is published.
    pub const fn place<S: SlotOwner<Id>>(self) -> PortSlot<Id, S> {
        PortSlot {
            owner: None,
            queues: None,
            signals: Signals::new(),
            identity: PhantomData,
        }
    }
}

/// Enter an outer local scope with a fresh invariant identity and no latch.
///
/// The callback owns the backing it places and its local P2 registration. It
/// must end that registration before ending the modeled hardware world. An
/// inner frontend future may end independently of this outer scope.
pub fn with_local_slot<R>(
    scope: impl for<'scope> AsyncFnOnce(SlotClaim<LocalId<'scope>>) -> R,
) -> impl Future<Output = R> {
    scope(SlotClaim {
        identity: PhantomData,
    })
}

const INITIAL: u8 = 0;
const RUNNING: u8 = 1;
const RETIRING: u8 = 2;
const FINISHING: u8 = 3;
const JOINED: u8 = 4;
const QUARANTINED: u8 = 5;
const NO_ANSWER: u8 = 0;
const STOPPED_ANSWER: u8 = 1;
const UNCONFIRMED_ANSWER: u8 = 2;
const TAKEN_ANSWER: u8 = 3;

struct Signals {
    state: AtomicU8,
    answer: AtomicU8,
    p2: AtomicWaker,
    join: AtomicWaker,
}

impl Signals {
    const fn new() -> Self {
        Self {
            state: AtomicU8::new(INITIAL),
            answer: AtomicU8::new(NO_ANSWER),
            p2: AtomicWaker::new(),
            join: AtomicWaker::new(),
        }
    }

    fn quarantine(&self) {
        // No transition out of quarantine exists, including after a late
        // hardware stop or a concurrent normal teardown.
        self.state.store(QUARANTINED, Ordering::Release);
        self.p2.wake();
        self.join.wake();
    }

    fn publish_answer(&self, outcome: Abandon) {
        // Both affine proofs are zero sized. Consume the actual P2 proof;
        // only the installation's unique receiver may reconstruct it after
        // Acquire. This cell never carries a reference or a generic payload.
        let answer = match outcome {
            Abandon::Stopped(_) => STOPPED_ANSWER,
            Abandon::Unconfirmed(_) => UNCONFIRMED_ANSWER,
        };
        self.answer.store(answer, Ordering::Release);
        self.join.wake();
    }

    fn take_answer(&self) -> Option<Abandon> {
        let answer = match self.answer.load(Ordering::Acquire) {
            STOPPED_ANSWER => Abandon::Stopped(Quarantine::new().stopped()),
            UNCONFIRMED_ANSWER => Abandon::Unconfirmed(Quarantine::new()),
            _ => return None,
        };
        self.answer.store(TAKEN_ANSWER, Ordering::Relaxed);
        Some(answer)
    }
}

/// Wake endpoints for this slot's single frontend task and P2 runner.
/// Publication uses the queue's Release/Acquire handoff; these only wake.
pub struct SlotWakes<'s, Id: SlotIdentity> {
    signals: &'s Signals,
    identity: PhantomData<fn(Id) -> Id>,
}

impl<Id: SlotIdentity> Copy for SlotWakes<'_, Id> {}
impl<Id: SlotIdentity> Clone for SlotWakes<'_, Id> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<Id: SlotIdentity> SlotWakes<'_, Id> {
    /// Wake P2 after publishing an owned command or return.
    pub fn wake_p2(&self) {
        self.signals.p2.wake();
    }
    /// Register P1 before its Acquire check of an event/answer queue.
    pub fn register_frontend(&self, cx: &mut Context<'_>) {
        self.signals.join.register(cx.waker());
    }
    /// Wake the single frontend task after publishing an event/answer.
    pub fn wake_frontend(&self) {
        self.signals.join.wake();
    }
}

/// Caller-owned backing, independent of the frontend future's allocation.
///
/// Only a construction claim creates a slot. A registration lends its P2
/// owner independently of the frontend; only ordinary join permits reinstall.
/// Dropping the slot never runs a retained P2 owner's destructor. A local
/// model ends all asynchronous reachability through `end_model_world` after
/// its runner ends; hardware adapters retain quarantined backing until reset.
pub struct PortSlot<Id: SlotIdentity, S: SlotOwner<Id>> {
    owner: Option<ManuallyDrop<Stored<Id, S>>>,
    queues: Option<ManuallyDrop<S::Queues>>,
    signals: Signals,
    identity: PhantomData<fn(Id) -> Id>,
}

enum Stored<Id: SlotIdentity, S: SlotOwner<Id>> {
    Registered(S),
    Vacant(<S as SlotOwner<Id>>::Vacant),
}

fn registered_owner<Id: SlotIdentity, S: SlotOwner<Id>>(
    storage: &mut Option<ManuallyDrop<Stored<Id, S>>>,
) -> &mut S {
    match &mut **storage.as_mut().expect("registration retains its P2 owner") {
        Stored::Registered(owner) => owner,
        Stored::Vacant(_) => unreachable!("a registration owns active state"),
    }
}

impl<Id: SlotIdentity, S: SlotOwner<Id>> PortSlot<Id, S> {
    /// Register the initial P2 owner synchronously, before issuing a frontend.
    /// Repeating this operation never releases the existing owner.
    pub fn register<'s>(
        &'s mut self,
        owner: S,
    ) -> Result<(SlotFrontend<'s, Id, S>, SlotRegistration<'s, Id, S>), S>
    where
        S: 's,
        Id: 's,
    {
        if self.signals.state.load(Ordering::Acquire) != INITIAL {
            return Err(owner);
        }
        Ok(self.install(owner))
    }

    /// Reinstall only with this slot's ordinary, consuming join authority.
    pub fn reinstall<'s, Resume: FnOnce(S::Vacant) -> S>(
        &'s mut self,
        joined: Joined<Id>,
        resume: Resume,
    ) -> Result<(SlotFrontend<'s, Id, S>, SlotRegistration<'s, Id, S>), Resume>
    where
        S: 's,
        Id: 's,
    {
        let Joined { identity: _ } = joined;
        if self.signals.state.load(Ordering::Acquire) != JOINED {
            return Err(resume);
        }
        let Stored::Vacant(vacant) = ManuallyDrop::into_inner(
            self.owner
                .take()
                .expect("join retains its stopped hardware"),
        ) else {
            unreachable!("ordinary join retains vacant state");
        };
        // Every payload was drained before the stop proof. Old backing is
        // destroyed on P2 after all old endpoint loans have ended.
        drop(self.queues.take().map(ManuallyDrop::into_inner));
        let owner = resume(vacant);
        Ok(self.install(owner))
    }

    fn install<'s>(&'s mut self, owner: S) -> (SlotFrontend<'s, Id, S>, SlotRegistration<'s, Id, S>)
    where
        S: 's,
        Id: 's,
    {
        self.owner = Some(ManuallyDrop::new(Stored::Registered(owner)));
        self.signals.answer.store(NO_ANSWER, Ordering::Relaxed);
        self.signals.state.store(RUNNING, Ordering::Release);
        // Arm before invoking backend initialization or splitting its queues.
        let mut registration = SlotRegistration {
            owner: &mut self.owner,
            invocation: None,
            signals: &self.signals,
            armed: true,
            identity: PhantomData,
        };
        let queues =
            registered_owner(registration.owner)
                .queues(crate::Mint::<'static, S::Pools, Id>::new());
        self.queues = Some(ManuallyDrop::new(queues));
        let queues = self
            .queues
            .as_mut()
            .expect("installation initializes its queues");
        let (ports, invocation) = S::split(
            &mut **queues,
            SlotWakes {
                signals: &self.signals,
                identity: PhantomData,
            },
        );
        registration.invocation = Some(invocation);
        (
            SlotFrontend {
                ports: Some(ports),
                signals: &self.signals,
                armed: true,
                identity: PhantomData,
            },
            registration,
        )
    }
}

impl<'scope, S: ModelSlotOwner<LocalId<'scope>>> PortSlot<LocalId<'scope>, S> {
    /// End the entire local model world on P2 after every endpoint and P2
    /// registration loan has ended. Consumes the slot and its identity domain:
    /// no vacant authority, owner or reset identity is returned to the caller.
    /// Dropping a frontend alone never invokes this operation.
    pub fn end_model_world(self) {
        let mut queues = self.queues.map(ManuallyDrop::into_inner);
        if let Some(owner) = self.owner.map(ManuallyDrop::into_inner) {
            match owner {
                Stored::Registered(mut owner) => owner.end_world(queues.as_mut()),
                Stored::Vacant(mut vacant) => S::end_vacant_world(&mut vacant),
            }
        }
        drop(queues);
    }
}

/// P1's lifecycle endpoints. This value has no reference to the P2 owner.
#[must_use = "finish the frontend or request its abandonment"]
pub struct SlotFrontend<'s, Id: SlotIdentity + 's, S: SlotOwner<Id> + 's> {
    ports: Option<S::Frontend<'s>>,
    signals: &'s Signals,
    armed: bool,
    identity: PhantomData<fn(Id) -> Id>,
}

impl<'s, Id: SlotIdentity + 's, S: SlotOwner<Id> + 's> SlotFrontend<'s, Id, S> {
    /// Borrow only the affine queue endpoints, never the backend or held state.
    pub fn ports(&mut self) -> &mut S::Frontend<'s> {
        self.ports
            .as_mut()
            .expect("the live frontend retains its endpoints")
    }

    /// Run public body views under a fresh invariant loan. Only the backend's
    /// constructor receives the authority to move tokens to/from owned queues.
    /// A previously observed abandonment cancels a pending body, including
    /// one awaiting an unrelated future. A body returning in the poll that
    /// published its request still forwards its typed result to the supervisor.
    pub async fn run_body<P: SlotBody<Id, S>>(&mut self, body: P) -> Option<P::Output>
    where
        S: SlotBodyOwner<Id>,
    {
        let signals = self.signals;
        if signals.state.load(Ordering::Acquire) == QUARANTINED {
            return None;
        }
        let views = S::lend(self.ports(), crate::BodyLoan::new());
        let mut body = pin!(body.run(views));
        poll_fn(|cx| {
            signals.join.register(cx.waker());
            if signals.state.load(Ordering::Acquire) == QUARANTINED {
                return Poll::Ready(None);
            }
            match body.as_mut().poll(cx) {
                Poll::Ready(output) => Poll::Ready(Some(output)),
                Poll::Pending if signals.state.load(Ordering::Acquire) == QUARANTINED => {
                    Poll::Ready(None)
                }
                Poll::Pending => Poll::Pending,
            }
        })
        .await
    }

    /// Publish abandonment immediately, without invoking any P2 operation.
    pub fn abandon(mut self) -> SlotJoin<'s, Id> {
        self.signals.quarantine();
        self.armed = false;
        SlotJoin {
            signals: self.signals,
            armed: true,
            identity: PhantomData,
        }
    }

    /// Close the frontend and wait for proved P2 destruction/unregistration.
    pub fn finish(mut self) -> SlotJoin<'s, Id> {
        // End every frontend endpoint before Release publishes retirement.
        drop(self.ports.take());
        let _ = self.signals.state.compare_exchange(
            RUNNING,
            RETIRING,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
        self.signals.p2.wake();
        self.armed = false;
        SlotJoin {
            signals: self.signals,
            armed: true,
            identity: PhantomData,
        }
    }
}

impl<Id: SlotIdentity, S: SlotOwner<Id>> Drop for SlotFrontend<'_, Id, S> {
    fn drop(&mut self) {
        if self.armed {
            self.signals.quarantine();
        }
    }
}

/// A notification of absorbing quarantine; it contains no reuse authority.
pub struct SlotQuarantined<Id: SlotIdentity> {
    outcome: Abandon,
    identity: PhantomData<fn(Id) -> Id>,
}

impl<Id: SlotIdentity> SlotQuarantined<Id> {
    /// What the timely P2 response proved; expiry is always unconfirmed.
    pub fn outcome(&self) -> &Abandon {
        &self.outcome
    }
}

impl<Id: SlotIdentity> core::fmt::Debug for SlotQuarantined<Id> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SlotQuarantined")
            .field("stopped", &matches!(self.outcome, Abandon::Stopped(_)))
            .finish_non_exhaustive()
    }
}

/// Sole affine permission to install again after an ordinary P2 join.
#[must_use = "consume the joined authority to reuse its slot"]
pub struct Joined<Id: SlotIdentity> {
    identity: PhantomData<fn(Id) -> Id>,
}

/// P1's join receiver, with no P2 owner or cleanup destructor.
#[must_use = "await the join or quarantine the installation"]
pub struct SlotJoin<'s, Id: SlotIdentity> {
    signals: &'s Signals,
    armed: bool,
    identity: PhantomData<fn(Id) -> Id>,
}

impl<Id: SlotIdentity> SlotJoin<'_, Id> {
    fn poll(&mut self, cx: &mut Context<'_>) -> Poll<Result<Joined<Id>, SlotQuarantined<Id>>> {
        self.signals.join.register(cx.waker());
        match self.signals.state.load(Ordering::Acquire) {
            JOINED => Poll::Ready(Ok(Joined {
                identity: PhantomData,
            })),
            QUARANTINED => match self.signals.take_answer() {
                Some(outcome) => Poll::Ready(Err(SlotQuarantined {
                    outcome,
                    identity: PhantomData,
                })),
                None => Poll::Pending,
            },
            _ => Poll::Pending,
        }
    }

    /// Wait for ordinary P2 unregistration or its actual quarantine answer.
    /// A request or wake alone proves neither stopping nor a reusable join.
    pub async fn wait(mut self) -> Result<Joined<Id>, SlotQuarantined<Id>> {
        let result = poll_fn(|cx| self.poll(cx)).await;
        self.armed = false;
        result
    }

    /// The lifecycle supervisor's single wait, under its named finite bound.
    /// Deadline checks bracket the answer poll so a late ready proof cannot
    /// produce either `Stopped` or `Joined`.
    pub async fn wait_within(
        mut self,
        bound: impl Future<Output = ()>,
    ) -> Result<Joined<Id>, SlotQuarantined<Id>> {
        let mut bound = pin!(bound);
        let result = poll_fn(|cx| {
            let expired = bound.as_mut().poll(cx).is_ready();
            let answer = self.poll(cx);
            if expired || bound.as_mut().poll(cx).is_ready() {
                self.signals.quarantine();
                return Poll::Ready(Err(SlotQuarantined {
                    outcome: Abandon::Unconfirmed(Quarantine::new()),
                    identity: PhantomData,
                }));
            }
            answer
        })
        .await;
        self.armed = false;
        result
    }
}

impl<Id: SlotIdentity> Drop for SlotJoin<'_, Id> {
    fn drop(&mut self) {
        if self.armed {
            self.signals.quarantine();
        }
    }
}

/// P2's registered cleanup guard, separate from the frontend and its future.
///
/// The execution adapter owns and destroys this value on P2. Frontend drop
/// only wakes it. Its Drop executes quarantine once even without another poll.
#[must_use = "retain on P2 until normal finish or quarantine cleanup"]
pub struct SlotRegistration<'s, Id: SlotIdentity + 's, S: SlotOwner<Id> + 's> {
    invocation: Option<S::Invocation<'s>>,
    owner: &'s mut Option<ManuallyDrop<Stored<Id, S>>>,
    signals: &'s Signals,
    armed: bool,
    identity: PhantomData<fn(Id) -> Id>,
}

impl<'s, Id: SlotIdentity + 's, S: SlotOwner<Id> + 's> SlotRegistration<'s, Id, S> {
    /// Register P2's wake before observing whether new work is prohibited.
    pub fn poll_request(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        self.signals.p2.register(cx.waker());
        match self.signals.state.load(Ordering::Acquire) {
            RUNNING => Poll::Pending,
            _ => Poll::Ready(()),
        }
    }

    /// Whether this poll must take quarantine before any device work.
    pub fn is_quarantined(&self) -> bool {
        self.signals.state.load(Ordering::Acquire) == QUARANTINED
    }

    /// Execute quarantine exactly once on P2, retaining the owner in place.
    pub fn quarantine(&mut self) -> bool {
        self.signals.quarantine();
        if !self.armed {
            return false;
        }
        self.armed = false;
        let outcome =
            registered_owner(self.owner).abandon(self.invocation.as_mut(), Quarantine::new());
        self.signals.publish_answer(outcome);
        true
    }

    /// Drive on P2, prioritizing abandonment before every driver poll.
    /// Stop, detach and destroy the owner before publishing ordinary join.
    /// Cancellation wins over a late proof and keeps quarantine absorbing.
    pub async fn run(mut self) -> Option<SlotCompletion<'s, Id>> {
        if self.is_quarantined() {
            return None;
        }
        let invocation = self
            .invocation
            .as_mut()
            .expect("registration has one P2 invocation");
        let stopped = {
            let signals = self.signals;
            let mut stopping = pin!(registered_owner(self.owner).run(
                invocation,
                SlotRetire {
                    signals,
                    owner: PhantomData,
                    identity: PhantomData
                }
            ));
            poll_fn(|cx| {
                signals.p2.register(cx.waker());
                if signals.state.load(Ordering::Acquire) == QUARANTINED {
                    return Poll::Ready(None);
                }
                stopping.as_mut().poll(cx).map(Some)
            })
            .await
        };
        let Some(stopped) = stopped else {
            return None;
        };
        if self
            .signals
            .state
            .compare_exchange(RETIRING, FINISHING, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return None;
        }
        // The core consumes the proof. Giving it back to the backend would
        // let a same-Id reinstall retain an earlier registration's authority.
        let SlotStopped {
            identity: _,
            owner: _,
        } = stopped;
        self.armed = false;
        let completion = SlotCompletion {
            signals: self.signals,
            armed: true,
            identity: PhantomData,
        };
        drop(self.invocation.take());
        let Stored::Registered(owner) = ManuallyDrop::into_inner(
            self.owner
                .take()
                .expect("ordinary join retains its P2 owner"),
        ) else {
            unreachable!("ordinary retirement owns active state");
        };
        *self.owner = Some(ManuallyDrop::new(Stored::Vacant(owner.finish())));
        Some(completion)
    }
}

impl<Id: SlotIdentity, S: SlotOwner<Id>> Drop for SlotRegistration<'_, Id, S> {
    fn drop(&mut self) {
        if self.armed {
            self.quarantine();
        }
    }
}

/// A finished P2 side, awaiting removal of its executor registration.
/// This contains no backend, queue endpoint or cleanup guard.
#[must_use = "unregister P2 before publishing ordinary join"]
pub struct SlotCompletion<'s, Id: SlotIdentity> {
    signals: &'s Signals,
    armed: bool,
    identity: PhantomData<fn(Id) -> Id>,
}

impl<Id: SlotIdentity> SlotCompletion<'_, Id> {
    /// Publish join after the executor removed every possible P2 poll and
    /// destroyed the completed task future. The local runner performs both
    /// steps before calling this handoff; static executors do the same in
    /// their completion path. P1 never receives this authority.
    pub fn unregistered(mut self) {
        let _ = self.signals.state.compare_exchange(
            FINISHING,
            JOINED,
            Ordering::Release,
            Ordering::Relaxed,
        );
        self.armed = false;
        self.signals.join.wake();
    }
}

impl<Id: SlotIdentity> Drop for SlotCompletion<'_, Id> {
    fn drop(&mut self) {
        if self.armed {
            self.signals.quarantine();
            self.signals
                .publish_answer(Abandon::Unconfirmed(Quarantine::new()));
        }
    }
}

pub(crate) fn run_local<'s, Id: SlotIdentity + 's, S: SlotOwner<Id> + 's, R>(
    slot: &'s mut PortSlot<Id, S>,
    owner: S,
    body: impl AsyncFnOnce(SlotFrontend<'s, Id, S>) -> R,
) -> Result<impl Future<Output = R>, S> {
    // Synchronous registration precedes even construction of the P1 future.
    // The resulting outer future belongs to the local P2 runner.
    let (frontend, registration) = slot.register(owner)?;
    let signals = frontend.signals;
    let body = body(frontend);
    // A never-polled outer future must drop P1 before the P2 cleanup guard,
    // just like the reverse declaration order of the pinned locals below.
    struct Pending<Body, Registration> {
        body: Body,
        registration: Registration,
    }
    let pending = Pending { body, registration };
    Ok(async move {
        let Pending { body, registration } = pending;
        let mut p2 = pin!(Some(registration.run()));
        let mut body = pin!(Some(body));
        let mut output = None;
        let mut registered = true;
        poll_fn(|cx| {
            if let Some(body_future) = body.as_mut().as_pin_mut() {
                if let Poll::Ready(result) = body_future.poll(cx) {
                    output = Some(result);
                    body.as_mut().set(None);
                }
            }
            // Returning after forgetting the frontend cannot leave a running
            // registration behind when this outer scope ends.
            if output.is_some() && signals.state.load(Ordering::Acquire) == RUNNING {
                signals.quarantine();
            }
            if let Some(p2_future) = p2.as_mut().as_pin_mut() {
                if let Poll::Ready(completion) = p2_future.poll(cx) {
                    // End every old poll and loan before the completion may
                    // publish Joined to P1 or authorize the next installation.
                    p2.as_mut().set(None);
                    if let Some(completion) = completion {
                        completion.unregistered();
                    }
                    registered = false;
                    if let Some(body_future) = body.as_mut().as_pin_mut() {
                        if let Poll::Ready(result) = body_future.poll(cx) {
                            output = Some(result);
                            body.as_mut().set(None);
                        }
                    }
                }
            }
            if !registered {
                if let Some(output) = output.take() {
                    return Poll::Ready(output);
                }
            }
            Poll::Pending
        })
        .await
    })
}

#[cfg(test)]
mod tests;
