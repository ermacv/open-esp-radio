//! Caller-slot frontend supervision and consuming body transitions.
//! The slot and its P2 registration own hardware, progress and cleanup.
//! A frontend only publishes requests and joins bounded lifecycle answers.

use crate::slot::Signals;
use crate::{
    ClockError, EventEnd, EventsLost, Joined, Poisoned, PortResult, SlotBody, SlotBodyOwner,
    SlotFrontend, SlotIdentity, SlotQuarantined,
};
use core::{
    fmt::Debug,
    future::Future,
    marker::PhantomData,
    ops::{Deref, DerefMut},
};
use oer_time::RadioInstant;
/// Authority to synchronously quarantine an installation.
#[must_use = "quarantine the installation and return its outcome"]
pub struct Quarantine {
    private: (),
}
impl Quarantine {
    pub(crate) const fn new() -> Self {
        Self { private: () }
    }

    /// Confirm hardware stopping after admission was closed, routes masked
    /// and every entered driver step and ISR left within the bound. The
    /// implementation must also establish its device-specific stop condition.
    /// This does not establish DMA backing reclamation permission.
    pub fn stopped(self) -> HardwareStopped {
        let Self { private: () } = self;
        HardwareStopped { _private: () }
    }
}

/// Proof that bounded CPU exclusion and the hardware stop both completed.
/// Only the backend's quarantine authority can issue this affine proof.
#[must_use = "retain the stop proof until final detachment"]
pub struct HardwareStopped {
    _private: (),
}

/// Result of synchronous quarantine, including what was proved about hardware.
/// Neither outcome permits backing storage to be reused.
#[must_use = "retain the outcome through final detachment"]
pub enum Abandon {
    /// Every entered step/ISR left within the bound and hardware stopping was
    /// confirmed by the implementation's device-specific condition.
    Stopped(HardwareStopped),
    /// The installation is poisoned, but stopping and interrupt exclusion
    /// are not proved. Hardware and entered operations may still run. Every
    /// reachable allocation remains quarantined until chip reset. Consumes
    /// the call authority without manufacturing a masking or stop proof.
    Unconfirmed(Quarantine),
}

/// Disabled transition authority; lends setup and permits ordinary uninstall.
pub enum Disabled {}
/// Enabled transition authority; lends phases and must disable before uninstall.
pub enum Enabled {}
/// Affine authority issued by the frontend lifecycle for one backend call.
pub struct Call {
    _private: (),
}
impl Call {
    fn new() -> Self {
        Self { _private: () }
    }
}
/// Enable preserves the disabled owner on refusal and closes it on poison.
pub type EnableResult<R, F> = PortResult<(), R, F>;

/// The phase's unique, cancellation-safe event consumer.
pub trait PortPhase {
    /// Owned event of the body loan, including admitted terminals.
    type Event;
    /// Published cause of the final event-stream end.
    type Fault: Copy + Debug;
    /// Receive in occurrence order. Cancelling this wait loses no event.
    /// Every call after the final end returns the same end and cause.
    fn next_event(
        &mut self,
    ) -> impl Future<Output = Result<Result<Self::Event, EventsLost>, EventEnd<Self::Fault>>> + '_;
}

