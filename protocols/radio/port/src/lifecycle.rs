//! One installation, one body and one independently owned driver.
//!
//! `Installed` moves its backend and driver into `run_port` once. The static
//! `drive` signature consumes a closed affine `DriverLoan` and its retire
//! receiver; its future borrows the driver without borrowing the backend.
//! Both owners remain in `run_port`. The backend is lent by
//! `&mut` to the body's owner. Consuming transitions transfer that same loan.
//!
//! The driver has no installation lifetime. All branded queues (`Slot<'id>`,
//! in-flight owners and their terminal capacity) live in `Held<'id>`; `drive`
//! receives a shared borrow. `Local<'id>` belongs only to the body. The phase
//! and setup are invariant loans, and cannot outlive an owner transition.
//! Hardware-reachable backing has static storage and never resides inline
//! in the movable driver or its future. Cancelling the drive future ends
//! only its loan. Quarantine retains the backend, driver and completion
//! state without running any of their destructors, even if its receipt is
//! dropped; only ordinary uninstall receives reusable owners.
//!
//! A driver returns either a retirement proof obtained after the core's
//! request, or a poison proof obtained by publishing a fault. A faulted
//! driver's state is retained while the body drains earlier results and
//! returns `Ended`. Uninstall consumes `Retired` and `Detached` of the same
//! installation. A missed retirement deadline is a fail-stop.
//!
//! Dropping an owner wakes the core through `AbandonLatch`. The core drops
//! the body, poisons the installation through `abandon`, requests asynchronous
//! retirement, and detaches after the driver ends. Cancelling `run_port`
//! poisons synchronously as a safety net; it cannot await teardown.
//! Forgetting an owner cannot construct `Ended`: the core still owns the
//! backend, and cancellation still abandons it. Neither path needs a panic.

use core::{
    fmt::Debug,
    future::{Future, poll_fn},
    marker::PhantomData,
    mem::ManuallyDrop,
    ops::{Deref, DerefMut},
    pin::{Pin, pin},
    sync::atomic::{AtomicBool, AtomicU8, Ordering},
    task::{Context, Poll},
};

use atomic_waker::AtomicWaker;
use oer_time::RadioInstant;
use scopeguard::{ScopeGuard, guard};

use crate::pool::prepare_brand;
use crate::{ClockError, EventEnd, EventsLost, Mint, Poisoned, PoolSet, PortResult};

type Brand<'id> = PhantomData<fn(&'id ()) -> &'id ()>;

/// A disabled installation, which lends settings and can be uninstalled.
pub enum Disabled {}
/// An enabled installation, which lends a phase and can be disabled.
pub enum Enabled {}

/// Enable's refusal preserves the disabled owner; poisoning ends admission.
pub type EnableResult<R, F> = PortResult<(), R, F>;

/// Authority for one backend call, issued only by the lifecycle core.
#[must_use = "a call authority is consumed by its backend operation"]
pub struct Call {
    _private: (),
}

impl Call {
    const fn new() -> Self {
        Self { _private: () }
    }
}

/// Authority to synchronously quarantine an installation.
#[must_use = "quarantine the installation and return its outcome"]
pub struct Quarantine {
    private: (),
}
impl Quarantine {
    const fn new() -> Self {
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

/// Owners permanently withheld from reuse until the chip is reset.
///
/// There is no accessor or extraction operation. Dropping this value does
/// not drop `S`, free its backing allocations or run hardware teardown.
/// This is a quarantine receipt, not permission to recover the owners.
/// Hardware-reachable storage must already have a durable address and
/// lifetime: this wrapper cannot extend a borrow or preserve inline memory
/// after its containing allocation ends.
#[must_use = "quarantined owners require a chip reset before reuse"]
pub struct Quarantined<S> {
    _retained: ManuallyDrop<S>,
}

impl<S> Quarantined<S> {
    /// Permanently retain owners without invoking their destructors.
    pub const fn new(state: S) -> Self {
        Self {
            _retained: ManuallyDrop::new(state),
        }
    }
}

/// Authority for final detachment, after retirement or quarantine.
#[must_use = "consume this authority in the single detach call"]
pub enum Detach<'q> {
    /// The driver ended and the core is performing an ordinary uninstall.
    Retired(Call),
    /// Quarantine already ran; its outcome states which effects were proved.
    Quarantined {
        /// The core's single detachment authority.
        call: Call,
        /// The outcome retained for the caller after this call.
        outcome: &'q Abandon,
    },
}

/// The core's single affine loan of its independently owned driver.
///
/// Only `run_port` constructs this value. Consuming it starts `drive` once;
/// dropping that future ends the loan without dropping the driver. Shared
/// access exposes the driver's operations without an extraction operation or
/// mutable reference that could replace the whole driver. Retirement consumes
/// this loan through `Retire::requested`; a fault consumes it through
/// `fail_stop`. Neither completion proof leaves access to the driver behind.
#[must_use = "consume this loan in the installation's single drive call"]
pub struct DriverLoan<'p, D> {
    driver: &'p mut D,
}

impl<D> DriverLoan<'_, D> {
    /// End driver access after publishing poison. The publication and this
    /// consumed loan together prove a faulted exit; neither alone suffices.
    pub fn fail_stop(self, published: PublishedPoison) -> PoisonProof {
        let PublishedPoison { private: () } = published;
        PoisonProof { private: () }
    }
}

impl<D> Deref for DriverLoan<'_, D> {
    type Target = D;
    fn deref(&self) -> &D {
        self.driver
    }
}

