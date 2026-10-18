extern crate std;

mod forget;
mod lifecycle;
mod work;

use super::*;
use core::{cell::RefCell, convert::Infallible, future::Future, task::Context};
use std::{
    boxed::Box,
    sync::{Arc, Barrier, Mutex},
    task::Wake,
    thread,
};

#[derive(Clone, Default)]
struct Count(Arc<AtomicUsize>);
impl Count {
    fn get(&self) -> usize {
        self.0.load(Ordering::SeqCst)
    }
    fn increment(&self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[derive(Default)]
struct Routing {
    masked: AtomicBool,
    pends: Count,
    acks: Count,
    last_ready: AtomicBool,
    clock: Mutex<oer_time_virtual::VirtualClock>,
    sample_elapsed_us: AtomicUsize,
}
type Platform = Arc<Routing>;
type Schedule = fn(&Platform, WakePlan);
type Storage<E = Infallible> = ScopeStorage<Hardware, Interrupts, Platform, E, Schedule>;
type Lease<E = Infallible> = ScopeLease<Hardware, Interrupts, Platform, E, Schedule>;
type Latch<E = Infallible> = ScopeReturnLatch<Hardware, Interrupts, Platform, E, Schedule>;
type Return<E = Infallible> = ScopeReturn<Hardware, Interrupts, Platform, E, Schedule>;
fn now(p: &Platform) -> Instant {
    // A virtual hardware clock: tests choose elapsed time between reads,
    // independently of the number or cost of spin instructions.
    let clock = p.clock.lock().unwrap();
    clock
        .advance(Duration::from_micros(
            p.sample_elapsed_us.load(Ordering::Relaxed) as u64,
        ))
        .unwrap();
    clock.now()
}
fn mask(p: &Platform) {
    p.masked.store(true, Ordering::SeqCst);
}
fn acknowledge(p: &Platform) {
    p.acks.increment();
}
fn enable(p: &Platform) -> Result<(), Infallible> {
    p.masked.store(false, Ordering::SeqCst);
    Ok(())
}
fn schedule(p: &Platform, plan: WakePlan) {
    p.pends.increment();
    p.last_ready
        .store(matches!(plan, WakePlan::Ready), Ordering::Release);
    if matches!(plan, WakePlan::Wait { .. }) {
        let hook = WAIT_ARM.with(|slot| slot.borrow_mut().take());
        if let Some(hook) = hook {
            hook();
        }
    }
}
fn make_route() -> (
    ScopeRoute<Platform, Infallible>,
    &'static ScopeSlot,
    Platform,
) {
    make_route_on(Arc::new(Routing::default()))
}
fn make_route_on(
    platform: Platform,
) -> (
    ScopeRoute<Platform, Infallible>,
    &'static ScopeSlot,
    Platform,
) {
    let slot = Box::leak(Box::new(ScopeSlot::new()));
    platform.sample_elapsed_us.store(1, Ordering::Relaxed);
    (
        ScopeRoute::new(
            platform.clone(),
            slot,
            mask,
            acknowledge,
            enable,
            schedule as Schedule,
            now,
        ),
        slot,
        platform,
    )
}
fn lease<E: 'static>() -> Lease<E> {
    Storage::pin_static(Box::leak(Box::new(Storage::new())))
}

// A deterministic scheduler point inside the bounded wait. The hook can let
// another core finish before the next observation; no wall-clock sleeps.
std::thread_local! {
    static WAIT_BOUNDARY: RefCell<Option<Box<dyn FnOnce()>>> = const { RefCell::new(None) };
    static WAIT_ARM: RefCell<Option<Box<dyn FnOnce()>>> = const { RefCell::new(None) };
}
pub(super) fn wait_boundary() {
    let hook = WAIT_BOUNDARY.with(|slot| slot.borrow_mut().take());
    if let Some(hook) = hook {
        hook();
    }
}

struct Block {
    entered: Barrier,
    resume: Barrier,
}
impl Block {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            entered: Barrier::new(2),
            resume: Barrier::new(2),
        })
    }
    fn actor(&self) {
        self.entered.wait();
        self.resume.wait();
    }
}
pub(crate) struct Hardware {
    work: work::DmaQueue,
    steps: Count,
    drops: Count,
    stops: Count,
    facts: Count,
    capacity: Count,
    accesses_after_pause: Count,
    block: Option<Arc<Block>>,
    irq_during_step: Option<&'static ScopeSlot>,
    /// Whether stop reads its DMA as quiescent.
    dma_quiescent: bool,
    /// The model DMA owner a bridge executor installed with it reaches.
    dma: BridgeDma,
    platform: Platform,
}
/// The model DMA owner the bridge tests bind their executors for.
pub(crate) type BridgeDma = commands::bridge::tests::Hardware;
pub(crate) type BridgeExecutor = commands::bridge::Executor<BridgeDma, 4, 15>;
/// The fixture hardware with a bridge executor, as S31 installs it.
pub(crate) type Bridge = commands::bridge::Bridged<Hardware, 4, 15>;
/// Test-only: the fixture hardware owns no executor of its own; with one it
/// is installed through [`Bridge`].
impl sealed::OwnsDma for Hardware {
    type Dma = BridgeDma;
    fn dma(&mut self) -> &mut BridgeDma {
        &mut self.dma
    }
    fn erase_pending_keys(&mut self) {}
}
impl Drop for Hardware {
    fn drop(&mut self) {
        self.drops.increment();
    }
}
fn hardware(platform: &Platform) -> Hardware {
    Hardware {
        work: work::DmaQueue::default(),
        steps: Count::default(),
        drops: Count::default(),
        stops: Count::default(),
        facts: Count::default(),
        capacity: Count::default(),
        accesses_after_pause: Count::default(),
        block: None,
        irq_during_step: None,
        dma_quiescent: true,
        dma: BridgeDma::default(),
        platform: platform.clone(),
    }
}
fn step(
    h: &mut Hardware,
    _: &mut IrqControl<Interrupts>,
    events: DriverEvents,
    signals: DriverStatus,
) -> DriverStep {
    assert_eq!(h.stops.get(), 0);
    h.work.assert_owned();
    if events.mac.rx_capacity_returned() {
        h.capacity.increment();
    }
    h.facts.0.fetch_add(events.mac.count(), Ordering::SeqCst);
    let _handled = events.power;
    h.steps.increment();
    if let Some(block) = &h.block {
        block.actor();
        h.accesses_after_pause.increment();
    }
    if let Some(slot) = h.irq_during_step {
        assert!(slot.mac_interrupt().had_status);
    }
    if signals == DriverStatus::RetireRequested {
        h.work.drain();
        DriverStep::Retired
    } else {
        DriverStep::Wake(WakePlan::until(
            Instant::from_micros(0),
            Instant::from_micros(100),
            WakeSources::CommandsAndInterrupts,
        ))
    }
}
fn stop(h: &mut Hardware, _: &mut IrqControl<Interrupts>) -> StopOutcome {
    assert!(h.platform.masked.load(Ordering::SeqCst));
    assert_eq!(
        h.stops.get(),
        0,
        "stop runs exactly once after all actors exit"
    );
    h.stops.increment();
    h.work.drain();
    if h.dma_quiescent {
        #[allow(unsafe_code, reason = "the model's DMA queue was just drained")]
        // SAFETY: the model has no hardware DMA; its queue was drained above.
        StopOutcome::Quiescent(unsafe { DmaQuiescent::confirmed() })
    } else {
        StopOutcome::Unconfirmed
    }
}
#[derive(Default)]
struct Interrupts {
    mac_reads: Count,
    power_reads: Count,
    detached: Count,
    drops: Count,
    accesses_after_pause: Count,
    block: Option<Arc<Block>>,
    driver_during_irq: Option<&'static ScopeSlot>,
}
impl Drop for Interrupts {
    fn drop(&mut self) {
        self.drops.increment();
    }
}
impl sealed::InterruptOwner for Interrupts {}
impl InterruptOwner for Interrupts {
    type Detached = ();
    fn mac(&mut self) -> IrqSnapshot {
        self.mac_reads.increment();
        if let Some(slot) = self.driver_during_irq {
            dispatch(slot);
        }
        if let Some(block) = &self.block {
            block.actor();
            self.accesses_after_pause.increment();
        }
        IrqSnapshot {
            had_status: true,
            posted_events: oer_esp32s31_ieee80211_mac::irq::EVENT_RX_SUCCESS,
            ..IrqSnapshot::default()
        }
    }
    fn power(&mut self) -> PowerIrqSnapshot {
        self.power_reads.increment();
        if let Some(block) = &self.block {
            block.actor();
            self.accesses_after_pause.increment();
        }
        PowerIrqSnapshot {
            observation: MacPowerInterruptObservation::from_semantic_events(
                false, false, false, false, true, false,
            ),
        }
    }
    fn detach(self) {
        self.detached.increment();
    }
}