/// Body operations over queue views of a caller-slot P2 owner.
/// Hardware, driver progress, owned payloads and cleanup remain in `SlotOwner`.
pub trait PortBackend<Id: SlotIdentity>: SlotBodyOwner<Id> {
    /// Coordinate domain of the backend's radio clock.
    type Domain;
    /// Published fault cause, independent of resource ownership.
    type Fault: Copy + Debug;
    /// Enable refusal, which changes no disabled authority.
    type Refusal;
    /// Public event whose resources retain the invariant body loan.
    type Event<'p>
    where
        Self: 'p,
        Id: 'p;
    /// Admitted owners returned by disable, without unsolicited RX.
    type Terminal<'p>
    where
        Self: 'p,
        Id: 'p;
    /// Body-local accounting and short queue views; contains no P2 owner.
    type View<'p>
    where
        Self: 'p,
        Id: 'p;
    /// Resources lent once under the body's invariant loan.
    type Resources<'p>
    where
        Self: 'p,
        Id: 'p;
    /// Disabled-only settings, borrowed until the next transition.
    type Setup<'a, 'p>
    where
        Self: 'p,
        Id: 'p,
        'p: 'a;
    /// Running phase, separately borrowed from its body resource loan.
    type Phase<'a, 'p>: PortPhase<Event = Self::Event<'p>, Fault = Self::Fault>
    where
        Self: 'p,
        Id: 'p,
        'p: 'a;
    /// Owned uninstall command, containing no P1-frame reference or hardware.
    type Uninstall;
    /// Decompose this backend's public body views into lifecycle parts.
    fn body_parts<'p>(body: Self::Body<'p>) -> InstallParts<'p, Id, Self>
    where
        Self: 'p,
        Id: 'p;
    /// Enable within the backend's named transition bound.
    fn enable<'a, 'p>(
        call: Call,
        view: &'a mut Self::View<'p>,
    ) -> impl Future<Output = EnableResult<Self::Refusal, Self::Fault>> + 'a
    where
        Self: 'p,
        Id: 'p,
        'p: 'a;
    /// End admitted work within the bound and return its terminals.
    /// A rejecting sink returns its terminal by value; retain it and poison.
    /// The concrete sink remains exclusively borrowed until the future ends.
    fn disable<'a, 'p, S>(
        call: Call,
        view: &'a mut Self::View<'p>,
        sink: &'a mut S,
    ) -> impl Future<Output = Result<(), Poisoned<Self::Fault>>> + 'a
    where
        Self: 'p,
        Id: 'p,
        'p: 'a,
        S: FnMut(Self::Terminal<'p>) -> core::ops::ControlFlow<Self::Terminal<'p>>;
    /// Read the radio clock through the frontend without changing admission.
    fn now<'a, 'p>(
        call: Call,
        view: &'a Self::View<'p>,
    ) -> impl Future<Output = PortResult<RadioInstant<Self::Domain>, ClockError, Self::Fault>> + 'a
    where
        Self: 'p,
        Id: 'p,
        'p: 'a;
    /// Borrow disabled settings without accessing the P2 owner.
    fn setup<'a, 'p>(call: Call, view: &'a mut Self::View<'p>) -> Self::Setup<'a, 'p>
    where
        Self: 'p,
        Id: 'p,
        'p: 'a;
    /// Lend one running phase from the body-local view.
    fn phase<'a, 'p>(call: Call, view: &'a mut Self::View<'p>) -> Self::Phase<'a, 'p>
    where
        Self: 'p,
        Id: 'p,
        'p: 'a;
    /// Publish the normal uninstall control after all body views have ended.
    /// Lifecycle command capacity is reserved; this does not await P2.
    fn request_uninstall<'s>(
        raw: &mut Self::Frontend<'s>,
        control: Self::Uninstall,
    ) -> Result<(), Poisoned<Self::Fault>>
    where
        Self: 's,
        Id: 's;
    /// Create the named stop/join deadline, armed on its first poll.
    /// The future cannot borrow the frontend value; backing `'s` outlives it.
    fn shutdown_bound<'s>(raw: &Self::Frontend<'s>) -> impl Future<Output = ()> + use<'s, Id, Self>
    where
        Self: 's,
        Id: 's;
}

/// Body-only views from one backend construction authority.
pub struct InstallParts<'p, Id: SlotIdentity + 'p, B: PortBackend<Id> + 'p> {
    /// Frontend accounting and short queue loans.
    pub view: B::View<'p>,
    /// The complete public resource set, issued once for this body.
    pub resources: B::Resources<'p>,
}

/// A role body whose result is fixed before its invariant loan is introduced.
pub trait PortBody<Id: SlotIdentity, B: PortBackend<Id>> {
    /// Result with no borrowed body resource, even for a static Id.
    type Output;
    /// Run disabled-to-ended; fail-stop only publishes a typed request.
    fn run<'a, 'p>(
        self,
        owner: Owner<'a, 'p, Id, B, Disabled>,
        resources: B::Resources<'p>,
    ) -> impl Future<Output = Ended<Id, B, Self::Output>>
    where
        B: 'p,
        Id: 'p,
        'p: 'a;
}

