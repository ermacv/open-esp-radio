//! One permanent, installation-owned SRAM region for driver and IRQ registration.
//!
//! The future owns its affine pinned lease, never the backing allocation.
//! Bounded quiescence returns that lease or quarantines it permanently. Neither
//! cancellation nor forgetting the future can reclaim memory an entered actor
//! still uses. Context and the output latch are outside the registered graph.

use atomic_waker::AtomicWaker;
use core::{
    cell::UnsafeCell,
    future::poll_fn,
    marker::{PhantomData, PhantomPinned},
    mem::{ManuallyDrop, MaybeUninit},
    pin::Pin,
    ptr,
    sync::atomic::{AtomicBool, AtomicPtr, AtomicU8, AtomicUsize, Ordering},
    task::{Context, Poll, RawWaker, RawWakerVTable, Waker},
};
use oer_esp32s31_hal::{
    owner::{ConnectedStaInterruptPrepared, RadioRuntimeOwner},
    types::{MacPowerInterruptObservation, MacPowerWakeCause},
};
use oer_esp32s31_ieee80211_mac::irq::{
    IrqSnapshot, PowerIrqSnapshot,
    consumer::{
        IrqBatch, IrqConsumer, IrqPublication, IrqPublisher, IrqStorage, PowerIrqConsumer,
        PowerIrqPublisher, PowerIrqStorage,
    },
    epoch::{ActiveEpoch, InactiveEpoch, Moderated},
};
use oer_radio_port::Quarantined;
use oer_time::{Clock, Duration, Instant};

pub mod commands;
pub mod driver;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WakeSources {
    Commands,
    Interrupts,
    CommandsAndInterrupts,
}

/// A ready finite step, or concrete events and a future watchdog deadline.
#[must_use = "the executor must arrange every wake in this plan"]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WakePlan {
    Ready,
    Wait {
        sources: WakeSources,
        deadline: FutureDeadline,
    },
}

/// A deadline checked against the fresh clock sample at the end of a step.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FutureDeadline {
    at: Instant,
}
impl FutureDeadline {
    pub const fn at(self) -> Instant {
        self.at
    }
}
impl WakePlan {
    pub fn until(now: Instant, at: Instant, sources: WakeSources) -> Self {
        if at <= now {
            Self::Ready
        } else {
            Self::Wait {
                sources,
                deadline: FutureDeadline { at },
            }
        }
    }
}

#[must_use]
pub enum DriverStep {
    Wake(WakePlan),
    /// Retirement work is complete. The scope still drains all actors and
    /// invokes stop once before returning hardware or its backing lease.
    Retired,
}

/// Facts captured once per finite driver step. New publications are next-step work.
#[must_use = "every captured IRQ fact belongs to this step"]
pub struct DriverEvents {
    pub mac: IrqBatch,
    pub power: MacPowerInterruptObservation,
}

/// One monotonic snapshot at entry to a finite driver step. A later request
/// belongs to the next step; the final wake check observes it independently.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum DriverStatus {
    Running,
    RetireRequested,
    Retired,
    Abandoned,
}

struct DriverLifecycle(AtomicU8);
impl DriverLifecycle {
    const fn new() -> Self {
        Self(AtomicU8::new(DriverStatus::Running as u8))
    }
    fn snapshot(&self) -> DriverStatus {
        match self.0.load(Ordering::Acquire) {
            0 => DriverStatus::Running,
            1 => DriverStatus::RetireRequested,
            2 => DriverStatus::Retired,
            // Only the private transitions below write this atomic.
            _ => DriverStatus::Abandoned,
        }
    }
    fn request_retire(&self) {
        self.0
            .fetch_max(DriverStatus::RetireRequested as u8, Ordering::AcqRel);
    }
    fn retired(&self) {
        // An abandonment concurrent with completion remains terminal.
        let _previous = self.0.compare_exchange(
            DriverStatus::RetireRequested as u8,
            DriverStatus::Retired as u8,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }
    fn abandon(&self) {
        self.0
            .store(DriverStatus::Abandoned as u8, Ordering::Release);
    }
}

mod sealed {
    pub trait InterruptOwner {}
    /// The form's own view of a driver's hardware; only the form implements it.
    pub trait OwnsDma {
        /// The type of the DMA owner a bridge executor reaches.
        type Dma;
        /// The DMA owner, for the composite's executor step.
        fn dma(&mut self) -> &mut Self::Dma;
        /// Withdraw every pending bridge record and zero its key copy. The
        /// form calls it after every stop whose drain completed.
        fn erase_pending_keys(&mut self);
    }
}

/// A driver's hardware, as the form knows it: which DMA owner its records
/// reach (`Dma`) and where their key copies live.
///
/// Sealed ( ): only the form implements it, for
/// [`RadioRuntimeOwner`] (the S31's one Wi-Fi DMA, owning no executor) and
/// for the composite [`commands::bridge::Bridged`], which owns hardware and
/// its bridge executor and is the only way to step that executor. So the
/// form itself finds every key copy it erases after a completed stop, and a
/// stop proof names the DMA its records could reach.
pub trait OwnsDma: sealed::OwnsDma {}
impl<H: sealed::OwnsDma> OwnsDma for H {}

/// The bare radio owner, as the placement example installs it, owns no
/// bridge executor and so no key copy.
impl sealed::OwnsDma for RadioRuntimeOwner {
    type Dma = RadioRuntimeOwner;
    fn dma(&mut self) -> &mut RadioRuntimeOwner {
        self
    }
    fn erase_pending_keys(&mut self) {}
}

/// Only the real moderated epoch implements this outside this module's tests.
/// The two register banks are disjoint from the task-side radio owner.
pub trait InterruptOwner: sealed::InterruptOwner + Send + 'static {
    type Detached;
    fn mac(&mut self) -> IrqSnapshot;
    fn power(&mut self) -> PowerIrqSnapshot;
    fn detach(self) -> Self::Detached;
}
impl sealed::InterruptOwner for ActiveEpoch<Moderated> {}
impl InterruptOwner for ActiveEpoch<Moderated> {
    type Detached = InactiveEpoch;
    fn mac(&mut self) -> IrqSnapshot {
        self.service_mac().1
    }
    fn power(&mut self) -> PowerIrqSnapshot {
        self.service_power().1
    }
    fn detach(self) -> InactiveEpoch {
        self.quiesce()
    }
}