/// An installed backend and its independent, unbranded driver.
///
/// The private fields can only be consumed by `run_port`; there is no
/// extraction or second entry. The constructor does not execute the driver.
#[must_use = "run the installed port to return its hardware owners"]
pub struct Installed<B: PortBackend> {
    backend: B,
    driver: B::Driver,
}

impl<B: PortBackend> Installed<B> {
    /// Pair a freshly installed, disabled backend with its driver.
    pub const fn new(backend: B, driver: B::Driver) -> Self {
        Self { backend, driver }
    }
}

/// Named owners produced by the installation's single capacity mint.
pub struct InstallParts<'id, B: PortBackend> {
    /// Queues, signals and branded in-flight state shared with the driver.
    pub held: B::Held<'id>,
    /// Accounting reached only through the body's exclusive owner.
    pub local: B::Local<'id>,
    /// Capacity and configuration owners passed once to the body.
    pub resources: B::Resources<'id>,
}

/// A backend's operations, invoked only with the core's affine authority.
pub trait PortBackend: Sized {
    /// The complete capacity layout, issued once by `Mint::into_pools`.
    type Pools: PoolSet;
    /// An owned event of this installation.
    type Event<'id>;
    /// Owners returned by admitted work, delivered by disable's sink.
    type Terminal<'id>;
    /// The domain of the radio clock.
    type Domain;
    /// A published cause of poisoning.
    type Fault: Copy + Debug;
    /// A radio refusal of enable, leaving the port disabled.
    type Refusal;
    /// Settings available while disabled.
    type Setup<'a, 'id>
    where
        Self: 'a,
        'id: 'a;
    /// One phase under the owner's exclusive loan.
    type Phase<'a, 'id>: PortPhase<Event = Self::Event<'id>, Fault = Self::Fault>
    where
        Self: 'a,
        'id: 'a;
    /// Capacity handed once to the body.
    type Resources<'id>;
    /// Shared queues and signals, including every branded driver resource.
    type Held<'id>;
    /// State owned only by the body's side of the installation.
    type Local<'id>;
    /// The driver retained by the core and lent to `drive` once. It has no
    /// installation brand. All hardware-reachable backing resides in static
    /// storage, never inline in this movable owner or its drive future.
    type Driver;
    /// Unbranded completion state returned beside the retained driver.
    type State;
    /// The caller's uninstall control.
    type Uninstall;
    /// Hardware owners, or typed retained owners, returned by uninstall.
    type Uninstalled;

    /// Enable within the backend's documented bound, borrowing this
    /// installation's shared state and its exclusive body-local accounting.
    fn enable<'a, 'id>(
        &'a mut self,
        call: Call,
        held: &'a Self::Held<'id>,
        local: &'a mut Self::Local<'id>,
    ) -> impl Future<Output = EnableResult<Self::Refusal, Self::Fault>> + 'a
    where
        'id: 'a;