/// A frontend result containing no backend, driver, held state or P2 guard.
#[must_use = "retain the outcome; only Joined permits a P2 reinstall"]
pub enum PortEnd<Id: SlotIdentity, Out> {
    /// P2 stopped, detached, destroyed its registration and retained vacant hardware.
    Uninstalled {
        /// The slot's unique ordinary reuse authority.
        joined: Joined<Id>,
        /// Body's result after all of its loans ended.
        output: Out,
    },
    /// Body requested cleanup or ordinary retirement missed its deadline.
    FailStopped {
        /// Actual timely P2 stopping proof, or unconfirmed quarantine.
        outcome: SlotQuarantined<Id>,
        /// The returned body result, carrying only its request at fail-stop.
        output: Out,
    },
    /// The owner's drop cancelled a body that had no ended result.
    Abandoned {
        /// Construction or cancellation error that ended the body loan.
        cause: crate::SlotInitError,
        /// Quarantine notification; grants neither extraction nor reuse.
        outcome: SlotQuarantined<Id>,
    },
}

/// Supervise a body and one bounded P2 answer through its frontend.
/// Registration already belongs to P2 before this future is constructed.
/// Cancellation only requests quarantine; the P2 runner retains cleanup.
pub fn run_port<'s, Id: SlotIdentity + 's, B: PortBackend<Id> + 's, P: PortBody<Id, B>>(
    mut frontend: SlotFrontend<'s, Id, B>,
    body: P,
) -> impl Future<Output = PortEnd<Id, P::Output>> {
    struct BodyAdapter<'s, P> {
        signals: &'s Signals,
        body: P,
    }
    impl<Id: SlotIdentity, B: PortBackend<Id>, P: PortBody<Id, B>> SlotBody<Id, B>
        for BodyAdapter<'_, P>
    {
        type Output = Ended<Id, B, P::Output>;
        async fn run<'p>(self, parts: B::Body<'p>) -> Self::Output
        where
            B: 'p,
            Id: 'p,
        {
            let InstallParts {
                mut view,
                resources,
            } = B::body_parts(parts);
            self.body
                .run(Owner::new(&mut view, self.signals), resources)
                .await
        }
    }
    // Construct the bound without borrowing raw frontend state across a body
    // loan. Only this lifecycle supervisor waits for a stop or join answer.
    let bound = B::shutdown_bound(frontend.ports());
    let signals = frontend.lifecycle_signal();
    async move {
        // End the body's future and its temporary Ended before retaining
        // the one output across the supervisor's single bounded wait.
        let Ended { end, output, .. } = match frontend.run_body(BodyAdapter { signals, body }).await
        {
            Ok(ended) => ended,
            Err(cause) => {
                let outcome = frontend.abandon().wait_failure_within(bound).await;
                return PortEnd::Abandoned { outcome, cause };
            }
        };
        let result = match end {
            BodyEnd::Uninstall(control) => {
                if B::request_uninstall(frontend.ports(), control).is_ok() {
                    frontend.finish().wait_within(bound).await
                } else {
                    Err(frontend.abandon().wait_failure_within(bound).await)
                }
            }
            BodyEnd::FailStopRequested => Err(frontend.abandon().wait_failure_within(bound).await),
        };
        match result {
            Ok(joined) => PortEnd::Uninstalled { joined, output },
            Err(outcome) => PortEnd::FailStopped { outcome, output },
        }
    }
}