/// The sole permanent ISR address and admission domain. An entrant increments
/// before testing admission. Closing admission and observing zero therefore
/// excludes every possible borrow of the old registration on every core.
pub struct ScopeSlot {
    entry: AtomicPtr<Header>,
    open: AtomicBool,
    entered: AtomicUsize,
}
impl ScopeSlot {
    pub const fn new() -> Self {
        Self {
            entry: AtomicPtr::new(ptr::null_mut()),
            open: AtomicBool::new(false),
            entered: AtomicUsize::new(0),
        }
    }

    fn enter(&self) -> Option<Entered<'_>> {
        // SeqCst closes the two-atomic admission race: an entrant not seen by
        // the zero-count observation must see the preceding closed admission.
        self.entered.fetch_add(1, Ordering::SeqCst);
        let guard = Entered(self);
        if self.open.load(Ordering::SeqCst) {
            Some(guard)
        } else {
            None
        }
    }

    #[allow(
        clippy::disallowed_methods,
        reason = "synchronous cancellation has a finite observation bound and cannot yield to an executor"
    )]
    fn close(&self, bound: DrainBound, clock: &impl Clock) -> Quiescence {
        self.open.store(false, Ordering::SeqCst);
        let start = clock.now();
        loop {
            if self.entered.load(Ordering::SeqCst) == 0 {
                return Quiescence::Drained;
            }
            if clock.now().saturating_duration_since(start) >= bound.duration() {
                return Quiescence::TimedOut;
            }
            #[cfg(test)]
            tests::wait_boundary();
            core::hint::spin_loop();
        }
    }

    #[allow(
        unsafe_code,
        reason = "the owned waker refers only to the permanent address slot"
    )]
    fn driver_waker(&'static self) -> Waker {
        // SAFETY: the slot itself is static, independently of any lease. Wake
        // enters its admission domain before looking at the current binding;
        // clone/drop need no installation memory or allocation.
        unsafe { Waker::from_raw(RawWaker::new(ptr::from_ref(self).cast(), &DRIVER_WAKER)) }
    }

    #[allow(
        unsafe_code,
        reason = "the static slot admits this scheduling callback independently"
    )]
    #[cfg_attr(
        target_arch = "riscv32",
        unsafe(link_section = ".rwtext.open_radio_irq")
    )]
    #[inline(never)]
    fn notify_driver(&self) {
        let Some(_entered) = self.enter() else {
            return;
        };
        let entry = self.entry.load(Ordering::Acquire);
        // SAFETY: the count protects the current IRQ region. This callback
        // only pends the executor, and cannot borrow Driver or synchronously
        // tear down the installation. Closed/stale wakes reach no region.
        let _scheduled = unsafe { ((*entry).schedule)((*entry).irq_context) };
    }

    /// Run one step on the separate driver executor, on either permitted core.
    ///
    /// # Safety
    /// The executor serializes driver dispatches (including across cores).
    /// Platform callbacks are the ones audited by DriverScope::new. Wakers
    /// only schedule: they must never poll or synchronously tear down a scope.
    #[allow(
        unsafe_code,
        reason = "the exclusive executor invokes its admitted registration"
    )]
    pub unsafe fn dispatch(&self) {
        let completion = {
            let Some(_entered) = self.enter() else {
                return;
            };
            let entry = self.entry.load(Ordering::Acquire);
            // SAFETY: admission keeps the permanent header and its exact
            // driver projection alive; the caller serializes driver steps.
            unsafe { ((*entry).driver)((*entry).driver_context) }
        };
        if let Some(waker) = completion {
            waker.wake();
        }
    }

    /// Capture facts through the IRQ region, never through a Driver borrow.
    /// Composition wakers may only pend execution; synchronous teardown or
    /// polling from clone/wake/drop would violate the installation binding.
    #[allow(
        unsafe_code,
        reason = "the hard entry and its callback graph execute from SRAM"
    )]
    #[cfg_attr(
        target_arch = "riscv32",
        unsafe(link_section = ".rwtext.open_radio_irq")
    )]
    #[inline(never)]
    pub fn mac_interrupt(&self) -> IrqSnapshot {
        match self.capture_mac() {
            Some(publication) => publication.wake(),
            None => IrqSnapshot::default(),
        }
    }
    #[inline(always)]
    fn capture_mac(&self) -> Option<MacPublication> {
        let _entered = self.enter()?;
        critical_section::with(|_| {
            let entry = self.entry.load(Ordering::Acquire);
            #[allow(unsafe_code, reason = "admission retains the separate IRQ region")]
            // SAFETY: the count precedes admission. Release cannot reclaim the
            // registration before this guard exits; timeout retains it forever.
            Some(unsafe { ((*entry).mac)((*entry).irq_context) })
        })
    }

    #[allow(
        unsafe_code,
        reason = "both hard entries share the audited SRAM placement"
    )]
    #[cfg_attr(
        target_arch = "riscv32",
        unsafe(link_section = ".rwtext.open_radio_irq")
    )]
    #[inline(never)]
    pub fn power_interrupt(&self) -> PowerIrqSnapshot {
        match self.capture_power() {
            Some(publication) => publication.wake(),
            None => PowerIrqSnapshot::default(),
        }
    }
    #[inline(always)]
    fn capture_power(&self) -> Option<PowerPublication> {
        let _entered = self.enter()?;
        critical_section::with(|_| {
            let entry = self.entry.load(Ordering::Acquire);
            #[allow(
                unsafe_code,
                reason = "the admission guard covers the whole IRQ transaction"
            )]
            // SAFETY: same admission and permanent-backing proof as MAC.
            Some(unsafe { ((*entry).power)((*entry).irq_context) })
        })
    }
}
impl Default for ScopeSlot {
    fn default() -> Self {
        Self::new()
    }
}

#[allow(
    unsafe_code,
    reason = "the driver's immutable wake table is in the IRQ SRAM closure"
)]
#[cfg_attr(
    target_arch = "riscv32",
    unsafe(link_section = ".critical.rodata.open_radio_irq")
)]
static DRIVER_WAKER: RawWakerVTable = RawWakerVTable::new(
    clone_driver_waker,
    wake_driver,
    wake_driver,
    drop_driver_waker,
);

