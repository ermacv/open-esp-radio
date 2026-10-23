//! Affine construction and permanent ownership of a registered P2 side.

use core::{
    future::{Future, poll_fn},
    marker::PhantomData,
    mem::ManuallyDrop,
    pin::{Pin, pin},
    sync::atomic::{AtomicU8, Ordering},
    task::{Context, Poll},
};

use atomic_waker::AtomicWaker;
use oer_sync::PriorityWaker;

use crate::{Abandon, LocalId, Quarantine, SlotIdentity};

/// The registered P2 state and its separate queue storage.
/// None of its operations run on the frontend.
pub trait SlotOwner<Id: SlotIdentity>: Sized {
    /// Capacity layout issued once per joined installation.
    type Pools: crate::PoolSet;
    /// Fixed queue backing, retained with the slot on every abnormal path.
    /// Normal retirement drains all payload owners before returning its proof.
    type Queues;
    /// Const backing retained in place across every joined installation.
    const VACANT_QUEUES: Self::Queues;
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
    fn prepare_queues(
        &mut self,
        queues: &mut Self::Queues,
        mint: crate::Mint<'static, Self::Pools, Id>,
    );

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

    /// Retain stopped hardware on P2 without moving this owner.
    /// The registration then destroys `self` in place before publishing join.
    fn finish<'s>(&mut self, invocation: &mut Self::Invocation<'s>) -> Self::Vacant
    where
        Self: 's,
        Id: 's;
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
    /// Report a P2 invariant failure without issuing an ordinary stop proof.
    /// Yield after this request; the registration ends the driver loan and
    /// invokes its own quarantine guard before publishing the answer.
    pub fn request_quarantine(&self) {
        self.signals.quarantine();
    }

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
    #[inline(never)]
    pub const fn place<S: SlotOwner<Id>>(self) -> PortSlot<Id, S> {
        PortSlot {
            owner: ManuallyDrop::new(None),
            queues: ManuallyDrop::new(S::VACANT_QUEUES),
            signals: Signals::new(),
            identity: PhantomData,
        }
    }

    /// Activate preinitialized backing without moving its storage.
    /// The invariant Id and consumed claim authorize exactly this one slot.
    pub fn place_in<S: SlotOwner<Id>>(
        self,
        backing: &mut VacantPortSlot<Id, S>,
    ) -> &mut PortSlot<Id, S> {
        &mut backing.slot
    }
}

/// Preinitialized storage with no authority to register or publish endpoints.
/// A consumed slot claim activates it through `SlotClaim::place_in`.
/// Static adapters can const-evaluate this value inside their one-time cell
/// initializer; no runtime constructor returns the large backing by value.
///
/// Unclaimed backing cannot publish a frontend without consuming a claim:
/// ```compile_fail,E0599
/// use oer_radio_port::{SlotIdentity, SlotOwner, VacantPortSlot};
/// fn publish<Id: SlotIdentity, S: SlotOwner<Id>>(
///     backing: &mut VacantPortSlot<Id, S>, owner: S,
/// ) {
///     let _ = backing.register(owner);
/// }
/// ```
pub struct VacantPortSlot<Id: SlotIdentity, S: SlotOwner<Id>> {
    slot: PortSlot<Id, S>,
}