enum BodyEnd<C> {
    Uninstall(C),
    FailStopRequested,
}
/// Consumed body authority and its result, independent of the body loan.
#[must_use = "return this value to the lifecycle supervisor"]
pub struct Ended<Id: SlotIdentity, B: PortBackend<Id>, Out> {
    end: BodyEnd<B::Uninstall>,
    output: Out,
    identity: PhantomData<fn(Id) -> Id>,
}
/// Affine notification that cleanup was requested; contains no stop outcome.
#[must_use = "forward the request through Ended::fail_stop"]
pub struct FailStopRequested<Id: SlotIdentity, B: PortBackend<Id>> {
    identity: PhantomData<fn(Id, B) -> (Id, B)>,
}
impl<Id: SlotIdentity, B: PortBackend<Id>, Out> Ended<Id, B, Out> {
    /// Consume disabled or poisoned authority and publish normal uninstall on return.
    pub fn uninstall(
        owner: impl UninstallOwner<Id, B>,
        control: B::Uninstall,
        output: Out,
    ) -> Self {
        owner.disarm();
        Self {
            end: BodyEnd::Uninstall(control),
            output,
            identity: PhantomData,
        }
    }
    /// Forward a synchronous request; the supervisor obtains the actual outcome.
    pub fn fail_stop(request: FailStopRequested<Id, B>, output: Out) -> Self {
        let FailStopRequested { .. } = request;
        Self {
            end: BodyEnd::FailStopRequested,
            output,
            identity: PhantomData,
        }
    }
}
mod sealed {
    pub trait Uninstall {
        fn disarm(self);
    }
}
/// Closed disabled/poisoned authority; enabled owners must first disable.
pub trait UninstallOwner<Id: SlotIdentity, B: PortBackend<Id>>: sealed::Uninstall {}
impl<Id: SlotIdentity, B: PortBackend<Id>> UninstallOwner<Id, B>
    for Owner<'_, '_, Id, B, Disabled>
{
}
impl<Id: SlotIdentity, B: PortBackend<Id>> sealed::Uninstall for Owner<'_, '_, Id, B, Disabled> {
    fn disarm(self) {
        self.sentry.disarm();
    }
}
impl<Id: SlotIdentity, B: PortBackend<Id>> UninstallOwner<Id, B> for PoisonedOwner<'_, '_, Id, B> {}
impl<Id: SlotIdentity, B: PortBackend<Id>> sealed::Uninstall for PoisonedOwner<'_, '_, Id, B> {
    fn disarm(self) {
        self.sentry.disarm();
    }
}
struct Sentry<'a> {
    signals: &'a Signals,
    armed: bool,
}
impl Sentry<'_> {
    fn disarm(mut self) {
        self.armed = false;
    }
}
impl Drop for Sentry<'_> {
    // Stack analysis resolves wake callbacks through their typed Waker scope.
    // Inlining this destructor into future drop glue loses that scope; the
    // resulting hole is rejected when an image requires a proven task stack.
    #[inline(never)]
    fn drop(&mut self) {
        if self.armed {
            self.signals
                .request_failure(crate::LifecycleError::Cancelled);
        }
    }
}