#[allow(
    unsafe_code,
    reason = "clone preserves the permanent slot address without borrowing an installation"
)]
#[cfg_attr(
    target_arch = "riscv32",
    unsafe(link_section = ".rwtext.open_radio_irq")
)]
unsafe fn clone_driver_waker(data: *const ()) -> RawWaker {
    RawWaker::new(data, &DRIVER_WAKER)
}
#[allow(
    unsafe_code,
    reason = "the raw waker was constructed solely from a static ScopeSlot"
)]
#[cfg_attr(
    target_arch = "riscv32",
    unsafe(link_section = ".rwtext.open_radio_irq")
)]
unsafe fn wake_driver(data: *const ()) {
    // SAFETY: only driver_waker/clone_driver_waker construct this vtable and
    // each retains the exact static slot pointer. No DriverScope pointer lives
    // in the waker, so late wakes remain valid after release or quarantine.
    unsafe { &*data.cast::<ScopeSlot>() }.notify_driver();
}
#[allow(
    unsafe_code,
    reason = "the permanent slot needs no wake-data reclamation"
)]
#[cfg_attr(
    target_arch = "riscv32",
    unsafe(link_section = ".rwtext.open_radio_irq")
)]
unsafe fn drop_driver_waker(_: *const ()) {}
struct Entered<'a>(&'a ScopeSlot);
impl Drop for Entered<'_> {
    fn drop(&mut self) {
        self.0.entered.fetch_sub(1, Ordering::SeqCst);
    }
}

enum Quiescence {
    Drained,
    TimedOut,
}

/// How closing ended: every actor left and stop reported its DMA fact, or an
/// actor may still be inside the installation.
enum Closed {
    Stopped(StopOutcome),
    TimedOut,
}

/// What the driver's stop proved about the installation's DMA.
#[must_use = "only a quiescent stop permits returning DMA-reachable owners"]
pub enum StopOutcome {
    /// Every DMA channel is inactive and no descriptor is owned by hardware.
    Quiescent(DmaQuiescent),
    /// Stop ran, but DMA quiescence was not read or cannot be trusted. After
    /// either outcome the drain completed, so the form itself then withdraws
    /// every pending bridge record of the hardware and zeroes its key copy
    /// before any owner can go into quarantine ( , ).
    Unconfirmed,
}

/// Evidence that the installation's DMA no longer touches its memory.
///
/// No production code can build it: only the model fixtures construct it,
/// and the HAL has no DMA-quiescence reading yet, so every production stop
/// reports `Unconfirmed`.
pub struct DmaQuiescent {
    _private: (),
}
impl DmaQuiescent {
    /// The model fixtures' stand-in for that reading.
    ///
    /// # Safety
    /// The caller has just read, through typed PAC accessors, that every DMA
    /// channel of this installation is inactive and that no descriptor is
    /// owned by hardware, using an indication confirmed against the vendor
    /// and cited with `SOURCE`. Owners returned on this evidence are reused
    /// at once; a wrong claim lets hardware write memory that has a new
    /// owner. Without such an indication the stop reports `Unconfirmed`.
    #[allow(
        unsafe_code,
        reason = "reusing DMA-reachable memory rests on the stop's hardware reading"
    )]
    #[cfg(test)]
    pub(crate) const unsafe fn confirmed() -> Self {
        Self { _private: () }
    }
}

/// Time allowed for entered actors to leave after synchronous masking.
/// Construct it from the worst finite driver step, the complete IRQ path
/// (including nested handlers / cross-core serialization), and scheduling /
/// clock-resolution margin, all in microseconds. This is independent of CPU
/// spin throughput. Production maxima must be measured and bounded by S31's
/// composition; the prototype supplies no production timing constants.
#[derive(Clone, Copy, Debug)]
pub struct DrainBound {
    duration: Duration,
}
impl DrainBound {
    pub const fn from_maxima<const STEP_US: u64, const IRQ_US: u64, const MARGIN_US: u64>() -> Self
    {
        const {
            assert!(
                STEP_US > 0 && IRQ_US > 0 && MARGIN_US > 0,
                "step, IRQ and margin bounds must be positive"
            );
            assert!(STEP_US <= u64::MAX - IRQ_US);
            assert!(STEP_US + IRQ_US <= u64::MAX - MARGIN_US);
        }
        Self {
            duration: Duration::from_micros(STEP_US + IRQ_US + MARGIN_US),
        }
    }
    pub const fn duration(self) -> Duration {
        self.duration
    }
}

#[macro_export]
macro_rules! declare_scope_slot {
    ($visibility:vis static $name:ident) => {
        #[cfg_attr(target_arch = "riscv32", unsafe(link_section = ".critical.data.open_radio_irq"))]
        $visibility static $name: $crate::scope::ScopeSlot = $crate::scope::ScopeSlot::new();
    };
}

/// Monomorphized scheduling supplied by a composition. It only arranges a
/// later driver step, never polls, destroys a scope, or calls an external waker.
/// A Wait plan arms its sources/deadline; it must never clear an already
/// pending driver-SWI. The executor acknowledges that SWI before dispatch.
/// IRQ publication and retire/abandon can therefore pend it on either side
/// of the driver's final Acquire check without losing a wake.
pub trait WakeScheduler<P>: Sync + 'static {
    fn schedule(&self, platform: &P, plan: WakePlan);
}
impl<P, F: Fn(&P, WakePlan) + Sync + 'static> WakeScheduler<P> for F {
    fn schedule(&self, platform: &P, plan: WakePlan) {
        self(platform, plan);
    }
}

/// By-value platform configuration. Constructing it registers nothing.
#[must_use]
pub struct ScopeRoute<P, E, S = fn(&P, WakePlan)> {
    platform: P,
    slot: &'static ScopeSlot,
    mask: fn(&P),
    acknowledge: fn(&P),
    enable: fn(&P) -> Result<(), E>,
    schedule: S,
    now: fn(&P) -> Instant,
}
impl<P, E, S> ScopeRoute<P, E, S> {
    pub const fn new(
        platform: P,
        slot: &'static ScopeSlot,
        mask: fn(&P),
        acknowledge: fn(&P),
        enable: fn(&P) -> Result<(), E>,
        schedule: S,
        now: fn(&P) -> Instant,
    ) -> Self {
        Self {
            platform,
            slot,
            mask,
            acknowledge,
            enable,
            schedule,
            now,
        }
    }
    pub fn into_platform(self) -> P {
        self.platform
    }
}