    /// End admitted work within the backend's bound and deliver every
    /// remaining terminal to `sink`, returning all permits before success.
    /// On `ControlFlow::Break(terminal)`, stop delivery, retain that terminal
    /// and every remaining owner in the installation, and return `Poisoned`.
    /// Uninstall must return those owners or quarantine them without Drop.
    fn disable<'a, 'id>(
        &'a mut self,
        call: Call,
        held: &'a Self::Held<'id>,
        local: &'a mut Self::Local<'id>,
        sink: &'a mut dyn FnMut(Self::Terminal<'id>) -> core::ops::ControlFlow<Self::Terminal<'id>>,
    ) -> impl Future<Output = Result<(), Poisoned<Self::Fault>>> + 'a
    where
        'id: 'a;

    /// Read the radio clock without changing admission.
    fn now(
        &self,
        call: Call,
    ) -> impl Future<Output = PortResult<RadioInstant<Self::Domain>, ClockError, Self::Fault>> + '_;

    /// Consume the one mint and distribute its complete layout.
    fn resources<'id>(&mut self, mint: Mint<'id, Self::Pools>) -> InstallParts<'id, Self>;

    /// Lend disabled-only settings, including shared installation state.
    fn setup<'a, 'id>(
        &'a mut self,
        call: Call,
        held: &'a Self::Held<'id>,
        local: &'a mut Self::Local<'id>,
    ) -> Self::Setup<'a, 'id>
    where
        'id: 'a;

    /// Lend the running phase, whose operations borrow their phase.
    fn phase<'a, 'id>(
        &'a mut self,
        call: Call,
        held: &'a Self::Held<'id>,
        local: &'a mut Self::Local<'id>,
    ) -> Self::Phase<'a, 'id>
    where
        'id: 'a;

    /// Drive the installation until retirement or published poisoning.
    ///
    /// This static signature consumes the single affine driver loan and
    /// retire receiver. The future cannot borrow the backend or take or
    /// replace the driver. The driver has no `'id`: branded queues and
    /// in-flight owners reside in `held`. At every `await`, the driver must
    /// be in a state that retirement and `abandon` can handle. Every step
    /// must be cancel-safe:
    /// progress and hardware owners remain in the driver or durable backing,
    /// never in future-local owned storage that cancellation could destroy.
    /// Cancelling the core synchronously poisons through `abandon`. Once
    /// cleanup finishes, `Retire::requested` or `DriverLoan::fail_stop`
    /// consumes the loan to construct the exit proof. Access cannot continue
    /// after that handoff. Cancellation tests must cover each suspension point,
    /// then retire/uninstall the retained driver and verify its owners/state.
    ///
    /// Agent implementation choice: the loan exposes only `&Driver`.
    /// Mutable driver progress therefore lives in durable backing under
    /// the backend's audited interior mutability. Cancellation must release
    /// only access to that progress, preserving it for retire and abandon.
    fn drive<'p, 'id>(
        driver: DriverLoan<'p, Self::Driver>,
        held: &'p Self::Held<'id>,
        retire: Retire<'p>,
    ) -> impl Future<Output = DriverExit<Self::State, Self::Fault>> + use<'p, 'id, Self>;

    /// The deadline for a requested retirement, started on its first poll.
    /// The core constructs this future before lending the backend. Precise
    /// capture excludes that borrow, so retirement remains bounded while a
    /// fail-stopped body waits to return its result. Ready means the driver
    /// missed its bound; the core quarantines without uninstalling.
    fn retire_bound(&self, call: Call) -> impl Future<Output = ()> + use<Self>;

    /// Detach interrupt entries synchronously after the driver has ended.
    /// The core issues this authority once per installation. After an
    /// explicit fail-stop, this call waits for the body to release its loan;
    /// `abandon` has already poisoned the installation. A quarantined detach
    /// must not wait without a bound or release hardware-reachable backing.
    /// Only `Stopped` proves masking and hardware stopping.
    fn detach<'id>(&mut self, call: Detach<'_>, held: &Self::Held<'id>);

    /// Consume an installation whose driver ended and entries are detached.
    fn uninstall<'id>(
        self,
        held: Self::Held<'id>,
        local: Self::Local<'id>,
        retired: Retired<'id, (Self::Driver, Self::State)>,
        detached: Detached<'id>,
        control: Self::Uninstall,
    ) -> Self::Uninstalled;

    /// Synchronously poison the installation and refuse new
    /// port work or installation until chip reset. Never wait without a bound,
    /// including acquiring a lock held by an ISR or driver step.
    /// The core guarantees at most one call per installation, including
    /// cancellation after fail-stop or during retirement. Backends need no
    /// memoized outcome or idempotence protocol.
    ///
    /// Return `Stopped` only after closing hardware admission, masking the
    /// installation's IRQ routes, joining entered operations within a finite
    /// bound, and proving the device-specific stop condition. Otherwise return
    /// `Unconfirmed`: it proves neither stopping nor interrupt exclusion.
    /// Backends without a bounded emergency path must use that outcome.
    ///
    /// Every state and allocation that hardware or an entered operation can
    /// still reach must remain durable and permanently quarantined, regardless
    /// of dropping or forgetting the body, backend or driver. Interrupts must
    /// never reach branded `held` data whose lifetime ends here. Even confirmed
    /// stopping does not authorize DMA backing reuse; recovery needs its own
    /// independently established hardware proof.
    ///
    /// Called immediately by `fail_stop`, or by the core when an owner is
    /// dropped, retirement expires, or `run_port` is cancelled. Only final
    /// detach and result delivery depend on the body's return. An unconfirmed
    /// outcome requires chip reset; it cannot become an ordinary uninstall.
    fn abandon(&mut self, call: Quarantine) -> Abandon;
}

/// One port body, generic over the installation's fresh brand.
///
/// `Output` is chosen before `run` introduces `'id`, so it cannot name that
/// lifetime. Branded owners cannot escape through the body's result.
pub trait PortBody<B: PortBackend> {
    /// An unbranded result returned with the installation's final outcome.
    type Output;

    /// Run disabled-to-ended, returning an uninstall request or fail-stop.
    fn run<'p, 'id>(
        self,
        owner: Owner<'p, 'id, B, Disabled>,
        resources: B::Resources<'id>,
    ) -> impl Future<Output = Ended<'id, B, Self::Output>>;
}

struct Signal {
    set: AtomicBool,
    waiter: AtomicWaker,
}

impl Signal {
    const fn new() -> Self {
        Self {
            set: AtomicBool::new(false),
            waiter: AtomicWaker::new(),
        }
    }

    fn set(&self) {
        self.set.store(true, Ordering::Release);
        self.waiter.wake();
    }

    fn is_set(&self) -> bool {
        self.set.load(Ordering::Acquire)
    }

    fn poll(&self, cx: &mut Context<'_>) -> Poll<()> {
        self.waiter.register(cx.waker());
        if self.is_set() {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }
}

/// An abandoned installation, observed by the core and the driver.
///
/// Each side has its own registered waiter. Registration precedes the
/// acquire check, so setting the latch cannot lose either wake.
/// The first published cause is permanent; cancellation cannot overwrite
/// an explicit fail-stop with a dropped-owner cause.
pub struct AbandonLatch {
    core: Signal,
    driver: Signal,
    // A latched event cause, not a second owner state. Only these two
    // publishers write it, before waking either receiver.
    cause: AtomicU8,
}

impl AbandonLatch {
    const LIVE: u8 = 0;
    const DROPPED: u8 = 1;
    const FAIL_STOPPED: u8 = 2;

    const fn new() -> Self {
        Self {
            core: Signal::new(),
            driver: Signal::new(),
            cause: AtomicU8::new(Self::LIVE),
        }
    }

    fn set(&self) {
        let _ = self.cause.compare_exchange(
            Self::LIVE,
            Self::DROPPED,
            Ordering::Release,
            Ordering::Relaxed,
        );
        self.core.set();
        self.driver.set();
    }

    fn fail_stopped(&self) {
        let _ = self.cause.compare_exchange(
            Self::LIVE,
            Self::FAIL_STOPPED,
            Ordering::Release,
            Ordering::Relaxed,
        );
        self.core.set();
        self.driver.set();
    }

    fn is_fail_stopped(&self) -> bool {
        self.cause.load(Ordering::Acquire) == Self::FAIL_STOPPED
    }