impl<Id: SlotIdentity, S: SlotOwner<Id>> VacantPortSlot<Id, S> {
    /// Build unclaimed storage, without creating any capacity or endpoint.
    pub const fn vacant() -> Self {
        Self {
            slot: PortSlot {
                owner: ManuallyDrop::new(None),
                queues: ManuallyDrop::new(S::VACANT_QUEUES),
                signals: Signals::new(),
                identity: PhantomData,
            },
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

pub(crate) struct Signals {
    state: AtomicU8,
    answer: AtomicU8,
    p2: PriorityWaker,
    join: AtomicWaker,
}

impl Signals {
    const fn new() -> Self {
        Self {
            state: AtomicU8::new(INITIAL),
            answer: AtomicU8::new(NO_ANSWER),
            p2: PriorityWaker::new(),
            join: AtomicWaker::new(),
        }
    }

    // Keep the typed Waker scopes outside the backend's generated poll frame
    // so stack analysis can resolve both quarantine notifications.
    #[inline(never)]
    pub(crate) fn quarantine(&self) {
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
    owner: ManuallyDrop<Option<Stored<Id, S>>>,
    queues: ManuallyDrop<S::Queues>,
    signals: Signals,
    identity: PhantomData<fn(Id) -> Id>,
}

enum Stored<Id: SlotIdentity, S: SlotOwner<Id>> {
    Registered(S),
    Vacant(Option<<S as SlotOwner<Id>>::Vacant>),
}

fn registered_owner<Id: SlotIdentity, S: SlotOwner<Id>>(
    storage: &mut Option<Stored<Id, S>>,
) -> &mut S {
    match storage.as_mut().expect("registration retains its P2 owner") {
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
        let Some(Stored::Vacant(vacant)) = self.owner.as_mut() else {
            unreachable!("ordinary join retains vacant state");
        };
        let vacant = vacant.take().expect("join retains its stopped hardware");
        // Queue storage stays at its permanent address. The backend prepares
        // that empty backing in place after all old endpoint loans ended.
        let owner = resume(vacant);
        Ok(self.install(owner))
    }

    fn install<'s>(&'s mut self, owner: S) -> (SlotFrontend<'s, Id, S>, SlotRegistration<'s, Id, S>)
    where
        S: 's,
        Id: 's,
    {
        self.owner = ManuallyDrop::new(Some(Stored::Registered(owner)));
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
        registered_owner(registration.owner).prepare_queues(
            &mut self.queues,
            crate::Mint::<'static, S::Pools, Id>::new(),
        );
        let (ports, invocation) = S::split(
            &mut self.queues,
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
                body_issued: false,
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
        let mut queues = ManuallyDrop::into_inner(self.queues);
        if let Some(owner) = ManuallyDrop::into_inner(self.owner) {
            match owner {
                Stored::Registered(mut owner) => owner.end_world(Some(&mut queues)),
                Stored::Vacant(Some(mut vacant)) => S::end_vacant_world(&mut vacant),
                Stored::Vacant(None) => unreachable!("vacant hardware retained until reinstall"),
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
    body_issued: bool,
    identity: PhantomData<fn(Id) -> Id>,
}

impl<'s, Id: SlotIdentity + 's, S: SlotOwner<Id> + 's> SlotFrontend<'s, Id, S> {
    pub(crate) fn lifecycle_signal(&self) -> &'s Signals {
        self.signals
    }

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
    /// Each registration lends its one initial resource set at most once;
    /// another attempt quarantines without calling the backend constructor.
    pub async fn run_body<P: SlotBody<Id, S>>(&mut self, body: P) -> Option<P::Output>
    where
        S: SlotBodyOwner<Id>,
    {
        let signals = self.signals;
        if signals.state.load(Ordering::Acquire) == QUARANTINED {
            return None;
        }
        if self.body_issued {
            signals.quarantine();
            return None;
        }
        self.body_issued = true;
        let views = S::lend(self.ports(), crate::BodyLoan::new());
        let mut body = pin!(body.run(views));
        poll_fn(|cx| {
            signals.join.register(cx.waker());
            if signals.state.load(Ordering::Acquire) == QUARANTINED {
                return Poll::Ready(None);
            }
            match poll_body(body.as_mut(), cx) {
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
    owner: &'s mut Option<Stored<Id, S>>,
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
                let result = stopping.as_mut().poll(cx);
                if signals.state.load(Ordering::Acquire) == QUARANTINED {
                    Poll::Ready(None)
                } else {
                    result.map(Some)
                }
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
        finish_owner(self.owner, &mut self.invocation);
        Some(completion)
    }
}

// Keep destruction outside the runner's poll frame. Assignment through the
// Option drops the registered state at its permanent address; only the small
// stopped hardware owner is returned by the backend.
#[inline(never)]
fn finish_owner<'s, Id: SlotIdentity + 's, S: SlotOwner<Id> + 's>(
    owner: &mut Option<Stored<Id, S>>,
    invocation: &mut Option<S::Invocation<'s>>,
) {
    let vacant = registered_owner(owner).finish(
        invocation
            .as_mut()
            .expect("ordinary completion retains its P2 invocation"),
    );
    drop(invocation.take());
    *owner = None;
    *owner = Some(Stored::Vacant(Some(vacant)));
}

// Separate stack frames for P1 and P2 while keeping both futures in their
// caller's pinned backing. These helpers require no pin projection or unsafe.
#[inline(never)]
fn poll_body<F: Future>(future: Pin<&mut F>, cx: &mut Context<'_>) -> Poll<F::Output> {
    future.poll(cx)
}

#[inline(never)]
fn poll_registration<F: Future>(future: Pin<&mut F>, cx: &mut Context<'_>) -> Poll<F::Output> {
    future.poll(cx)
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

pin_project_lite::pin_project! {
    #[project = LocalBodyProject]
    enum LocalBody<Body: Future> {
        Running { #[pin] future: Body },
        Returned { output: Option<Body::Output> },
    }
}

pin_project_lite::pin_project! {
    struct LocalRun<'s, Id: SlotIdentity, Body: Future, P2> {
        // Field order also ends a never-polled P1 future before its P2 guard.
        #[pin]
        body: LocalBody<Body>,
        #[pin]
        p2: Option<P2>,
        signals: &'s Signals,
        registered: bool,
        identity: PhantomData<fn(Id) -> Id>,
    }
}

fn poll_local_body<Body: Future>(mut body: Pin<&mut LocalBody<Body>>, cx: &mut Context<'_>) {
    if let LocalBodyProject::Running { future } = body.as_mut().project() {
        if let Poll::Ready(output) = poll_body(future, cx) {
            // Drop the completed future in place, then reuse its storage for
            // the output while P2 ends. No second body or output allocation.
            body.set(LocalBody::Returned {
                output: Some(output),
            });
        }
    }
}

impl<'s, Id: SlotIdentity, Body: Future, P2> Future for LocalRun<'s, Id, Body, P2>
where
    P2: Future<Output = Option<SlotCompletion<'s, Id>>>,
{
    type Output = Body::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut this = self.project();
        poll_local_body(this.body.as_mut(), cx);
        if matches!(this.body.as_ref().get_ref(), LocalBody::Returned { .. })
            && this.signals.state.load(Ordering::Acquire) == RUNNING
        {
            // Returning after forgetting the frontend cannot leave P2 live.
            this.signals.quarantine();
        }
        if let Some(p2) = this.p2.as_mut().as_pin_mut() {
            if let Poll::Ready(completion) = poll_registration(p2, cx) {
                this.p2.set(None);
                if let Some(completion) = completion {
                    completion.unregistered();
                }
                *this.registered = false;
                poll_local_body(this.body.as_mut(), cx);
            }
        }
        if !*this.registered {
            if let LocalBodyProject::Returned { output } = this.body.project() {
                return Poll::Ready(output.take().expect("local runner returns once"));
            }
        }
        Poll::Pending
    }
}

pub(crate) fn run_registered<'s, Id: SlotIdentity + 's, S: SlotOwner<Id> + 's, R>(
    frontend: SlotFrontend<'s, Id, S>,
    registration: SlotRegistration<'s, Id, S>,
    body: impl AsyncFnOnce(SlotFrontend<'s, Id, S>) -> R,
) -> impl Future<Output = R> {
    let signals = frontend.signals;
    LocalRun {
        body: LocalBody::Running {
            future: body(frontend),
        },
        p2: Some(registration.run()),
        signals,
        registered: true,
        identity: PhantomData,
    }
}

#[cfg(test)]
mod tests;