impl<P, E, S> Clock for ScopeRoute<P, E, S> {
    fn now(&self) -> Instant {
        (self.now)(&self.platform)
    }
}

/// Active caller-side scope owner. IRQ publication borrows this owner; it
/// cannot escape its lifetime. Retirement and abandonment consume the owner,
/// so a body pending after abandon holds no second shutdown obligation.
pub struct ScopeAccess<
    'latch,
    H: OwnsDma + Send + 'static = RadioRuntimeOwner,
    I: InterruptOwner = ActiveEpoch<Moderated>,
    P: Sync + 'static = (),
    E: 'static = core::convert::Infallible,
    S: WakeScheduler<P> = fn(&P, WakePlan),
> {
    scope: DriverScope<'latch, H, I, P, E, S>,
}
/// Completed consuming abandonment. It owns no driver, lease or stop right;
/// the outcome has already moved to the composition's ScopeReturnLatch.
pub struct ScopeAbandoned {
    _private: (),
}
impl<
    H: OwnsDma + Send + 'static,
    I: InterruptOwner,
    P: Sync + 'static,
    E: 'static,
    S: WakeScheduler<P>,
> ScopeAccess<'_, H, I, P, E, S>
{
    pub fn irq(&self) -> &IrqPublisher<'_> {
        &self.scope.irq().mac
    }
    /// Synchronously mask, close, drain and stop once or permanently quarantine.
    /// The active owner is consumed before returning this terminal receipt.
    pub fn abandon(self) -> ScopeAbandoned {
        let mut scope = ManuallyDrop::new(self.scope);
        let quiescence = scope.lease.storage().abandon(scope.bound);
        let returned = scope.complete_after(quiescence);
        scope.latch.returned = Some(returned);
        ScopeAbandoned { _private: () }
    }
    /// Keep the result until the executor finishes retirement, then detach.
    /// Cancelling this await still owns the complete active scope; its Drop
    /// performs the same bounded physical drain exactly once.
    pub async fn retire<R>(self, result: R) -> R {
        self.scope.retire().await;
        drop(self);
        result
    }
}

#[must_use]
pub struct ReturnedOwners<H, I, P, E, S = fn(&P, WakePlan)> {
    pub hardware: H,
    pub interrupts: I,
    pub route: ScopeRoute<P, E, S>,
}
#[must_use]
pub enum DetachedOwners<H> {
    /// Every actor drained and stop proved DMA quiescent, on any of the form's own stop paths.
    Stopped(StoppedOwners<H>),
    /// Stop could not prove DMA quiescence: keep everything until reset.
    Unconfirmed(Quarantined<H>),
}

/// Owners whose stop read their DMA quiescent. Only a quiescent stop builds
/// this value, so its proof cannot be forged.
#[must_use = "lend the stop proof, then take the owners"]
pub struct StoppedOwners<H> {
    owners: H,
}
impl<H: OwnsDma, I, P, E, S> StoppedOwners<ReturnedOwners<H, I, P, E, S>> {
    /// The DMA-quiescence proof of exactly these owners. While it lives the
    /// owners cannot be moved, so their hardware cannot be installed again
    /// in any scope and the proof cannot become stale.
    pub fn stopped(&self) -> DriverStopped<'_, H::Dma> {
        DriverStopped {
            _scope: PhantomData,
            _dma: PhantomData,
        }
    }
}
impl<H> StoppedOwners<H> {
    /// Take the owners, ending every proof borrowed from them.
    pub fn into_owners(self) -> H {
        self.owners
    }
}

/// Proof that the returned hardware's DMA is quiescent: every channel
/// inactive and no descriptor owned by hardware, read by the driver's stop.
/// It borrows those owners ([`StoppedOwners::stopped`]) and names their DMA
/// owner type `D`: a bridge Held bound for `D` finishes only on it. The S31
/// image has one Wi-Fi DMA, so the type names that peripheral (
/// ).
#[must_use = "lend the stop proof to every store that retains DMA owners"]
pub struct DriverStopped<'scope, D> {
    _scope: PhantomData<&'scope ()>,
    _dma: PhantomData<fn() -> D>,
}

/// Successful quiescence returns the affine lease for another installation.
#[must_use]
pub struct ReleasedScope<
    H: OwnsDma + Send + 'static,
    I: InterruptOwner,
    P: Sync + 'static,
    E: 'static,
    S: WakeScheduler<P>,
> {
    pub lease: ScopeLease<H, I, P, E, S>,
    pub owners: DetachedOwners<ReturnedOwners<H, I::Detached, P, E, S>>,
}

/// No lease, owner or HardwareStopped proof can be recovered from this receipt.
/// Hardware may still be running. Masks, closed admission and permanent DMA /
/// owner quarantine preserve memory safety. The composition must report this
/// fault and arrange a chip reset; it must not reinstall or release clocks.
#[must_use = "reset the chip before installing again after permanent quarantine"]
pub struct ScopePoisoned {
    _private: (),
}
#[must_use]
pub enum ScopeReturn<
    H: OwnsDma + Send + 'static,
    I: InterruptOwner = ActiveEpoch<Moderated>,
    P: Sync + 'static = (),
    E: 'static = core::convert::Infallible,
    S: WakeScheduler<P> = fn(&P, WakePlan),
> {
    Released(ReleasedScope<H, I, P, E, S>),
    Poisoned(ScopePoisoned),
}
/// An output slot, outside every registered callback's reachable memory.
/// None means no return was delivered (for example, the future was forgotten).
pub struct ScopeReturnLatch<
    H: OwnsDma + Send + 'static,
    I: InterruptOwner = ActiveEpoch<Moderated>,
    P: Sync + 'static = (),
    E: 'static = core::convert::Infallible,
    S: WakeScheduler<P> = fn(&P, WakePlan),
> {
    returned: Option<ScopeReturn<H, I, P, E, S>>,
}
impl<
    H: OwnsDma + Send + 'static,
    I: InterruptOwner,
    P: Sync + 'static,
    E: 'static,
    S: WakeScheduler<P>,