/// The body's unique transition authority, borrowing only its frontend view.
#[must_use = "end through uninstall or a synchronous fail-stop request"]
pub struct Owner<'a, 'p, Id: SlotIdentity + 'p, B: PortBackend<Id> + 'p, State> {
    view: &'a mut B::View<'p>,
    sentry: Sentry<'a>,
    state: PhantomData<State>,
}
impl<'a, 'p, Id: SlotIdentity + 'p, B: PortBackend<Id> + 'p, State> Owner<'a, 'p, Id, B, State> {
    fn new(view: &'a mut B::View<'p>, signals: &'a Signals) -> Self {
        Self {
            view,
            sentry: Sentry {
                signals,
                armed: true,
            },
            state: PhantomData,
        }
    }
    /// Read the backend's radio coordinate without changing admission.
    pub fn now(
        &self,
    ) -> impl Future<Output = PortResult<RadioInstant<B::Domain>, ClockError, B::Fault>> + '_ {
        B::now(Call::new(), self.view)
    }
    /// Publish quarantine and wake P2 immediately, without waiting or hardware access.
    pub fn fail_stop(self) -> FailStopRequested<Id, B> {
        self.sentry
            .signals
            .request_failure(crate::LifecycleError::FailStopRequested);
        self.sentry.disarm();
        FailStopRequested {
            identity: PhantomData,
        }
    }
}
impl<'a, 'p, Id: SlotIdentity + 'p, B: PortBackend<Id> + 'p> Owner<'a, 'p, Id, B, Disabled> {
    /// Lend disabled settings until the next owner transition.
    pub fn setup(&mut self) -> Lent<'_, B::Setup<'_, 'p>> {
        Lent::new(B::setup(Call::new(), self.view))
    }
    /// Consume disabled authority, preserving it on refusal and ending it on poison.
    pub async fn enable(self) -> Result<Owner<'a, 'p, Id, B, Enabled>, Failed<'a, 'p, Id, B>> {
        let Self { view, sentry, .. } = self;
        match B::enable(Call::new(), view).await {
            Ok(Ok(())) => Ok(Owner {
                view,
                sentry,
                state: PhantomData,
            }),
            Ok(Err(refusal)) => Err(Failed::Refused(
                Owner {
                    view,
                    sentry,
                    state: PhantomData,
                },
                refusal,
            )),
            Err(poisoned) => Err(Failed::Poisoned(PoisonedOwner {
                _view: view,
                sentry,
                cause: poisoned.cause,
            })),
        }
    }
}
impl<'a, 'p, Id: SlotIdentity + 'p, B: PortBackend<Id> + 'p> Owner<'a, 'p, Id, B, Enabled> {
    /// Lend the running phase until this owner can transition again.
    pub fn phase(&mut self) -> Lent<'_, B::Phase<'_, 'p>> {
        Lent::new(B::phase(Call::new(), self.view))
    }
    /// Drain admitted owners to the sink within the backend transition bound.
    pub async fn disable<S>(
        self,
        sink: &mut S,
    ) -> Result<Owner<'a, 'p, Id, B, Disabled>, PoisonedOwner<'a, 'p, Id, B>>
    where
        S: FnMut(B::Terminal<'p>) -> core::ops::ControlFlow<B::Terminal<'p>>,
    {
        let Self { view, sentry, .. } = self;
        match B::disable(Call::new(), view, sink).await {
            Ok(()) => Ok(Owner {
                view,
                sentry,
                state: PhantomData,
            }),
            Err(poisoned) => Err(PoisonedOwner {
                _view: view,
                sentry,
                cause: poisoned.cause,
            }),
        }
    }
}
/// Enable refusal preserves disabled authority; poison preserves its cause.
pub enum Failed<'a, 'p, Id: SlotIdentity + 'p, B: PortBackend<Id> + 'p> {
    /// The disabled owner and a refusal that changed no admission.
    Refused(Owner<'a, 'p, Id, B, Disabled>, B::Refusal),
    /// Poisoned frontend authority, with its published cause.
    Poisoned(PoisonedOwner<'a, 'p, Id, B>),
}
impl<Id: SlotIdentity, B: PortBackend<Id>> Failed<'_, '_, Id, B> {
    /// Request fail-stop synchronously from either failed enable outcome.
    pub fn fail_stop(self) -> FailStopRequested<Id, B> {
        match self {
            Self::Refused(owner, _) => owner.fail_stop(),
            Self::Poisoned(owner) => owner.fail_stop(),
        }
    }
}
/// A poisoned body view; its P2 owners remain in the caller slot.
#[must_use = "consume through uninstall or a synchronous fail-stop request"]
pub struct PoisonedOwner<'a, 'p, Id: SlotIdentity + 'p, B: PortBackend<Id> + 'p> {
    _view: &'a mut B::View<'p>,
    sentry: Sentry<'a>,
    cause: B::Fault,
}
impl<Id: SlotIdentity, B: PortBackend<Id>> PoisonedOwner<'_, '_, Id, B> {
    /// The published fault cause.
    pub fn cause(&self) -> B::Fault {
        self.cause
    }
    /// Publish cleanup without waiting for the supervisor's actual answer.
    pub fn fail_stop(self) -> FailStopRequested<Id, B> {
        self.sentry
            .signals
            .request_failure(crate::LifecycleError::FailStopRequested);
        self.sentry.disarm();
        FailStopRequested {
            identity: PhantomData,
        }
    }
}
/// Invariant exclusive phase/setup loan which never gives away its view.
pub struct Lent<'a, T> {
    part: T,
    loan: PhantomData<fn(&'a ()) -> &'a ()>,
}
impl<T> Lent<'_, T> {
    fn new(part: T) -> Self {
        Self {
            part,
            loan: PhantomData,
        }
    }
}
impl<T> Deref for Lent<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.part
    }
}
impl<T> DerefMut for Lent<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.part
    }
}

/// Disjoint components of a phase, borrowed through the owner's `Lent`.
pub struct PhaseParts<E, C, K> {
    /// The single event consumer.
    pub events: E,
    /// Radio-wide control.
    pub control: C,
    /// Interface clients, whose work borrows them.
    pub clients: K,
}

impl<E: PortPhase, C, K> PortPhase for PhaseParts<E, C, K> {
    type Event = E::Event;
    type Fault = E::Fault;
    fn next_event(
        &mut self,
    ) -> impl Future<Output = Result<Result<Self::Event, EventsLost>, EventEnd<Self::Fault>>> + '_
    {
        self.events.next_event()
    }
}

#[cfg(test)]
mod tests;