    /// Whether admission must remain closed for this installation.
    pub fn is_set(&self) -> bool {
        self.driver.is_set()
    }

    /// Wait on the driver's single receiver for abandonment.
    pub fn abandoned(&self) -> impl Future<Output = ()> + '_ {
        poll_fn(|cx| self.driver.poll(cx))
    }
}

/// The driver's single receiver of the core's retirement request.
#[must_use = "the driver must observe retirement"]
pub struct Retire<'p> {
    request: &'p Signal,
    latch: &'p AbandonLatch,
}

impl<'p> Retire<'p> {
    /// Whether the core has requested retirement.
    pub fn is_requested(&self) -> bool {
        self.request.is_set()
    }

    /// Wait for the request without consuming the receiver; cancellation
    /// leaves the request latched. There is one driver waiter.
    pub fn wait(&self) -> impl Future<Output = ()> + '_ {
        poll_fn(|cx| self.request.poll(cx))
    }

    /// Consume the receiver and driver access, then prove the core requested
    /// retirement. Driver cleanup precedes this handoff; `wait` lets cleanup
    /// observe the request without yet returning the loan.
    pub fn requested<D>(self, driver: DriverLoan<'p, D>) -> impl Future<Output = RetireProof> {
        // End access when the handoff is called, before its future is polled.
        let DriverLoan { driver: _ } = driver;
        async move {
            self.wait().await;
            RetireProof { private: () }
        }
    }

    /// A driver with no work returns its state after the retirement request.
    /// This keeps protocol models synchronous: the lifecycle core owns the
    /// wait and constructs the same affine proof as `requested`.
    pub async fn retired<D, S, F>(self, driver: DriverLoan<'p, D>, state: S) -> DriverExit<S, F> {
        DriverExit::Retired {
            state,
            proof: self.requested(driver).await,
        }
    }

    /// Whether the owner was abandoned.
    pub fn is_abandoned(&self) -> bool {
        self.latch.is_set()
    }

    /// The driver's latch, including its wakeable abandonment wait.
    pub const fn latch(&self) -> &'p AbandonLatch {
        self.latch
    }
}

/// Proof that the core requested retirement, minted only by `Retire`.
#[must_use = "return the retirement proof with the driver state"]
pub struct RetireProof {
    private: (),
}

/// Proof of published poison and consumed driver access, issued only by
/// `DriverLoan::fail_stop` after publication.
#[must_use = "return the poison proof with a faulted driver"]
pub struct PoisonProof {
    private: (),
}

/// A port's published fault, awaiting the driver's affine handoff.
#[must_use = "consume publication with the driver loan when returning a fault"]
pub struct PublishedPoison {
    private: (),
}

enum Health<C> {
    Healthy,
    Poisoned(Poisoned<C>),
}

/// A port's published poison state. Backends keep this in their existing
/// event/command synchronization and wake those consumers after publishing.
pub struct PoisonState<C> {
    health: Health<C>,
}

impl<C: Copy> PoisonState<C> {
    /// Unpoisoned state for a newly installed port.
    pub const fn new() -> Self {
        Self {
            health: Health::Healthy,
        }
    }

    /// Publish the first cause, returning the stored cause and its affine
    /// publication. `DriverLoan::fail_stop` consumes it and the loan to issue
    /// the exit proof. Later publishers retain that same first cause.
    pub fn publish(&mut self, cause: C) -> (Poisoned<C>, PublishedPoison) {
        let poisoned = match self.health {
            Health::Healthy => Poisoned { cause },
            Health::Poisoned(poisoned) => poisoned,
        };
        self.health = Health::Poisoned(poisoned);
        (poisoned, PublishedPoison { private: () })
    }

    /// Read the cause after draining all events preceding publication.
    pub const fn get(&self) -> Option<Poisoned<C>> {
        match self.health {
            Health::Healthy => None,
            Health::Poisoned(poisoned) => Some(poisoned),
        }
    }
}

impl<C: Copy> Default for PoisonState<C> {
    fn default() -> Self {
        Self::new()
    }
}

/// The only two possible driver exits; neither proof can be forged or copied.
#[must_use = "the core retains the driver's state until teardown"]
pub enum DriverExit<S, F> {
    /// The core requested retirement and the driver returned its owners.
    Retired {
        /// Retained driver state.
        state: S,
        /// The consumed retirement receiver's proof.
        proof: RetireProof,
    },
    /// Poisoning was published; the body can drain results and uninstall.
    Faulted {
        /// Retained driver state, including hardware owners on failure.
        state: S,
        /// The published fault.
        cause: F,
        /// Publication and consumption of driver access precede this exit.
        poisoned: PoisonProof,
    },
}

/// Ended driver state belonging to exactly one installation.
#[must_use = "retired state is consumed by uninstall"]
pub struct Retired<'id, S> {
    state: S,
    brand: Brand<'id>,
}

impl<S> Retired<'_, S> {
    /// Recover the driver's state when implementing uninstall.
    pub fn into_state(self) -> S {
        self.state
    }
}

/// Proof that the core detached this installation's entries once.
#[must_use = "detachment is consumed by uninstall"]
pub struct Detached<'id> {
    brand: Brand<'id>,
}

struct Install<'id, B: PortBackend> {
    held: ManuallyDrop<B::Held<'id>>,
    latch: AbandonLatch,
    retiring: Signal,
}

#[inline(always)]
fn prepare_install<'id, B: PortBackend>(
    backend: &mut B,
    mint: Mint<'id, B::Pools>,
) -> (
    Install<'id, B>,
    ManuallyDrop<B::Local<'id>>,
    B::Resources<'id>,
) {
    let InstallParts {
        held,
        local,
        resources,
    } = backend.resources(mint);
    (
        Install {
            held: ManuallyDrop::new(held),
            latch: AbandonLatch::new(),
            retiring: Signal::new(),
        },
        ManuallyDrop::new(local),
        resources,
    )
}