> ScopeReturnLatch<H, I, P, E, S>
{
    pub const fn new() -> Self {
        Self { returned: None }
    }
    pub fn into_return(self) -> Option<ScopeReturn<H, I, P, E, S>> {
        self.returned
    }
}
impl<
    H: OwnsDma + Send + 'static,
    I: InterruptOwner,
    P: Sync + 'static,
    E: 'static,
    S: WakeScheduler<P>,
> Default for ScopeReturnLatch<H, I, P, E, S>
{
    fn default() -> Self {
        Self::new()
    }
}

/// The composition declares one of these in SRAM and transfers its one
/// &'static mut reference at boot (or from its installation Mint). There is
/// no shared take(), installed flag, allocator or fallible second acquisition.
/// Context is not stored here and has no 'static requirement.
pub struct ScopeStorage<
    H: OwnsDma + Send + 'static,
    I: InterruptOwner = ActiveEpoch<Moderated>,
    P: Sync + 'static = (),
    E: 'static = core::convert::Infallible,
    S: WakeScheduler<P> = fn(&P, WakePlan),
> {
    mac: UnsafeCell<IrqStorage>,
    power: UnsafeCell<PowerIrqStorage>,
    irq: BackingCell<IrqRegion<I, P, E, S>>,
    driver: BackingCell<DriverRegion<H, I, P, E, S>>,
    header: BackingCell<Header>,
    _pin: PhantomPinned,
}
type BackingCell<T> = UnsafeCell<MaybeUninit<T>>;
impl<
    H: OwnsDma + Send + 'static,
    I: InterruptOwner,
    P: Sync + 'static,
    E: 'static,
    S: WakeScheduler<P>,
> ScopeStorage<H, I, P, E, S>
{
    pub const fn new() -> Self {
        Self {
            mac: UnsafeCell::new(IrqStorage::new()),
            power: UnsafeCell::new(PowerIrqStorage::new()),
            irq: UnsafeCell::new(MaybeUninit::uninit()),
            driver: UnsafeCell::new(MaybeUninit::uninit()),
            header: UnsafeCell::new(MaybeUninit::uninit()),
            _pin: PhantomPinned,
        }
    }
    #[allow(
        unsafe_code,
        reason = "the lease initialized this permanent IRQ projection"
    )]
    fn irq(&self) -> &IrqRegion<I, P, E, S> {
        // SAFETY: only the active scope and its matched lifecycle control call
        // this private method, before the explicit outcome leaves the install.
        unsafe { (*self.irq.get()).assume_init_ref() }
    }

    fn detach_entry(&self) {
        let irq = self.irq();
        (irq.route.mask)(&irq.route.platform);
        irq.route.slot.open.store(false, Ordering::SeqCst);
        (irq.route.acknowledge)(&irq.route.platform);
        // Keep the address while a previously entered actor may still use it.
        // Ordinary finish joins before extraction; core quarantine never
        // extracts. Closed entry rejects all later ISR/driver projections.
    }

    fn close_and_stop(&self, bound: DrainBound) -> Closed {
        self.detach_entry();
        let irq = self.irq();
        let quiescence = irq.route.slot.close(bound, &*irq.route);
        (irq.route.acknowledge)(&irq.route.platform);
        if matches!(quiescence, Quiescence::TimedOut) {
            return Closed::TimedOut;
        }
        irq.route
            .slot
            .entry
            .store(ptr::null_mut(), Ordering::Release);
        if irq.lifecycle.snapshot() != DriverStatus::Retired {
            irq.lifecycle.abandon();
        }
        #[allow(
            unsafe_code,
            reason = "zero admitted actors excludes the driver's only mutable borrow"
        )]
        // SAFETY: admission is closed and every previous projection exited.
        // The stop callback cannot reenter and the permanent lease stays live.
        let driver = unsafe { (*self.driver.get()).assume_init_mut() };
        let mut control = IrqControl {
            owner: &irq.interrupts,
        };
        let outcome = (driver.callbacks.stop)(&mut driver.hardware, &mut control);
        // Every actor drained and stop returned, so this has exclusive
        // access. Erase after either DMA fact: the core's fail-stop and
        // whole-run cancel quarantine Driver even after a quiescent stop
        // (), and Held::finish only finds these records already withdrawn.
        sealed::OwnsDma::erase_pending_keys(&mut *driver.hardware);
        Closed::Stopped(outcome)
    }

    fn abandon(&self, bound: DrainBound) -> Closed {
        // The core grants one abandon call and then quarantines Driver
        // without Drop. This path never extracts the initialized regions.
        let irq = self.irq();
        irq.lifecycle.abandon();
        irq.route
            .schedule
            .schedule(&irq.route.platform, WakePlan::Ready);
        self.close_and_stop(bound)
    }

    #[allow(
        unsafe_code,
        reason = "consuming the unique static borrow permanently pins its backing"
    )]
    pub fn pin_static(storage: &'static mut Self) -> ScopeLease<H, I, P, E, S> {
        // SAFETY: the affine lease never exposes mutable storage or a way to
        // unpin it. Released returns that same lease; Poisoned never does.
        let memory = unsafe { Pin::new_unchecked(storage) };
        ScopeLease { memory }
    }
}
impl<
    H: OwnsDma + Send + 'static,
    I: InterruptOwner,
    P: Sync + 'static,
    E: 'static,
    S: WakeScheduler<P>,
> Default for ScopeStorage<H, I, P, E, S>
{
    fn default() -> Self {
        Self::new()
    }
}
#[must_use = "retain the unique pinned SRAM lease until release or permanent quarantine"]
pub struct ScopeLease<
    H: OwnsDma + Send + 'static,
    I: InterruptOwner = ActiveEpoch<Moderated>,
    P: Sync + 'static = (),
    E: 'static = core::convert::Infallible,
    S: WakeScheduler<P> = fn(&P, WakePlan),
> {
    memory: Pin<&'static mut ScopeStorage<H, I, P, E, S>>,
}
impl<
    H: OwnsDma + Send + 'static,
    I: InterruptOwner,
    P: Sync + 'static,
    E: 'static,
    S: WakeScheduler<P>,
> ScopeLease<H, I, P, E, S>
{
    fn storage(&self) -> &ScopeStorage<H, I, P, E, S> {
        self.memory.as_ref().get_ref()
    }
}