#[allow(
    unsafe_code,
    reason = "the harness binds its unique slot, permanent lease and exclusive executor"
)]
fn new_scope<'a, E: 'static>(
    lease: Lease<E>,
    h: Hardware,
    irq: Interrupts,
    route: ScopeRoute<Platform, E>,
    latch: &'a mut Latch<E>,
) -> DriverScope<'a, Hardware, Interrupts, Platform, E> {
    // SAFETY: each test owns one route and one permanent backing; the only
    // dispatch is serialized. IRQ-bank access uses the host cross-core CS.
    // Callbacks do not poll, destroy or reenter the scope. Barriers model a
    // delayed actor; timeout must retain its backing without calling stop.
    unsafe {
        DriverScope::new(
            lease,
            h,
            irq,
            DriverCallbacks::new(step, stop),
            route,
            DrainBound::from_maxima::<100, 20, 30>(),
            latch,
        )
    }
}
fn dispatch(slot: &ScopeSlot) {
    #[allow(unsafe_code, reason = "tests never overlap two driver dispatches")]
    // SAFETY: tests permit one driver actor, with no external hardware borrow.
    unsafe {
        slot.dispatch()
    };
}
fn poll<F: Future>(future: Pin<&mut F>) -> Poll<F::Output> {
    future.poll(&mut Context::from_waker(Waker::noop()))
}
fn released(
    result: Option<Return>,
) -> ReleasedScope<Hardware, Interrupts, Platform, Infallible, Schedule> {
    match result {
        Some(ScopeReturn::Released(released)) => released,
        _ => panic!("expected Released"),
    }
}