/// Consume one installation and return its owners and unbranded body output.
/// A faulted driver lets the body drain preceding results and uninstall.
/// Dropping the owner returns `Abandoned` after the core's retirement path.
///
/// `fail_stop` immediately poisons the installation without unbounded waits.
/// Only its `Abandon::Stopped` proof confirms masking and hardware stopping.
/// Retirement and its timer then
/// progress while the body awaits its result; only final detachment and
/// result delivery depend on that body returning.
/// `execution` consumes the single driver invocation; its returned future
/// joins the driver without changing where the body is polled.
pub fn run_port<B: PortBackend, P: PortBody<B>, E: crate::PortExecution<B>>(
    installed: Installed<B>,
    body: P,
    execution: E,
) -> impl Future<Output = PortEnd<B, P::Output>> {
    // Construct the safety net before returning the future: cancellation
    // before its first poll must also poison and retain the installation.
    let installed = guard(installed, |mut installed| {
        let outcome = installed.backend.abandon(Quarantine::new());
        // Cancellation has no result receiver. Retain the outcome together
        // with all owners, applying the same no-Drop quarantine rule.
        drop(Quarantined::new((installed, outcome)));
    });
    prepare_brand(async move |mint| {
        let Installed {
            mut backend,
            driver,
        } = ScopeGuard::into_inner(installed);
        // Declared before the backend guard, so cancellation first poisons the
        // backend and then transfers this retained driver into quarantine.
        let mut driver_owner = guard(driver, |driver| drop(Quarantined::new(driver)));
        let (install, mut local, resources) = prepare_install(&mut backend, mint);
        let mut backend = guard(backend, |backend| drop(Quarantined::new(backend)));
        let bound = backend.retire_bound(Call::new());
        let mut bound = pin!(bound);
        // Consuming this guard after the first abandon leaves only the outer
        // quarantine guard. No backend state or memo is needed to suppress a
        // second abandon while asynchronous retirement is still pending.
        let mut armed = guard(&mut *backend, |backend| {
            abandon_on_cancel(backend, &install.latch);
        });
        let end = {
            let mut driver = pin!(async {
                ManuallyDrop::new(
                    execution
                        .start(crate::PortDriver {
                            loan: DriverLoan {
                                driver: &mut *driver_owner,
                            },
                            held: &install.held,
                            retire: Retire {
                                request: &install.retiring,
                                latch: &install.latch,
                            },
                        })
                        .await,
                )
            });
            let (outcome, driver) = {
                let mut body =
                    pin!(body.run(Owner::new(&mut **armed, &mut local, &install), resources));
                let first = poll_fn(|cx| {
                    if let Poll::Ready(ended) = body.as_mut().poll(cx) {
                        return Poll::Ready(Race::Body(ended));
                    }
                    if install.latch.core.poll(cx).is_ready() {
                        return Poll::Ready(if install.latch.is_fail_stopped() {
                            Race::FailStopped
                        } else {
                            Race::Abandoned
                        });
                    }
                    driver.as_mut().poll(cx).map(Race::Driver)
                })
                .await;
                let (race, driver): (Race<_, core::convert::Infallible>, _) = match first {
                    Race::Driver(exit) => {
                        // The returned state stays in ManuallyDrop while the
                        // body drains results, including cancellation here.
                        let race = poll_fn(|cx| {
                            if let Poll::Ready(ended) = body.as_mut().poll(cx) {
                                return Poll::Ready(Race::Body(ended));
                            }
                            if install.latch.core.poll(cx).is_ready() {
                                return Poll::Ready(if install.latch.is_fail_stopped() {
                                    Race::FailStopped
                                } else {
                                    Race::Abandoned
                                });
                            }
                            Poll::Pending
                        })
                        .await;
                        (race, DriverState::Ended(exit))
                    }
                    Race::Body(ended) => (Race::Body(ended), DriverState::Running(driver.as_mut())),
                    Race::Abandoned => (Race::Abandoned, DriverState::Running(driver.as_mut())),
                    Race::FailStopped => (Race::FailStopped, DriverState::Running(driver.as_mut())),
                };
                match race {
                    Race::Body(ended) => (BodyOutcome::Returned(ended), driver),
                    Race::Abandoned => (BodyOutcome::Abandoned, driver),
                    Race::FailStopped => {
                        install.retiring.set();
                        let mut retirement = pin!(retire(driver, bound.as_mut()));
                        let first = poll_fn(|cx| {
                            if let Poll::Ready(ended) = body.as_mut().poll(cx) {
                                return Poll::Ready(Joined::Body(ended));
                            }
                            retirement.as_mut().poll(cx).map(Joined::Retired)
                        })
                        .await;
                        let (ended, exit) = match first {
                            Joined::Body(ended) => (ended, retirement.await),
                            Joined::Retired(exit) => (body.await, exit),
                        };
                        (BodyOutcome::Returned(ended), DriverState::Bounded(exit))
                    }
                    Race::Driver(never) => match never {},
                }
            };
            // The body's exclusive loan ended. Retirement retains the armed
            // guard only while no abandon has happened yet.
            match outcome {
                BodyOutcome::Abandoned => {
                    let backend = ScopeGuard::into_inner(armed);
                    let outcome = backend.abandon(Quarantine::new());
                    install.retiring.set();
                    let state = retire(driver, bound.as_mut()).await;
                    InstallEnd::Abandoned { outcome, state }
                }
                BodyOutcome::Returned(Ended { end, output, .. }) => match end {
                    BodyEnd::FailStopped(outcome) => {
                        let _ = ScopeGuard::into_inner(armed);
                        install.retiring.set();
                        let state = retire(driver, bound.as_mut()).await;
                        InstallEnd::FailStopped {
                            outcome,
                            state,
                            output,
                        }
                    }
                    BodyEnd::Uninstall(control) => {
                        let control = ManuallyDrop::new(control);
                        install.retiring.set();
                        match retire(driver, bound.as_mut()).await {
                            Retirement::Ended(state) => {
                                let _ = ScopeGuard::into_inner(armed);
                                InstallEnd::Uninstall {
                                    state,
                                    control: ManuallyDrop::into_inner(control),
                                    output,
                                }
                            }
                            Retirement::TimedOut => {
                                let backend = ScopeGuard::into_inner(armed);
                                InstallEnd::FailStopped {
                                    outcome: backend.abandon(Quarantine::new()),
                                    state: Retirement::TimedOut,
                                    output,
                                }
                            }
                        }
                    }
                },
            }
        };
        // Dropping the pinned drive future ended its loan. It never owned the
        // driver; moving these handles cannot move their static DMA backing.
        let driver = ScopeGuard::into_inner(driver_owner);
        match end {
            InstallEnd::Uninstall {
                state,
                control,
                output,
            } => {
                let retired = Retired {
                    state: (driver, driver_state(state)),
                    brand: PhantomData,
                };
                backend.detach(Detach::Retired(Call::new()), &install.held);
                let backend = ScopeGuard::into_inner(backend);
                let detached = Detached { brand: PhantomData };
                PortEnd::Uninstalled {
                    uninstalled: backend.uninstall(
                        ManuallyDrop::into_inner(install.held),
                        ManuallyDrop::into_inner(local),
                        retired,
                        detached,
                        control,
                    ),
                    output,
                }
            }
            InstallEnd::FailStopped {
                outcome,
                state,
                output,
            } => {
                if let Retirement::Ended(_) = &state {
                    backend.detach(
                        Detach::Quarantined {
                            call: Call::new(),
                            outcome: &outcome,
                        },
                        &install.held,
                    );
                }
                let backend = ScopeGuard::into_inner(backend);
                PortEnd::FailStopped {
                    outcome,
                    quarantined: Quarantined::new((backend, driver, retired_state(state))),
                    output,
                }
            }
            InstallEnd::Abandoned { outcome, state } => {
                if let Retirement::Ended(_) = &state {
                    backend.detach(
                        Detach::Quarantined {
                            call: Call::new(),
                            outcome: &outcome,
                        },
                        &install.held,
                    );
                }
                let backend = ScopeGuard::into_inner(backend);
                PortEnd::Abandoned {
                    outcome,
                    quarantined: Quarantined::new((backend, driver, retired_state(state))),
                }
            }
        }
    })
}