/// Register ownership is separate from both Driver and its &mut borrow.
pub struct IrqControl<'scope, I: InterruptOwner = ActiveEpoch<Moderated>> {
    owner: &'scope UnsafeCell<ManuallyDrop<I>>,
}
impl IrqControl<'_, ActiveEpoch<Moderated>> {
    fn with_owner<R>(&mut self, operation: impl FnOnce(&mut ActiveEpoch<Moderated>) -> R) -> R {
        critical_section::with(|_| {
            #[allow(
                unsafe_code,
                reason = "IRQ-bank projections share the bounded register transaction"
            )]
            // SAFETY: admission retains backing; the critical section excludes
            // the hard handlers. This private helper calls only these MMIO leaves.
            operation(unsafe { &mut *self.owner.get() })
        })
    }
    pub fn unmask_rx_after_drain(&mut self) {
        self.with_owner(|irq| irq.unmask_rx_after_drain());
    }
    pub fn prepare_connected_sta_without_power_save(
        &mut self,
        radio: &mut RadioRuntimeOwner,
    ) -> ConnectedStaInterruptPrepared {
        self.with_owner(|irq| irq.prepare_connected_sta_without_power_save(radio))
    }
    pub fn mask_and_acknowledge_power_wake_cause(&mut self, cause: MacPowerWakeCause) {
        self.with_owner(|irq| irq.mask_and_acknowledge_power_wake_cause(cause));
    }
}

// Lifetime-free wake tails cannot address installation memory after wake.
struct MacPublication {
    snapshot: IrqSnapshot,
    wake: IrqPublication,
}
struct PowerPublication {
    snapshot: PowerIrqSnapshot,
    wake: IrqPublication,
}
impl MacPublication {
    #[inline(always)]
    fn wake(self) -> IrqSnapshot {
        self.wake.wake();
        self.snapshot
    }
}
impl PowerPublication {
    #[inline(always)]
    fn wake(self) -> PowerIrqSnapshot {
        self.wake.wake();
        self.snapshot
    }
}
struct Header {
    driver: unsafe fn(*mut ()) -> Option<Waker>,
    driver_context: *mut (),
    mac: unsafe fn(*mut ()) -> MacPublication,
    power: unsafe fn(*mut ()) -> PowerPublication,
    schedule: unsafe fn(*mut ()) -> Scheduled,
    irq_context: *mut (),
}
// Distinct private result also identifies this callback family in the linked
// SRAM audit; no untyped function-pointer allowlist is needed.
struct Scheduled {
    _published: bool,
}

#[allow(
    unsafe_code,
    reason = "the admitted wake callback sees only the separate IRQ region"
)]
#[cfg_attr(
    target_arch = "riscv32",
    unsafe(link_section = ".rwtext.open_radio_irq")
)]
unsafe fn schedule_entry<I: InterruptOwner, P: Sync + 'static, E: 'static, S: WakeScheduler<P>>(
    context: *mut (),
) -> Scheduled {
    // SAFETY: the same count/admission proof as the IRQ capture callbacks;
    // the exact region projection and function were published together.
    let irq = unsafe { &*context.cast::<IrqRegion<I, P, E, S>>() };
    irq.route
        .schedule
        .schedule(&irq.route.platform, WakePlan::Ready);
    Scheduled { _published: true }
}

// The hard entries know only this type. It contains no H, DriverRegion,
// pointer to DriverRegion, context, latch or future reference.
struct IrqRegion<I: InterruptOwner, P: Sync + 'static, E: 'static, S: WakeScheduler<P>> {
    interrupts: UnsafeCell<ManuallyDrop<I>>,
    mac: IrqPublisher<'static>,
    power: PowerIrqPublisher<'static>,
    route: ManuallyDrop<ScopeRoute<P, E, S>>,
    lifecycle: DriverLifecycle,
    completion: AtomicWaker,
}
struct DriverRegion<
    H: OwnsDma + Send + 'static,
    I: InterruptOwner,
    P: Sync + 'static,
    E: 'static,
    S: WakeScheduler<P>,
> {
    hardware: ManuallyDrop<H>,
    mac: IrqConsumer<'static>,
    power: PowerIrqConsumer<'static>,
    irq: *const IrqRegion<I, P, E, S>,
    callbacks: DriverCallbacks<H, I>,
}