/// Fixture hardware for [`Bridge::bind`], on its own platform.
pub(crate) fn fixture_hardware() -> Hardware {
    hardware(&Arc::new(Routing::default()))
}

/// Stop one scope (retired, or fail-stopped through abandon) with a DMA
/// fact of `quiescent`, and lend its stop proof, if any, to `with`. Stores in
/// other modules use it to finish their owners after a real stop.
pub(crate) fn with_stopped_scope<R>(
    abandon: bool,
    quiescent: bool,
    with: impl FnOnce(Option<&DriverStopped<'_, BridgeDma>>) -> R,
) -> R {
    let mut h = fixture_hardware();
    h.dma_quiescent = quiescent;
    let platform = h.platform.clone();
    stop_scope(
        h,
        platform,
        DriverCallbacks::new(step, stop),
        abandon,
        |owners| match owners {
            DetachedOwners::Stopped(owners) => with(Some(&owners.stopped())),
            DetachedOwners::Unconfirmed(_) => with(None),
        },
    )
}

/// [`with_stopped_scope`] with `bridge` installed as the driver's hardware:
/// a quiescent stop lends its executor together with the proof
/// ([`StoppedOwners::parts`]); an unconfirmed one quarantines it after the
/// form erased its pending keys.
pub(crate) fn with_stopped_scope_owning<R>(
    abandon: bool,
    quiescent: bool,
    mut bridge: Bridge,
    with: impl FnOnce(Option<(&mut BridgeExecutor, &DriverStopped<'_, BridgeDma>)>) -> R,
) -> R {
    fn bridged_step(
        b: &mut Bridge,
        irq: &mut IrqControl<Interrupts>,
        events: DriverEvents,
        signals: DriverStatus,
    ) -> DriverStep {
        step(b.hardware(), irq, events, signals)
    }
    fn bridged_stop(b: &mut Bridge, irq: &mut IrqControl<Interrupts>) -> StopOutcome {
        stop(b.hardware(), irq)
    }
    bridge.hardware().dma_quiescent = quiescent;
    let platform = bridge.hardware().platform.clone();
    let callbacks = DriverCallbacks::new(bridged_step, bridged_stop);
    stop_scope(
        bridge,
        platform,
        callbacks,
        abandon,
        |owners| match owners {
            DetachedOwners::Stopped(mut owners) => {
                let (executor, stopped) = owners.parts();
                with(Some((executor, &stopped)))
            }
            DetachedOwners::Unconfirmed(_) => with(None),
        },
    )
}

#[allow(
    unsafe_code,
    reason = "the harness binds its unique slot, permanent lease and exclusive executor"
)]
fn stop_scope<H: OwnsDma + Send + 'static, R>(
    h: H,
    platform: Platform,
    callbacks: DriverCallbacks<H, Interrupts>,
    abandon: bool,
    with: impl FnOnce(DetachedOwners<ReturnedOwners<H, (), Platform, Infallible, Schedule>>) -> R,
) -> R {
    let (route, slot, _) = make_route_on(platform);
    let lease = ScopeStorage::<H, Interrupts, Platform, Infallible, Schedule>::pin_static(
        Box::leak(Box::new(ScopeStorage::new())),
    );
    let mut latch = ScopeReturnLatch::new();
    // SAFETY: as new_scope: one route, one permanent backing, one serialized
    // dispatch; the callbacks only forward to the fixture's.
    let scope = unsafe {
        DriverScope::new(
            lease,
            h,
            Interrupts::default(),
            callbacks,
            route,
            DrainBound::from_maxima::<100, 20, 30>(),
            &mut latch,
        )
    };
    let mut future = Box::pin(scope.run((), async move |(), access| {
        if abandon {
            let _abandoned = access.abandon();
        } else {
            access.retire(()).await;
        }
    }));
    if poll(future.as_mut()).is_pending() {
        dispatch(slot);
        assert!(poll(future.as_mut()).is_ready());
    }
    drop(future);
    match latch.into_return() {
        Some(ScopeReturn::Released(released)) => with(released.owners),
        _ => panic!("expected Released"),
    }
}

#[test]
fn a_lost_owned_driver_releases_after_a_quiescent_stop_and_poisons_otherwise() {
    //  : the proof covers a lost owner alike. Dropping an owned
    // Driver outside core quarantine abandons it; the DMA fact decides.
    for quiescent in [true, false] {
        let (route, _slot, platform) = make_route();
        let mut h = hardware(&platform);
        h.dma_quiescent = quiescent;
        let drops = h.drops.clone();
        let mut latch = Latch::new();
        let driver = driver::Driver::new(new_scope(
            lease(),
            h,
            Interrupts::default(),
            route,
            &mut latch,
        ));
        drop(driver);
        match latch.into_return() {
            Some(ScopeReturn::Released(released)) => {
                assert!(quiescent, "only a quiescent stop releases");
                assert!(matches!(released.owners, DetachedOwners::Stopped(_)));
                drop(released);
                assert_eq!(drops.get(), 1, "released owners drop normally");
            }
            Some(ScopeReturn::Poisoned(_)) => {
                assert!(!quiescent, "an unconfirmed stop stays quarantined");
                assert_eq!(drops.get(), 0);
            }
            None => panic!("a lost driver always reports its outcome"),
        }
    }
}