enum Race<E, S> {
    Body(E),
    Abandoned,
    FailStopped,
    Driver(S),
}
enum BodyEnd<U> {
    Uninstall(U),
    FailStopped(Abandon),
}
enum BodyOutcome<E> {
    Returned(E),
    Abandoned,
}
enum DriverState<D, S> {
    Running(D),
    Ended(S),
    Bounded(Retirement<S>),
}
/// Driver completion retained with a quarantined installation.
pub enum Retirement<S> {
    /// The driver returned this state within the retirement bound.
    Ended(S),
    /// The bound expired; cancellation ended only the driver's loan.
    TimedOut,
}
enum Joined<E, S> {
    Body(E),
    Retired(S),
}

enum InstallEnd<U, Out, S> {
    Uninstall {
        state: S,
        control: U,
        output: Out,
    },
    FailStopped {
        outcome: Abandon,
        state: Retirement<S>,
        output: Out,
    },
    Abandoned {
        outcome: Abandon,
        state: Retirement<S>,
    },
}

fn driver_state<S, F>(exit: ManuallyDrop<DriverExit<S, F>>) -> S {
    match ManuallyDrop::into_inner(exit) {
        DriverExit::Retired {
            state,
            proof: RetireProof { private: () },
        }
        | DriverExit::Faulted {
            state,
            cause: _,
            poisoned: PoisonProof { private: () },
        } => state,
    }
}

fn retired_state<S, F>(exit: Retirement<ManuallyDrop<DriverExit<S, F>>>) -> Retirement<S> {
    match exit {
        Retirement::Ended(exit) => Retirement::Ended(driver_state(exit)),
        Retirement::TimedOut => Retirement::TimedOut,
    }
}

fn abandon_on_cancel<B: PortBackend>(backend: &mut B, latch: &AbandonLatch) {
    // A fail-stopped body may still hold the backend loan. It already
    // called abandon synchronously; cancellation must only quarantine.
    if !latch.is_fail_stopped() {
        latch.set();
        let outcome = backend.abandon(Quarantine::new());
        // Cancellation has no result receiver. Retain its outcome; the
        // enclosing guards independently quarantine backend and driver.
        drop(Quarantined::new(outcome));
    }
}

async fn retire<S>(
    driver: DriverState<Pin<&mut impl Future<Output = S>>, S>,
    mut bound: Pin<&mut impl Future<Output = ()>>,
) -> Retirement<S> {
    match driver {
        DriverState::Ended(exit) => Retirement::Ended(exit),
        DriverState::Bounded(exit) => exit,
        DriverState::Running(mut driver) => {
            poll_fn(|cx| {
                if let Poll::Ready(exit) = driver.as_mut().poll(cx) {
                    return Poll::Ready(Retirement::Ended(exit));
                }
                if bound.as_mut().poll(cx).is_ready() {
                    return Poll::Ready(Retirement::TimedOut);
                }
                Poll::Pending
            })
            .await
        }
    }
}