/// The bounded hardware operations bound by the installation's safety proof.
pub struct DriverCallbacks<H: OwnsDma, I: InterruptOwner> {
    step: fn(&mut H, &mut IrqControl<'_, I>, DriverEvents, DriverStatus) -> DriverStep,
    stop: fn(&mut H, &mut IrqControl<'_, I>) -> StopOutcome,
}
impl<H: OwnsDma, I: InterruptOwner> DriverCallbacks<H, I> {
    pub const fn new(
        step: fn(&mut H, &mut IrqControl<'_, I>, DriverEvents, DriverStatus) -> DriverStep,
        stop: fn(&mut H, &mut IrqControl<'_, I>) -> StopOutcome,
    ) -> Self {
        Self { step, stop }
    }
}

#[allow(
    unsafe_code,
    reason = "only the exclusive admitted executor projects DriverRegion"
)]
unsafe fn invoke<
    H: OwnsDma + Send + 'static,
    I: InterruptOwner,
    P: Sync + 'static,
    E: 'static,
    S: WakeScheduler<P>,
>(
    context: *mut (),
) -> Option<Waker> {
    // SAFETY: header and exact callback are bound together; admitted steps
    // are serialized. IRQ callbacks cannot reach this DriverRegion.
    let driver = unsafe { &mut *context.cast::<DriverRegion<H, I, P, E, S>>() };
    // SAFETY: the admission count retains this separate shared IRQ region.
    let irq = unsafe { &*driver.irq };
    let status = irq.lifecycle.snapshot();
    if matches!(status, DriverStatus::Retired | DriverStatus::Abandoned) {
        return None;
    }
    // The permanent slot waker covers software and hardware publications.
    // Registration precedes each Acquire snapshot, never follows its check.
    let waker = irq.route.slot.driver_waker();
    let mut cx = Context::from_waker(&waker);
    let events = DriverEvents {
        mac: match driver.mac.poll_batch(&mut cx) {
            Poll::Ready(batch) => batch,
            Poll::Pending => IrqBatch::default(),
        },
        power: match driver.power.poll(&mut cx) {
            Poll::Ready(observation) => observation,
            Poll::Pending => MacPowerInterruptObservation::default(),
        },
    };
    let mut control = IrqControl {
        owner: &irq.interrupts,
    };
    match (driver.callbacks.step)(&mut driver.hardware, &mut control, events, status) {
        DriverStep::Wake(plan) => {
            irq.route.schedule.schedule(&irq.route.platform, plan);
            // Arm before checking. A fact published before the Acquire is
            // visible here; a later publication pends the executor itself.
            // The captured batch stays finite: this work belongs to next step.
            if driver.mac.has_pending()
                || driver.power.has_pending()
                || irq.lifecycle.snapshot() != DriverStatus::Running
            {
                irq.route
                    .schedule
                    .schedule(&irq.route.platform, WakePlan::Ready);
            }
            None
        }
        DriverStep::Retired => {
            irq.lifecycle.retired();
            irq.completion.take()
        }
    }
}

#[allow(
    unsafe_code,
    reason = "the admitted hard callback projects only the disjoint IRQ region"
)]
#[cfg_attr(
    target_arch = "riscv32",
    unsafe(link_section = ".rwtext.open_radio_irq")
)]
unsafe fn mac_entry<I: InterruptOwner, P: Sync + 'static, E: 'static, S: WakeScheduler<P>>(
    context: *mut (),
) -> MacPublication {
    // SAFETY: admission retains this exact IRQ region; the caller holds its
    // register critical section. No driver address is present in this type.
    let irq = unsafe { &*context.cast::<IrqRegion<I, P, E, S>>() };
    let measurement = irq.mac.begin_mac_entry();
    // SAFETY: every bank projection shares the same critical section.
    let snapshot = unsafe { (&mut *irq.interrupts.get()).mac() };
    let wake = irq.mac.publish_snapshot_deferred(snapshot);
    MacPublication {
        snapshot,
        wake: measurement.finish(wake),
    }
}
#[allow(
    unsafe_code,
    reason = "power shares the same admission and disjoint IRQ-bank proof"
)]
#[cfg_attr(
    target_arch = "riscv32",
    unsafe(link_section = ".rwtext.open_radio_irq")
)]
unsafe fn power_entry<I: InterruptOwner, P: Sync + 'static, E: 'static, S: WakeScheduler<P>>(
    context: *mut (),
) -> PowerPublication {
    // SAFETY: same exact-type, admission and bank-serialization proof as MAC.
    let irq = unsafe { &*context.cast::<IrqRegion<I, P, E, S>>() };
    // SAFETY: the caller serializes every IRQ-bank projection.
    let snapshot = unsafe { (&mut *irq.interrupts.get()).power() };
    let wake = irq.power.publish_deferred(snapshot.observation);
    PowerPublication { snapshot, wake }
}

/// The future owns only the lease and caller data. The permanent backing owns
/// every registered byte, even after Poisoned, cancellation and owning-Pin drop.
#[must_use]
pub struct DriverScope<
    'latch,
    H: OwnsDma + Send + 'static,
    I: InterruptOwner = ActiveEpoch<Moderated>,
    P: Sync + 'static = (),
    E: 'static = core::convert::Infallible,
    S: WakeScheduler<P> = fn(&P, WakePlan),
> {
    lease: ManuallyDrop<ScopeLease<H, I, P, E, S>>,
    latch: &'latch mut ScopeReturnLatch<H, I, P, E, S>,
    bound: DrainBound,
}
impl<
    'latch,
    H: OwnsDma + Send + 'static,
    I: InterruptOwner,
    P: Sync + 'static,
    E: 'static,
    S: WakeScheduler<P>,