#[test]
fn completion_keeps_result_until_separate_retirement_and_stops_once() {
    let (route, slot, platform) = make_route();
    let h = hardware(&platform);
    let (steps, stops, drops) = (h.steps.clone(), h.stops.clone(), h.drops.clone());
    let mut latch = Latch::new();
    let mut context = 5;
    let scope = new_scope(lease(), h, Interrupts::default(), route, &mut latch);
    let mut future = Box::pin(scope.run(&mut context, async |context, access| {
        *context += 1;
        access.retire(*context).await
    }));
    assert!(poll(future.as_mut()).is_pending());
    assert_eq!(steps.get(), 0);
    assert_eq!(
        platform.pends.get(),
        2,
        "start and retire both wake the separate driver"
    );
    dispatch(slot);
    assert_eq!(steps.get(), 1);
    assert_eq!(
        stops.get(),
        0,
        "retirement does not bypass the quiescence boundary"
    );
    assert_eq!(poll(future.as_mut()), Poll::Ready(Ok(6)));
    drop(future);
    assert_eq!(context, 6);
    assert_eq!(stops.get(), 1);
    assert!(platform.masked.load(Ordering::SeqCst));
    assert!(slot.entry.load(Ordering::Acquire).is_null());
    let returned = released(latch.into_return());
    assert!(matches!(returned.owners, DetachedOwners::Stopped(_)));
    assert_eq!(drops.get(), 0);
    drop(returned);
    assert_eq!(drops.get(), 1);
}

#[test]
fn cancellation_before_and_during_poll_returns_lease_and_owners_after_a_quiescent_stop() {
    for started in [false, true] {
        let (route, slot, platform) = make_route();
        let h = hardware(&platform);
        let (stops, drops) = (h.stops.clone(), h.drops.clone());
        let mut latch = Latch::new();
        let mut context = 0;
        let scope = new_scope(lease(), h, Interrupts::default(), route, &mut latch);
        let mut future = Box::pin(scope.run(&mut context, async |context, _| {
            *context += 1;
            core::future::pending::<()>().await;
        }));
        if started {
            assert!(poll(future.as_mut()).is_pending());
        }
        drop(future);
        assert_eq!(context, usize::from(started));
        assert_eq!(stops.get(), 1);
        assert!(platform.masked.load(Ordering::SeqCst));
        assert!(!slot.mac_interrupt().had_status);
        assert!(slot.power_interrupt().observation.is_empty());
        let returned = released(latch.into_return());
        assert!(
            matches!(returned.owners, DetachedOwners::Stopped(_)),
            "a confirmed stop returns cancelled owners"
        );
        assert_eq!(drops.get(), 0);
        drop(returned);
        assert_eq!(drops.get(), 1);
    }
}

#[test]
fn fail_stop_is_synchronous_and_drop_does_not_stop_twice() {
    for (body_waits, quiescent) in [(false, true), (true, true), (false, false), (true, false)] {
        let (route, slot, platform) = make_route();
        let mut h = hardware(&platform);
        h.dma_quiescent = quiescent;
        let stops = h.stops.clone();
        let mut latch = Latch::new();
        let scope = new_scope(lease(), h, Interrupts::default(), route, &mut latch);
        let mut future = Box::pin(scope.run((), async |(), access| {
            access.abandon();
            assert_eq!(stops.get(), 1);
            assert!(platform.masked.load(Ordering::SeqCst));
            assert!(!slot.open.load(Ordering::SeqCst));
            if body_waits {
                core::future::pending::<()>().await;
            }
            17
        }));
        assert_eq!(poll(future.as_mut()).is_pending(), body_waits);
        dispatch(slot);
        drop(future);
        assert_eq!(stops.get(), 1);
        // The DMA fact decides: a confirmed fail-stop returns its owners, an
        // unconfirmed one keeps them quarantined until reset.
        let owners = released(latch.into_return()).owners;
        if quiescent {
            assert!(matches!(owners, DetachedOwners::Stopped(_)));
        } else {
            assert!(matches!(owners, DetachedOwners::Unconfirmed(_)));
        }
    }
}

#[test]
fn irq_and_driver_backing_survive_owning_pin_forget_without_static_context() {
    let (route, slot, platform) = make_route();
    let h = hardware(&platform);
    let (steps, stops, drops) = (h.steps.clone(), h.stops.clone(), h.drops.clone());
    let irq = Interrupts::default();
    let detached = irq.detached.clone();
    let mut latch = Latch::new();
    {
        let mut context = 5;
        let scope = new_scope(lease(), h, irq, route, &mut latch);
        let mut future = Box::pin(scope.run(&mut context, async |context, _| {
            *context += 1;
            core::future::pending::<()>().await;
        }));
        assert!(poll(future.as_mut()).is_pending());
        core::mem::forget(future);
    }
    assert!(slot.mac_interrupt().had_status);
    assert!(slot.power_interrupt().observation.sta_tbtt());
    dispatch(slot);
    assert_eq!(steps.get(), 1);
    assert_eq!(stops.get(), 0);
    assert_eq!(drops.get(), 0);
    assert_eq!(detached.get(), 0);
    assert!(
        latch.into_return().is_none(),
        "forget fabricates neither release nor poison"
    );
}