/// The installation's final outcome, including the body's unbranded result.
#[must_use = "handle the returned owners or the fail-stop outcome"]
pub enum PortEnd<B: PortBackend, Out> {
    /// Retirement and detachment completed; uninstall returned its owners.
    Uninstalled {
        /// Hardware owners, including retained owners on teardown failure.
        uninstalled: B::Uninstalled,
        /// The body's result.
        output: Out,
    },
    /// The port was quarantined explicitly or missed its retirement bound.
    FailStopped {
        /// What the synchronous quarantine proved about hardware stopping.
        outcome: Abandon,
        /// Permanently retained backend, driver and completion state.
        quarantined: Quarantined<(B, B::Driver, Retirement<B::State>)>,
        /// The body's result; no hardware ownership escapes quarantine.
        output: Out,
    },
    /// The owner was dropped; the core ended the installation itself.
    Abandoned {
        /// What the synchronous quarantine proved about hardware stopping.
        outcome: Abandon,
        /// Permanently retained backend, driver and completion state.
        quarantined: Quarantined<(B, B::Driver, Retirement<B::State>)>,
    },
}

/// Proof that this installation has already been synchronously quarantined.
/// Its brand ties it to the body; only consuming an owner can construct it.
#[must_use = "return this proof through Ended::fail_stopped"]
pub struct FailStopped<'id, B: PortBackend> {
    outcome: Abandon,
    brand: Brand<'id>,
    backend: PhantomData<fn() -> B>,
}

impl<B: PortBackend> FailStopped<'_, B> {
    /// Inspect whether the earlier synchronous stop was confirmed. The proof
    /// remains affine and must still return through `Ended::fail_stopped`.
    pub fn outcome(&self) -> &Abandon {
        &self.outcome
    }
}

/// The body's consumed owner and result, with no borrow lifetime.
#[must_use = "return the ended port to run_port"]
pub struct Ended<'id, B: PortBackend, Out> {
    end: BodyEnd<B::Uninstall>,
    output: Out,
    brand: Brand<'id>,
}

impl<'id, B: PortBackend, Out> Ended<'id, B, Out> {
    /// Consume a disabled or poisoned owner and request uninstall on return.
    pub fn uninstall(
        owner: impl UninstallOwner<'id, B>,
        control: B::Uninstall,
        output: Out,
    ) -> Self {
        owner.disarm();
        Self {
            end: BodyEnd::Uninstall(control),
            output,
            brand: PhantomData,
        }
    }

    /// Attach the result to proof of the earlier, immediate fail-stop.
    pub fn fail_stopped(proof: FailStopped<'id, B>, output: Out) -> Self {
        let FailStopped { outcome, .. } = proof;
        Self {
            end: BodyEnd::FailStopped(outcome),
            output,
            brand: PhantomData,
        }
    }
}

mod sealed {
    pub trait Uninstall {
        fn disarm(self);
    }
}

/// A disabled or poisoned owner accepted by `Ended::uninstall`.
/// Sealed: an enabled owner must first complete `disable`.
pub trait UninstallOwner<'id, B: PortBackend>: sealed::Uninstall {}

impl<'id, B: PortBackend> UninstallOwner<'id, B> for Owner<'_, 'id, B, Disabled> {}
impl<B: PortBackend> sealed::Uninstall for Owner<'_, '_, B, Disabled> {
    fn disarm(self) {
        self.sentry.disarm();
    }
}
impl<'id, B: PortBackend> UninstallOwner<'id, B> for PoisonedOwner<'_, 'id, B> {}
impl<B: PortBackend> sealed::Uninstall for PoisonedOwner<'_, '_, B> {
    fn disarm(self) {
        self.sentry.disarm();
    }
}

struct Sentry<'p>(&'p AbandonLatch);
impl Sentry<'_> {
    fn disarm(self) {
        core::mem::forget(self);
    }
}
impl Drop for Sentry<'_> {
    fn drop(&mut self) {
        self.0.set();
    }
}

/// The sole authority over the backend, borrowing it for the port body.
#[must_use = "consume the owner through a transition, uninstall or fail-stop"]
pub struct Owner<'p, 'id, B: PortBackend, S> {
    backend: &'p mut B,
    local: &'p mut B::Local<'id>,
    install: &'p Install<'id, B>,
    sentry: Sentry<'p>,
    state: PhantomData<S>,
}

impl<'p, 'id, B: PortBackend, S> Owner<'p, 'id, B, S> {
    fn new(backend: &'p mut B, local: &'p mut B::Local<'id>, install: &'p Install<'id, B>) -> Self {
        Self {
            backend,
            local,
            install,
            sentry: Sentry(&install.latch),
            state: PhantomData,
        }
    }

    /// Read the backend's radio clock.
    pub fn now(
        &self,
    ) -> impl Future<Output = PortResult<RadioInstant<B::Domain>, ClockError, B::Fault>> + '_ {
        self.backend.now(Call::new())
    }

    /// Synchronously quarantine and attempt bounded stopping, then wake the core
    /// to retire the driver. Final detachment and result delivery wait only
    /// for `Ended::fail_stopped` to return from the body.
    pub fn fail_stop(self) -> FailStopped<'id, B> {
        let outcome = self.backend.abandon(Quarantine::new());
        self.install.latch.fail_stopped();
        self.install.retiring.set();
        self.sentry.disarm();
        FailStopped {
            outcome,
            brand: PhantomData,
            backend: PhantomData,
        }
    }
}