> DriverScope<'latch, H, I, P, E, S>
{
    /// Bind one installation to its unique permanent SRAM lease.
    ///
    /// # Safety
    /// Retain the unique platform routing tokens for this slot. Mask disables
    /// MAC/POWER and driver-SWI admission at the hardware routes on all bound
    /// cores; acknowledge clears the SWI. The separate driver executor is
    /// exclusive, including across cores. IRQ-bank operations are serialized
    /// by the platform's cross-core critical-section implementation.
    /// The route's Clock reads a monotonic source that continues advancing
    /// while MAC/POWER and driver-SWI are masked, on every participating core.
    /// Its read is bounded and does not wait for one of those actors. The
    /// bound includes worst-case step/IRQ time and a scheduling/resolution
    /// margin; no CPU-frequency-dependent spin count is a time bound.
    /// Step/stop and routing callbacks are bounded and do not unwind,
    /// synchronously poll, reenter driver dispatch or destroy the scope.
    /// IRQ preemption is permitted. Step reports Retired only after observing
    /// RetireRequested and accounting for all accepted work; stop stops
    /// hardware and DMA, runs once after quiescence, and returns
    /// `StopOutcome::Quiescent` only on the reading `DmaQuiescent` requires.
    /// It is never invoked on timeout. Schedulers
    /// and composition wakers (including clone/drop) ONLY pend execution: they
    /// never synchronously poll or detach. All reachable callback data survive
    /// forget and lie in the linked SRAM closure. No priority order is a safety
    /// premise. The lease was placed in SRAM by the composition's linker.
    #[allow(
        unsafe_code,
        reason = "one permanent registration primitive binds all external premises"
    )]
    pub unsafe fn new(
        lease: ScopeLease<H, I, P, E, S>,
        hardware: H,
        interrupts: I,
        callbacks: DriverCallbacks<H, I>,
        route: ScopeRoute<P, E, S>,
        bound: DrainBound,
        latch: &'latch mut ScopeReturnLatch<H, I, P, E, S>,
    ) -> Self {
        let storage = lease.storage();
        // SAFETY: an unused/Released lease has no live handoff borrower. A new
        // installation begins with its own empty facts, diagnostics and wakers.
        unsafe {
            *storage.mac.get() = IrqStorage::new();
            *storage.power.get() = PowerIrqStorage::new();
        }
        // SAFETY: this affine, unused/Released lease has no live projection.
        // These fields physically live forever. Consumers and publishers are
        // private to this initialized region, destroyed before lease return.
        let (mac, mac_consumer) = unsafe { (&mut *storage.mac.get()).split() };
        // SAFETY: the distinct permanent power handoff has the same ownership.
        let (power, power_consumer) = unsafe { (&mut *storage.power.get()).split() };
        // SAFETY: all three destinations are uninitialized under this lease.
        let irq = unsafe {
            (*storage.irq.get()).write(IrqRegion {
                interrupts: UnsafeCell::new(ManuallyDrop::new(interrupts)),
                mac,
                power,
                route: ManuallyDrop::new(route),
                lifecycle: DriverLifecycle::new(),
                completion: AtomicWaker::new(),
            })
        };
        // SAFETY: independent driver storage, initialized once for this lease.
        let driver = unsafe {
            (*storage.driver.get()).write(DriverRegion {
                hardware: ManuallyDrop::new(hardware),
                mac: mac_consumer,
                power: power_consumer,
                irq,
                callbacks,
            })
        };
        // SAFETY: the exact projections and callbacks are published together.
        unsafe {
            (*storage.header.get()).write(Header {
                driver: invoke::<H, I, P, E, S>,
                driver_context: ptr::from_mut(driver).cast(),
                mac: mac_entry::<I, P, E, S>,
                power: power_entry::<I, P, E, S>,
                schedule: schedule_entry::<I, P, E, S>,
                irq_context: ptr::from_mut(irq).cast(),
            });
        }
        Self {
            lease: ManuallyDrop::new(lease),
            latch,
            bound,
        }
    }

    #[allow(unsafe_code, reason = "the live lease owns one initialized IRQ region")]
    fn irq(&self) -> &IrqRegion<I, P, E, S> {
        // SAFETY: new initializes it; Drop extracts only after quiescence.
        unsafe { (*self.lease.storage().irq.get()).assume_init_ref() }
    }

    fn start(&self) -> Result<(), E> {
        let irq = self.irq();
        // run_port polls its body before drive. fail_stop may therefore have
        // closed this installation before the driver's first poll.
        if irq.lifecycle.snapshot() != DriverStatus::Abandoned {
            (irq.route.mask)(&irq.route.platform);
            (irq.route.acknowledge)(&irq.route.platform);
            let header = self.lease.storage().header.get().cast::<Header>();
            irq.route.slot.entry.store(header, Ordering::Release);
            irq.route.slot.open.store(true, Ordering::SeqCst);
            (irq.route.enable)(&irq.route.platform)?;
            irq.route
                .schedule
                .schedule(&irq.route.platform, WakePlan::Ready);
        }
        Ok(())
    }

    async fn retire(&self) {
        let irq = self.irq();
        if irq.lifecycle.snapshot() == DriverStatus::Abandoned {
            return;
        }
        irq.lifecycle.request_retire();
        irq.route
            .schedule
            .schedule(&irq.route.platform, WakePlan::Ready);
        poll_fn(|cx| {
            irq.completion.register(cx.waker());
            if matches!(
                irq.lifecycle.snapshot(),
                DriverStatus::Retired | DriverStatus::Abandoned
            ) {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
        .await;
    }

    fn close_and_stop(&self) -> Closed {
        self.lease.storage().close_and_stop(self.bound)
    }

    /// Transfer the active scope to its caller body. The body consumes its
    /// access through retire or abandon; dropping active access cancels it.
    /// After consuming abandon, pending body work owns no shutdown guard.
    pub async fn run<C, R>(
        self,
        context: C,
        body: impl for<'scope> AsyncFnOnce(C, ScopeAccess<'scope, H, I, P, E, S>) -> R,
    ) -> Result<R, E> {
        self.start()?;
        Ok(body(context, ScopeAccess { scope: self }).await)
    }

    /// Consume the scope through ordinary explicit teardown. No outcome relies
    /// on Drop; a timed-out actor keeps the permanent backing in quarantine.
    pub fn finish(self) -> ScopeReturn<H, I, P, E, S> {
        ManuallyDrop::new(self).complete()
    }

    fn complete(&mut self) -> ScopeReturn<H, I, P, E, S> {
        let quiescence = self.close_and_stop();
        self.complete_after(quiescence)
    }

    fn complete_after(&mut self, closed: Closed) -> ScopeReturn<H, I, P, E, S> {
        let stop = match closed {
            Closed::Stopped(stop) => stop,
            Closed::TimedOut => {
                // Do not inspect, move or drop anything an admitted actor can
                // use. Losing this affine lease leaves its backing quarantined.
                return ScopeReturn::Poisoned(ScopePoisoned { _private: () });
            }
        };
        #[allow(
            unsafe_code,
            reason = "closed admission and zero actors permit unique owner recovery"
        )]
        // SAFETY: all publishers/consumers/owners are initialized and no actor
        // can project them. Moving these values destroys the private borrows
        // before the affine lease returns; its backing remains pinned forever.
        let (mut driver, mut irq, lease) = unsafe {
            let storage = self.lease.storage();
            (
                (*storage.driver.get()).assume_init_read(),
                (*storage.irq.get()).assume_init_read(),
                ManuallyDrop::take(&mut self.lease),
            )
        };
        #[allow(
            unsafe_code,
            reason = "the drained initialized owners move out exactly once"
        )]
        // SAFETY: the sole extraction above detached every actor; the local
        // ManuallyDrop wrappers prevent a second destruction of each owner.
        let owners = unsafe {
            ReturnedOwners {
                hardware: ManuallyDrop::take(&mut driver.hardware),
                interrupts: ManuallyDrop::take(irq.interrupts.get_mut()).detach(),
                route: ManuallyDrop::take(&mut irq.route),
            }
        };
        // The DMA fact decides, whatever path stopped the driver: retire,
        // fail-stop and a lost owner all return owners after a quiescent stop.
        let owners = match stop {
            StopOutcome::Quiescent(DmaQuiescent { _private: () }) => {
                DetachedOwners::Stopped(StoppedOwners { owners })
            }
            StopOutcome::Unconfirmed => DetachedOwners::Unconfirmed(Quarantined::new(owners)),
        };
        // The non-Drop driver projection ends with this function, before the
        // returned lease can be used. IRQ's owned waker is destroyed here.
        drop(irq);
        ScopeReturn::Released(ReleasedScope { lease, owners })
    }
}
impl<
    H: OwnsDma + Send + 'static,
    I: InterruptOwner,
    P: Sync + 'static,
    E: 'static,
    S: WakeScheduler<P>,
> Drop for DriverScope<'_, H, I, P, E, S>
{
    fn drop(&mut self) {
        self.latch.returned = Some(self.complete());
    }
}

#[cfg(test)]
mod tests;