#[test]
#[allow(
    clippy::forget_non_drop,
    reason = "regression distinguishes borrowed Pin from ownership of its future"
)]
fn forgetting_borrowed_pin_still_cancels_when_the_future_drops() {
    let (route, slot, platform) = make_route();
    let h = hardware(&platform);
    let stops = h.stops.clone();
    let mut latch = Latch::new();
    {
        let scope = new_scope(lease(), h, Interrupts::default(), route, &mut latch);
        let mut future =
            core::pin::pin!(scope.run((), async |(), _| core::future::pending::<()>().await));
        assert!(poll(future.as_mut()).is_pending());
        core::mem::forget(future);
        assert!(slot.mac_interrupt().had_status);
    }
    assert_eq!(stops.get(), 1);
    assert!(!slot.mac_interrupt().had_status);
    assert!(matches!(
        latch.into_return(),
        Some(ScopeReturn::Released(_))
    ));
}

#[test]
fn cancellation_after_retirement_keeps_owners_and_drops_the_result() {
    struct Output(Count);
    impl Drop for Output {
        fn drop(&mut self) {
            self.0.increment();
        }
    }
    let (route, slot, platform) = make_route();
    let h = hardware(&platform);
    let (drops, stops) = (h.drops.clone(), h.stops.clone());
    let result_drops = Count::default();
    let mut latch = Latch::new();
    let scope = new_scope(lease(), h, Interrupts::default(), route, &mut latch);
    let mut future = Box::pin(
        scope.run(Output(result_drops.clone()), async |output, access| {
            access.retire(output).await
        }),
    );
    assert!(poll(future.as_mut()).is_pending());
    dispatch(slot);
    assert_eq!(result_drops.get(), 0);
    drop(future);
    assert_eq!(result_drops.get(), 1);
    assert_eq!(stops.get(), 1);
    drop(released(latch.into_return()));
    assert_eq!(drops.get(), 1);
}

#[test]
fn wrong_core_does_not_start_body_and_detaches_both_entries() {
    #[derive(Debug, Eq, PartialEq)]
    struct WrongCore;
    fn wrong_core(_: &Platform) -> Result<(), WrongCore> {
        Err(WrongCore)
    }
    let (route, slot, platform) = make_route();
    let route = ScopeRoute::new(
        route.into_platform(),
        slot,
        mask,
        acknowledge,
        wrong_core,
        schedule as Schedule,
        now,
    );
    let h = hardware(&platform);
    let stops = h.stops.clone();
    let irq = Interrupts::default();
    let detached = irq.detached.clone();
    let mut latch = Latch::new();
    let scope = new_scope(lease(), h, irq, route, &mut latch);
    let mut future =
        Box::pin(scope.run((), async |(), _| panic!("body cannot start on wrong core")));
    assert_eq!(poll(future.as_mut()), Poll::Ready(Err(WrongCore)));
    drop(future);
    assert_eq!(stops.get(), 1);
    assert_eq!(detached.get(), 1);
    assert!(platform.masked.load(Ordering::SeqCst));
    assert!(!slot.mac_interrupt().had_status);
    assert!(slot.power_interrupt().observation.is_empty());
    assert!(matches!(
        latch.into_return(),
        Some(ScopeReturn::Released(_))
    ));
}

#[derive(Clone, Copy, Debug)]
enum Actor {
    Driver,
    Mac,
    Power,
}
fn actor(slot: &'static ScopeSlot, which: Actor) {
    match which {
        Actor::Driver => dispatch(slot),
        Actor::Mac => {
            slot.mac_interrupt();
        }
        Actor::Power => {
            slot.power_interrupt();
        }
    }
}

// Regression for target/agent-a/reclamation-counterexample.rs: an inline
// pinned future freed the registration after timeout while an actor held it.
// Here dropping that same owning Pin cannot release the permanent allocation.
#[test]
fn timeout_quarantines_live_driver_and_irq_backing_without_calling_stop() {
    for which in [Actor::Driver, Actor::Mac, Actor::Power] {
        let (route, slot, platform) = make_route();
        let block = Block::new();
        let mut h = hardware(&platform);
        let (stops, drops) = (h.stops.clone(), h.drops.clone());
        let mut irq = Interrupts::default();
        let (irq_drops, detached) = (irq.drops.clone(), irq.detached.clone());
        let resumed = match which {
            Actor::Driver => {
                h.block = Some(block.clone());
                h.accesses_after_pause.clone()
            }
            _ => {
                irq.block = Some(block.clone());
                irq.accesses_after_pause.clone()
            }
        };
        let mut latch = Latch::new();
        let scope = new_scope(lease(), h, irq, route, &mut latch);
        let mut future = Box::pin(scope.run((), async |(), _| core::future::pending::<()>().await));
        assert!(poll(future.as_mut()).is_pending());
        let worker = thread::spawn(move || actor(slot, which));
        block.entered.wait();
        assert_eq!(slot.entered.load(Ordering::SeqCst), 1);
        drop(future);
        assert!(
            matches!(latch.into_return(), Some(ScopeReturn::Poisoned(_))),
            "{which:?}"
        );
        assert!(platform.masked.load(Ordering::SeqCst));
        assert!(!slot.open.load(Ordering::SeqCst));
        assert_eq!(stops.get(), 0, "timeout cannot project a borrowed Driver");
        assert_eq!(drops.get(), 0);
        assert_eq!(irq_drops.get(), 0);
        assert_eq!(detached.get(), 0);
        assert!(
            !slot.mac_interrupt().had_status,
            "closed admission rejects without taking the bank lock"
        );
        block.resume.wait();
        worker.join().unwrap();
        assert_eq!(
            resumed.get(),
            1,
            "the admitted actor still owns live permanent bytes"
        );
        assert_eq!(slot.entered.load(Ordering::SeqCst), 0);
        assert_eq!(stops.get(), 0, "eventual exit cannot undo permanent poison");
        assert_eq!(drops.get(), 0);
        assert_eq!(irq_drops.get(), 0);
    }
}