impl<'p, 'id, B: PortBackend> Owner<'p, 'id, B, Disabled> {
    /// Borrow settings for this disabled installation.
    pub fn setup(&mut self) -> Lent<'_, B::Setup<'_, 'id>> {
        Lent::new(
            self.backend
                .setup(Call::new(), &self.install.held, self.local),
        )
    }

    /// Consume the disabled owner, returning the enabled owner or its
    /// refusal/poisoned successor within the backend's bound.
    pub async fn enable(self) -> Result<Owner<'p, 'id, B, Enabled>, Failed<'p, 'id, B>> {
        let Self {
            backend,
            local,
            install,
            sentry,
            ..
        } = self;
        match backend.enable(Call::new(), &install.held, local).await {
            Ok(Ok(())) => Ok(Owner {
                backend,
                local,
                install,
                sentry,
                state: PhantomData,
            }),
            Ok(Err(refusal)) => Err(Failed::Refused(
                Owner {
                    backend,
                    local,
                    install,
                    sentry,
                    state: PhantomData,
                },
                refusal,
            )),
            Err(poisoned) => Err(Failed::Poisoned(PoisonedOwner {
                backend,
                _local: local,
                install,
                sentry,
                cause: poisoned.cause,
            })),
        }
    }
}

impl<'p, 'id, B: PortBackend> Owner<'p, 'id, B, Enabled> {
    /// Lend one running phase, which ends before any owner transition.
    pub fn phase(&mut self) -> Lent<'_, B::Phase<'_, 'id>> {
        Lent::new(
            self.backend
                .phase(Call::new(), &self.install.held, self.local),
        )
    }

    /// End admitted work and hand its remaining terminals to `sink`.
    /// A sink returning `Break(terminal)` stops delivery and leaves that
    /// terminal and its successors in the returned poisoned installation.
    pub async fn disable(
        self,
        sink: &mut dyn FnMut(B::Terminal<'id>) -> core::ops::ControlFlow<B::Terminal<'id>>,
    ) -> Result<Owner<'p, 'id, B, Disabled>, PoisonedOwner<'p, 'id, B>> {
        let Self {
            backend,
            local,
            install,
            sentry,
            ..
        } = self;
        match backend
            .disable(Call::new(), &install.held, local, sink)
            .await
        {
            Ok(()) => Ok(Owner {
                backend,
                local,
                install,
                sentry,
                state: PhantomData,
            }),
            Err(poisoned) => Err(PoisonedOwner {
                backend,
                _local: local,
                install,
                sentry,
                cause: poisoned.cause,
            }),
        }
    }
}

/// An enable refusal keeps the disabled owner; poisoning prevents admission.
pub enum Failed<'p, 'id, B: PortBackend> {
    /// Radio refusal, with unchanged disabled authority.
    Refused(Owner<'p, 'id, B, Disabled>, B::Refusal),
    /// Published poisoning, with the retained installation.
    Poisoned(PoisonedOwner<'p, 'id, B>),
}

impl<'id, B: PortBackend> Failed<'_, 'id, B> {
    /// End either outcome as a fail-stop.
    pub fn fail_stop(self) -> FailStopped<'id, B> {
        match self {
            Self::Refused(owner, _) => owner.fail_stop(),
            Self::Poisoned(owner) => owner.fail_stop(),
        }
    }
}

/// A poisoned installation: its cause and retained owners remain available.
#[must_use = "a poisoned owner ends through uninstall or fail-stop"]
pub struct PoisonedOwner<'p, 'id, B: PortBackend> {
    backend: &'p mut B,
    _local: &'p mut B::Local<'id>,
    install: &'p Install<'id, B>,
    sentry: Sentry<'p>,
    cause: B::Fault,
}

impl<'id, B: PortBackend> PoisonedOwner<'_, 'id, B> {
    /// The published fault that ended admission.
    pub const fn cause(&self) -> B::Fault {
        self.cause
    }

    /// Synchronously quarantine and attempt bounded stopping, then request the
    /// driver's retirement. Return the proof with `Ended::fail_stopped`.
    pub fn fail_stop(self) -> FailStopped<'id, B> {
        let outcome = self.backend.abandon(Quarantine::new());
        self.install.latch.fail_stopped();
        self.install.retiring.set();
        self.sentry.disarm();
        FailStopped {
            outcome,
            brand: PhantomData,
            backend: PhantomData,
        }
    }
}

/// An invariant loan which exposes references but never gives its part away.
pub struct Lent<'a, T> {
    part: T,
    loan: Brand<'a>,
}
impl<T> Lent<'_, T> {
    const fn new(part: T) -> Self {
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

/// The running phase's event consumer. Cancelling `next_event` loses no
/// event. Once ended, every call returns the same [`EventEnd`] and cause.
pub trait PortPhase {
    /// An owned event, including a reserved terminal of admitted work.
    type Event;
    /// The cause carried by the final stream end.
    type Fault: Copy + Debug;
    /// Dequeue in occurrence order; only unpromised unsolicited data may
    /// be replaced by `EventsLost`. [`EventEnd::Drained`] promises every
    /// admitted terminal was delivered; [`EventEnd::Quarantined`] leaves
    /// unread owners in [`PortEnd`]. An [`Abandon::Unconfirmed`] end must
    /// return without waiting or acquiring the ISR's mutex.
    fn next_event(
        &mut self,
    ) -> impl Future<Output = Result<Result<Self::Event, EventsLost>, EventEnd<Self::Fault>>> + '_;
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