#[test]
fn unconfirmed_can_free_driver_container_without_drop_or_reclaiming_backing() {
    for which in [Actor::Driver, Actor::Mac, Actor::Power] {
        let (route, slot, platform) = make_route();
        let block = Block::new();
        let mut h = hardware(&platform);
        let (stops, drops) = (h.stops.clone(), h.drops.clone());
        let mut irq = Interrupts::default();
        let (irq_drops, detached) = (irq.drops.clone(), irq.detached.clone());
        let resumed = match which {
            Actor::Driver => {
                h.block = Some(block.clone());
                h.accesses_after_pause.clone()
            }
            _ => {
                irq.block = Some(block.clone());
                irq.accesses_after_pause.clone()
            }
        };
        let context = std::string::String::from("caller-owned context");
        let mut latch = Latch::new();
        let scope = new_scope(lease(), h, irq, route, &mut latch);
        // The same borrowed wait as Driver::drive; the core explicitly abandons.
        let mut future = Box::pin(async {
            scope.start().unwrap();
            assert_eq!(&context, "caller-owned context");
            core::future::pending::<()>().await;
        });
        assert!(poll(future.as_mut()).is_pending());
        let registered = slot.entry.load(Ordering::Acquire);
        let worker = thread::spawn(move || actor(slot, which));
        block.entered.wait();
        assert!(matches!(
            scope.lease.storage().abandon(scope.bound),
            Closed::TimedOut
        ));
        assert!(platform.masked.load(Ordering::Acquire));
        assert!(!slot.open.load(Ordering::Acquire));
        drop(future);
        drop(context);
        // Model core quarantine, then free the outer allocation without Drop.
        let quarantine = Box::new(Quarantined::new(driver::Driver::new(scope)));
        drop(quarantine);
        assert!(latch.into_return().is_none(), "Driver::drop must not run");
        assert_eq!(slot.entry.load(Ordering::Acquire), registered);
        assert_eq!(
            (stops.get(), drops.get(), irq_drops.get(), detached.get()),
            (0, 0, 0, 0)
        );
        assert!(!slot.mac_interrupt().had_status);
        block.resume.wait();
        worker.join().unwrap();
        assert_eq!(resumed.get(), 1, "the actor resumes in permanent backing");
        assert_eq!(slot.entered.load(Ordering::Acquire), 0);
        assert_eq!(
            (stops.get(), drops.get(), irq_drops.get(), detached.get()),
            (0, 0, 0, 0)
        );
    }
}

// Resume an actor at a chosen virtual instant, after any number of polls.
// Each hook owns the join handle until it either re-arms or joins it.
fn resume_at(platform: Platform, block: Arc<Block>, worker: thread::JoinHandle<()>, at: Instant) {
    WAIT_BOUNDARY.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            let time = platform.clock.lock().unwrap().now();
            if time < at {
                resume_at(platform, block, worker, at);
            } else {
                block.resume.wait();
                worker.join().unwrap();
            }
        }));
    });
}

#[test]
fn cancellation_drains_another_cores_bounded_step_independently_of_spin_speed() {
    // The same 100 us step/IRQ ends after different numbers of observations.
    // The old four-observation bound would poison these ordinary cancellations.
    for elapsed_per_read in [1, 7, 25] {
        for which in [Actor::Driver, Actor::Mac, Actor::Power] {
            let (route, slot, platform) = make_route();
            platform
                .sample_elapsed_us
                .store(elapsed_per_read, Ordering::Relaxed);
            let block = Block::new();
            let mut h = hardware(&platform);
            let stops = h.stops.clone();
            let mut irq = Interrupts::default();
            let detached = irq.detached.clone();
            match which {
                Actor::Driver => h.block = Some(block.clone()),
                _ => irq.block = Some(block.clone()),
            }
            let mut latch = Latch::new();
            let scope = new_scope(lease(), h, irq, route, &mut latch);
            let bound = scope.bound.duration();
            let mut future =
                Box::pin(scope.run((), async |(), _| core::future::pending::<()>().await));
            assert!(poll(future.as_mut()).is_pending());
            let worker = thread::spawn(move || actor(slot, which));
            block.entered.wait();
            resume_at(
                platform.clone(),
                block,
                worker,
                Instant::from_micros(if matches!(which, Actor::Driver) {
                    100
                } else {
                    20
                }),
            );
            drop(future);
            assert!(platform.clock.lock().unwrap().now().as_micros() <= bound.as_micros());
            assert_eq!(stops.get(), 1);
            assert_eq!(detached.get(), 1);
            assert!(slot.entry.load(Ordering::Acquire).is_null());
            assert!(matches!(
                latch.into_return(),
                Some(ScopeReturn::Released(_))
            ));
        }
    }
}

#[test]
fn lifecycle_is_monotonic_and_abandonment_cannot_be_overwritten_by_completion() {
    let state = DriverLifecycle::new();
    assert_eq!(state.snapshot(), DriverStatus::Running);
    state.retired();
    assert_eq!(
        state.snapshot(),
        DriverStatus::Running,
        "completion requires a retirement request"
    );
    state.request_retire();
    assert_eq!(state.snapshot(), DriverStatus::RetireRequested);
    state.retired();
    assert_eq!(state.snapshot(), DriverStatus::Retired);
    state.request_retire();
    assert_eq!(state.snapshot(), DriverStatus::Retired);
    // Both completion/abandon orders, plus abandonment directly from Running.
    for before in 0..3 {
        let state = Arc::new(DriverLifecycle::new());
        if before >= 1 {
            state.request_retire();
        }
        if before >= 2 {
            state.retired();
        }
        state.abandon();
        let other_core = state.clone();
        thread::spawn(move || {
            other_core.request_retire();
            other_core.retired();
        })
        .join()
        .unwrap();
        assert_eq!(state.snapshot(), DriverStatus::Abandoned);
    }
}

#[test]
fn irq_can_preempt_or_overlap_a_mutable_driver_step_on_either_core() {
    // Both same-core orders: hard IRQ preempts a live Driver borrow, or a
    // higher driver-SWI preempts an IRQ-bank borrow. Neither projects the other.
    for mac_first in [false, true] {
        let (route, slot, platform) = make_route();
        let mut h = hardware(&platform);
        let mut irq = Interrupts::default();
        if mac_first {
            irq.driver_during_irq = Some(slot);
        } else {
            h.irq_during_step = Some(slot);
        }
        let mut latch = Latch::new();
        let scope = new_scope(lease(), h, irq, route, &mut latch);
        let mut future = Box::pin(scope.run((), async |(), _| core::future::pending::<()>().await));
        assert!(poll(future.as_mut()).is_pending());
        if mac_first {
            assert!(slot.mac_interrupt().had_status);
        } else {
            dispatch(slot);
        }
        drop(future);
        assert!(matches!(
            latch.into_return(),
            Some(ScopeReturn::Released(_))
        ));
    }

    // Either priority on another core: the other actor remains entered.
    for driver_first in [false, true] {
        let (route, slot, platform) = make_route();
        let block = Block::new();
        let mut h = hardware(&platform);
        let mut irq = Interrupts::default();
        if driver_first {
            h.block = Some(block.clone());
        } else {
            irq.block = Some(block.clone());
        }
        let mut latch = Latch::new();
        let scope = new_scope(lease(), h, irq, route, &mut latch);
        let mut future = Box::pin(scope.run((), async |(), _| core::future::pending::<()>().await));
        assert!(poll(future.as_mut()).is_pending());
        let worker = thread::spawn(move || {
            actor(
                slot,
                if driver_first {
                    Actor::Driver
                } else {
                    Actor::Mac
                },
            )
        });
        block.entered.wait();
        if driver_first {
            assert!(slot.mac_interrupt().had_status);
        } else {
            dispatch(slot);
        }
        block.resume.wait();
        worker.join().unwrap();
        drop(future);
        assert!(matches!(
            latch.into_return(),
            Some(ScopeReturn::Released(_))
        ));
    }
}

#[test]
fn a_released_lease_can_be_installed_again_with_clean_facts() {
    let (route, slot, platform) = make_route();
    let mut latch = Latch::new();
    let scope = new_scope(
        lease(),
        hardware(&platform),
        Interrupts::default(),
        route,
        &mut latch,
    );
    let mut future = Box::pin(scope.run((), async |(), access| access.retire(()).await));
    assert!(poll(future.as_mut()).is_pending());
    assert!(slot.mac_interrupt().had_status);
    dispatch(slot);
    assert!(slot.mac_interrupt().had_status);
    assert_eq!(poll(future.as_mut()), Poll::Ready(Ok(())));
    drop(future);
    let first = released(latch.into_return());
    let DetachedOwners::Stopped(owners) = first.owners else {
        panic!("normal return");
    };
    let owners = owners.into_owners();
    let h = hardware(&platform);
    let facts = h.facts.clone();
    drop(owners.hardware);
    let mut latch = Latch::new();
    let scope = new_scope(
        first.lease,
        h,
        Interrupts::default(),
        owners.route,
        &mut latch,
    );
    let mut future = Box::pin(scope.run((), async |(), access| access.retire(()).await));
    assert!(poll(future.as_mut()).is_pending());
    dispatch(slot);
    assert_eq!(
        facts.get(),
        0,
        "previous installation facts cannot cross lease reuse"
    );
    assert_eq!(poll(future.as_mut()), Poll::Ready(Ok(())));
    drop(future);
    assert!(matches!(
        latch.into_return(),
        Some(ScopeReturn::Released(_))
    ));
}

struct Wakes {
    count: Count,
    slot: &'static ScopeSlot,
}
impl Wake for Wakes {
    fn wake(self: Arc<Self>) {
        assert_eq!(
            self.slot.entered.load(Ordering::SeqCst),
            0,
            "all installation borrows end before wake"
        );
        self.count.increment(); // Composition wakers only schedule, never detach.
    }
}
#[test]
fn both_irq_wake_tails_own_values_and_run_after_admission_guard_exits() {
    for mac in [false, true] {
        let (route, slot, platform) = make_route();
        let mut latch = Latch::new();
        let scope = new_scope(
            lease(),
            hardware(&platform),
            Interrupts::default(),
            route,
            &mut latch,
        );
        let mut future = Box::pin(scope.run((), async |(), _| core::future::pending::<()>().await));
        assert!(poll(future.as_mut()).is_pending());
        let count = Count::default();
        let waker = Waker::from(Arc::new(Wakes {
            count: count.clone(),
            slot,
        }));
        #[allow(
            unsafe_code,
            reason = "the test exclusively registers a waker before starting any actor"
        )]
        // SAFETY: this test binds exactly these types and has no concurrent
        // driver/IRQ actors; the exclusive test projection ends before entry.
        let driver = unsafe {
            &mut *(*slot.entry.load(Ordering::Acquire))
                .driver_context
                .cast::<DriverRegion<Hardware, Interrupts, Platform, Infallible, Schedule>>()
        };
        if mac {
            assert!(
                driver
                    .mac
                    .poll_batch(&mut Context::from_waker(&waker))
                    .is_pending()
            );
        } else {
            assert!(
                driver
                    .power
                    .poll(&mut Context::from_waker(&waker))
                    .is_pending()
            );
        }
        if mac {
            assert!(slot.mac_interrupt().had_status);
        } else {
            assert!(slot.power_interrupt().observation.sta_tbtt());
        }
        assert_eq!(count.get(), 1);
        drop(future);
        assert!(matches!(
            latch.into_return(),
            Some(ScopeReturn::Released(_))
        ));
    }
}

#[test]
fn due_deadlines_are_ready_and_future_deadlines_name_sources() {
    let now = Instant::from_micros(100);
    assert_eq!(
        WakePlan::until(now, now, WakeSources::Commands),
        WakePlan::Ready
    );
    assert_eq!(
        WakePlan::until(now, Instant::from_micros(99), WakeSources::Interrupts),
        WakePlan::Ready
    );
    let WakePlan::Wait { sources, deadline } = WakePlan::until(
        now,
        Instant::from_micros(101),
        WakeSources::CommandsAndInterrupts,
    ) else {
        panic!("future deadline");
    };
    assert_eq!(sources, WakeSources::CommandsAndInterrupts);
    assert_eq!(deadline.at(), Instant::from_micros(101));
}

#[test]
fn software_capacity_publication_wakes_the_registered_driver() {
    let (route, slot, platform) = make_route();
    let h = hardware(&platform);
    let capacity = h.capacity.clone();
    let publish = Arc::new(AtomicBool::new(false));
    let mut latch = Latch::new();
    let scope = new_scope(lease(), h, Interrupts::default(), route, &mut latch);
    let mut future = Box::pin(scope.run(publish.clone(), async |publish, access| {
        poll_fn(|_| {
            if publish.load(Ordering::Acquire) {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
        .await;
        access.irq().notify_rx_capacity();
        core::future::pending::<()>().await;
    }));
    assert!(poll(future.as_mut()).is_pending());
    dispatch(slot); // Registers the owned static-slot waker before checking facts.
    let before = platform.pends.get();
    publish.store(true, Ordering::Release);
    assert!(poll(future.as_mut()).is_pending());
    assert!(platform.pends.get() > before);
    assert!(platform.last_ready.load(Ordering::Acquire));
    dispatch(slot);
    assert_eq!(capacity.get(), 1);
    drop(future);
}

#[test]
fn irq_while_arming_wait_leaves_ready_work_for_a_finite_next_step() {
    let (route, slot, platform) = make_route();
    let h = hardware(&platform);
    let facts = h.facts.clone();
    let mut latch = Latch::new();
    let scope = new_scope(lease(), h, Interrupts::default(), route, &mut latch);
    let mut future = Box::pin(scope.run((), async |(), _| core::future::pending::<()>().await));
    assert!(poll(future.as_mut()).is_pending());
    WAIT_ARM.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            assert!(slot.mac_interrupt().had_status);
        }))
    });
    dispatch(slot);
    assert_eq!(facts.get(), 0, "new facts cannot extend the captured batch");
    assert!(platform.last_ready.load(Ordering::Acquire));
    dispatch(slot);
    assert_eq!(facts.get(), 1);
    drop(future);
}

#[test]
fn late_owned_driver_waker_does_not_touch_detached_installation_memory() {
    let (route, slot, platform) = make_route();
    let mut latch = Latch::new();
    let scope = new_scope(
        lease(),
        hardware(&platform),
        Interrupts::default(),
        route,
        &mut latch,
    );
    let mut future = Box::pin(scope.run((), async |(), _| core::future::pending::<()>().await));
    assert!(poll(future.as_mut()).is_pending());
    let waker = slot.driver_waker();
    drop(future);
    let before = platform.pends.get();
    let cloned = waker.clone();
    drop(waker);
    cloned.wake_by_ref();
    cloned.wake();
    assert_eq!(platform.pends.get(), before);
    assert_eq!(slot.entered.load(Ordering::Acquire), 0);
}
