//! Execute the actual shared lifecycle with the S31 Driver as B::Driver.
//! The port-facing mock has no radio traffic; hardware dispatch and teardown
//! are the same production registration primitive as the placement fixture.
mod cancellation;

use super::*;
use crate::scope::driver::{Driver, State};
use core::future::pending;
use core::ops::ControlFlow;
use oer_radio_port::{
    Abandon, Call, ClockError, Detach, Detached, Disabled, DriverExit, DriverLoan, EnableResult,
    Ended, EventEnd, EventsLost, InstallParts, Installed, Mint, Owner, Poisoned, PortBackend,
    PortBody, PortEnd, PortPhase, PortResult, Quarantine, Retire, Retired, Slot, Slots, run_port,
};
use oer_time::RadioInstant;

type S31Driver<'a> = Driver<'a, Hardware, Interrupts, Platform, Infallible, Schedule>;
type S31State = State<Hardware, Interrupts, Platform, Infallible, Schedule>;
enum HeldKind {}
struct Held<'id, 'control, 'context> {
    _place: Slot<'id, HeldKind>,
    storage: &'control Storage,
    context: &'context str,
    cancellation: Option<cancellation::Cut>,
}
struct Backend<'a, 'context> {
    storage: &'a Storage,
    bound: DrainBound,
    context: &'context str,
    detached: Count,
    deadline: Arc<AtomicBool>,
    cancellation: Option<cancellation::Cut>,
}
struct Phase;
impl PortPhase for Phase {
    type Event = Infallible;
    type Fault = Infallible;
    async fn next_event(&mut self) -> Result<Result<Infallible, EventsLost>, EventEnd<Infallible>> {
        pending().await
    }
}
impl<'a, 'context> PortBackend for Backend<'a, 'context> {
    type Pools = Slots<HeldKind, 1>;
    type Event<'id> = Infallible;
    type Terminal<'id> = Infallible;
    type Domain = ();
    type Fault = Infallible;
    type Refusal = Infallible;
    type Setup<'p, 'id>
        = ()
    where
        Self: 'p,
        'id: 'p;
    type Phase<'p, 'id>
        = Phase
    where
        Self: 'p,
        'id: 'p;
    type Resources<'id> = ();
    type Held<'id> = Held<'id, 'a, 'context>;
    type Local<'id> = ();
    // No &mut Backend, installation brand or context lifetime in Driver.
    type Driver = S31Driver<'a>;
    type State = ();
    type Uninstall = ();
    type Uninstalled = S31State;

    async fn enable<'p, 'id>(
        &'p mut self,
        _: Call,
        _: &'p Self::Held<'id>,
        _: &'p mut (),
    ) -> EnableResult<Infallible, Infallible>
    where
        'id: 'p,
    {
        Ok(Ok(()))
    }
    async fn disable<'p, 'id>(
        &'p mut self,
        _: Call,
        _: &'p Self::Held<'id>,
        _: &'p mut (),
        _: &'p mut dyn FnMut(Infallible) -> ControlFlow<Infallible>,
    ) -> Result<(), Poisoned<Infallible>>
    where
        'id: 'p,
    {
        Ok(())
    }
    async fn now(&self, _: Call) -> PortResult<RadioInstant<()>, ClockError, Infallible> {
        Ok(Ok(RadioInstant::from_micros(1)))
    }
    fn resources<'id>(&mut self, mint: Mint<'id, Self::Pools>) -> InstallParts<'id, Self> {
        let [place] = mint.into_pools();
        InstallParts {
            held: Held {
                _place: place,
                storage: self.storage,
                context: self.context,
                cancellation: self.cancellation.clone(),
            },
            local: (),
            resources: (),
        }
    }
    fn setup<'p, 'id>(
        &'p mut self,
        _: Call,
        _: &'p Self::Held<'id>,
        _: &'p mut Self::Local<'id>,
    ) -> Self::Setup<'p, 'id>
    where
        'id: 'p,
    {
    }
    fn phase<'p, 'id>(&'p mut self, _: Call, _: &'p Self::Held<'id>, _: &'p mut ()) -> Phase
    where
        'id: 'p,
    {
        Phase
    }
    async fn drive<'p, 'id>(
        driver: DriverLoan<'p, Self::Driver>,
        held: &'p Self::Held<'id>,
        retire: Retire<'p>,
    ) -> DriverExit<Self::State, Infallible> {
        assert_eq!(held.context, "non-static context");
        if let Some(cut) = &held.cancellation {
            {
                // Use the real Retire minted by run_port and the production
                // Driver future. The wrapper chooses the cancellation cut.
                let mut waiting = Box::pin(driver.drive(&retire));
                poll_fn(|cx| {
                    if cut.cancel.load(Ordering::Acquire) {
                        return Poll::Ready(());
                    }
                    assert!(waiting.as_mut().poll(cx).is_pending());
                    cut.polled(held.storage);
                    Poll::Pending
                })
                .await;
                drop(waiting);
            }
            cut.cancelled(held.storage);
        } else {
            match driver.drive(&retire).await {
                Ok(()) => (),
                Err(never) => match never {},
            }
        }
        // Only this caller-side future accesses Held. Every hardware callback
        // already finished or reaches exclusively permanent quarantined memory.
        assert_eq!(held.context, "non-static context");
        let proof = retire.requested(driver).await;
        DriverExit::Retired { state: (), proof }
    }
    fn retire_bound(&self, _: Call) -> impl Future<Output = ()> + use<'a, 'context> {
        let deadline = self.deadline.clone();
        poll_fn(move |_| {
            if deadline.load(Ordering::Acquire) {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
    }
    fn detach<'id>(&mut self, _call: Detach<'_>, _: &Self::Held<'id>) {
        self.storage.detach_entry();
        self.detached.increment();
    }
    fn uninstall<'id>(
        self,
        held: Self::Held<'id>,
        _: (),
        retired: Retired<'id, (Self::Driver, ())>,
        _: Detached<'id>,
        _: (),
    ) -> S31State {
        assert_eq!(self.detached.get(), 1);
        let (driver, ()) = retired.into_state();
        let state = driver.finish();
        if let Some(cut) = &held.cancellation {
            cut.finished(&state);
        }
        state
    }
    fn abandon(&mut self, call: Quarantine) -> Abandon {
        // Abandon::Stopped only when stop proved DMA quiescent.
        match self.storage.abandon(self.bound) {
            Closed::Stopped(StopOutcome::Quiescent(_)) => Abandon::Stopped(call.stopped()),
            Closed::Stopped(StopOutcome::Unconfirmed) | Closed::TimedOut => {
                Abandon::Unconfirmed(call)
            }
        }
    }
}

struct Body {
    end: Arc<AtomicBool>,
    fail: Arc<AtomicBool>,
    output: Count,
}
impl<'a, 'context> PortBody<Backend<'a, 'context>> for Body {
    type Output = usize;
    async fn run<'p, 'id>(
        self,
        owner: Owner<'p, 'id, Backend<'a, 'context>, Disabled>,
        _: (),
    ) -> Ended<'id, Backend<'a, 'context>, usize> {
        poll_fn(|_| {
            if self.fail.load(Ordering::Acquire) || self.end.load(Ordering::Acquire) {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
        .await;
        if self.fail.load(Ordering::Acquire) {
            let proof = owner.fail_stop();
            poll_fn(|_| {
                if self.end.load(Ordering::Acquire) {
                    Poll::Ready(())
                } else {
                    Poll::Pending
                }
            })
            .await;
            self.output.increment();
            Ended::fail_stopped(proof, 7)
        } else {
            self.output.increment();
            Ended::uninstall(owner, (), 7)
        }
    }
}
#[derive(Default)]
struct Caller {
    end: Arc<AtomicBool>,
    fail: Arc<AtomicBool>,
    output: Count,
    detached: Count,
    deadline: Arc<AtomicBool>,
    cancellation: Option<cancellation::Cut>,
}
fn installed<'a, 'context>(
    scope: DriverScope<'a, Hardware, Interrupts, Platform, Infallible>,
    caller: &Caller,
    context: &'context str,
) -> (Installed<Backend<'a, 'context>>, Body) {
    // This test binding mirrors a private S31 factory: no control or Driver
    // field escapes Installed, and uninstall never returns the control. The
    // Driver owns the affine lease; its State cannot release quarantine.
    let pointer = core::ptr::from_ref(scope.lease.storage());
    #[allow(
        unsafe_code,
        reason = "the private factory pairs control with its one durable Driver"
    )]
    // SAFETY: the permanent allocation survives cancellation/forget. The
    // backend never escapes run_port; ordinary uninstall consumes the control
    // before a Released lease can be reused. The same caller owns shutdown.
    let storage = unsafe { &*pointer };
    let backend = Backend {
        storage,
        bound: scope.bound,
        context,
        detached: caller.detached.clone(),
        deadline: caller.deadline.clone(),
        cancellation: caller.cancellation.clone(),
    };
    let body = Body {
        end: caller.end.clone(),
        fail: caller.fail.clone(),
        output: caller.output.clone(),
    };
    (Installed::new(backend, Driver::new(scope)), body)
}

#[test]
fn loan_retires_then_core_detaches_and_consumes_driver_into_released() {
    let (route, slot, platform) = make_route();
    let h = hardware(&platform);
    let stops = h.stops.clone();
    let caller = Caller::default();
    let context = std::string::String::from("non-static context");
    let mut latch = Latch::new();
    let scope = new_scope(lease(), h, Interrupts::default(), route, &mut latch);
    let (installed, body) = installed(scope, &caller, &context);
    let mut future = Box::pin(run_port(installed, body));
    assert!(poll(future.as_mut()).is_pending());
    dispatch(slot);
    caller.end.store(true, Ordering::Release);
    assert!(poll(future.as_mut()).is_pending());
    dispatch(slot);
    let Poll::Ready(PortEnd::Uninstalled {
        uninstalled: State::Released(released),
        output: 7,
    }) = poll(future.as_mut())
    else {
        panic!("explicit released state");
    };
    assert_eq!(stops.get(), 1);
    assert_eq!(caller.detached.get(), 1);
    assert!(matches!(released.owners, DetachedOwners::Stopped(_)));
    drop(future);
    assert!(
        latch.into_return().is_none(),
        "normal exit returns State, not Drop output"
    );
}

#[test]
fn fail_stop_before_drive_does_not_reopen_admission_and_quarantines_explicitly() {
    let (route, slot, platform) = make_route();
    let h = hardware(&platform);
    let stops = h.stops.clone();
    let drops = h.drops.clone();
    let caller = Caller::default();
    caller.fail.store(true, Ordering::Release);
    let mut latch = Latch::new();
    let context = std::string::String::from("non-static context");
    let scope = new_scope(lease(), h, Interrupts::default(), route, &mut latch);
    let (installed, body) = installed(scope, &caller, &context);
    let mut future = Box::pin(run_port(installed, body));
    assert!(poll(future.as_mut()).is_pending());
    assert!(platform.masked.load(Ordering::Acquire));
    assert!(!slot.open.load(Ordering::Acquire));
    assert_eq!(stops.get(), 1);
    assert_eq!(caller.detached.get(), 0, "body still owns its result loan");
    caller.end.store(true, Ordering::Release);
    assert!(matches!(
        poll(future.as_mut()),
        Poll::Ready(PortEnd::FailStopped {
            output: 7,
            outcome: Abandon::Stopped(_),
            ..
        })
    ));
    assert_eq!(
        drops.get(),
        0,
        "discarding DriverExit retains quarantined state"
    );
    drop(future);
    assert!(
        latch.into_return().is_none(),
        "drive explicitly returned quarantine"
    );
}

#[test]
fn cancelling_run_port_during_isr_masks_before_core_quarantines_driver() {
    let (route, slot, platform) = make_route();
    let h = hardware(&platform);
    let stops = h.stops.clone();
    let drops = h.drops.clone();
    let block = Block::new();
    let mut irq = Interrupts::default();
    irq.block = Some(block.clone());
    let resumed = irq.accesses_after_pause.clone();
    let caller = Caller::default();
    let context = std::string::String::from("non-static context");
    let mut latch = Latch::new();
    let scope = new_scope(lease(), h, irq, route, &mut latch);
    let (installed, body) = installed(scope, &caller, &context);
    let mut future = Box::pin(run_port(installed, body));
    assert!(poll(future.as_mut()).is_pending());
    let actor = thread::spawn(move || slot.mac_interrupt());
    block.entered.wait();
    drop(future);
    assert!(platform.masked.load(Ordering::Acquire));
    assert!(!slot.open.load(Ordering::Acquire));
    assert_eq!(stops.get(), 0);
    assert_eq!(drops.get(), 0);
    assert!(
        latch.into_return().is_none(),
        "core quarantines Driver without Drop"
    );
    block.resume.wait();
    actor.join().unwrap();
    assert_eq!(
        resumed.get(),
        1,
        "the entered ISR still owns permanent backing"
    );
    assert_eq!(drops.get(), 0);
}

#[test]
fn fail_stop_with_entered_driver_returns_unconfirmed_state_without_stop_or_drop() {
    let (route, slot, platform) = make_route();
    let mut h = hardware(&platform);
    let block = Block::new();
    h.block = Some(block.clone());
    let stops = h.stops.clone();
    let drops = h.drops.clone();
    let resumed = h.accesses_after_pause.clone();
    let caller = Caller::default();
    let context = std::string::String::from("non-static context");
    let mut latch = Latch::new();
    let scope = new_scope(lease(), h, Interrupts::default(), route, &mut latch);
    let (installed, body) = installed(scope, &caller, &context);
    let mut future = Box::pin(run_port(installed, body));
    assert!(poll(future.as_mut()).is_pending());
    let actor = thread::spawn(move || dispatch(slot));
    block.entered.wait();
    caller.fail.store(true, Ordering::Release);
    assert!(poll(future.as_mut()).is_pending());
    assert_eq!(caller.detached.get(), 0);
    assert_eq!(stops.get(), 0);
    caller.end.store(true, Ordering::Release);
    assert!(matches!(
        poll(future.as_mut()),
        Poll::Ready(PortEnd::FailStopped {
            output: 7,
            outcome: Abandon::Unconfirmed(_),
            ..
        })
    ));
    drop(future);
    assert!(
        latch.into_return().is_none(),
        "Unconfirmed state returned explicitly"
    );
    assert_eq!(drops.get(), 0);
    block.resume.wait();
    actor.join().unwrap();
    assert_eq!(resumed.get(), 1);
    assert_eq!(drops.get(), 0);
}

#[test]
fn forgotten_run_port_never_registers_its_borrowed_context_or_output_latch() {
    let (route, slot, platform) = make_route();
    let h = hardware(&platform);
    let steps = h.steps.clone();
    let drops = h.drops.clone();
    let caller = Caller::default();
    let context = std::string::String::from("non-static context");
    let mut latch = Latch::new();
    let scope = new_scope(lease(), h, Interrupts::default(), route, &mut latch);
    let (installed, body) = installed(scope, &caller, &context);
    let mut future = Box::pin(run_port(installed, body));
    assert!(poll(future.as_mut()).is_pending());
    core::mem::forget(future);
    drop(context);
    assert!(latch.into_return().is_none());
    assert!(slot.mac_interrupt().had_status);
    dispatch(slot);
    assert_eq!(steps.get(), 1);
    assert_eq!(drops.get(), 0);
}

struct AbandonBody;
impl<'a, 'context> PortBody<Backend<'a, 'context>> for AbandonBody {
    type Output = ();
    async fn run<'p, 'id>(
        self,
        owner: Owner<'p, 'id, Backend<'a, 'context>, Disabled>,
        _: (),
    ) -> Ended<'id, Backend<'a, 'context>, ()> {
        drop(owner);
        pending().await
    }
}

#[test]
fn abandoned_port_returns_core_quarantine_without_dropping_driver() {
    let (route, _, platform) = make_route();
    let h = hardware(&platform);
    let (stops, drops) = (h.stops.clone(), h.drops.clone());
    let caller = Caller::default();
    let mut latch = Latch::new();
    let scope = new_scope(lease(), h, Interrupts::default(), route, &mut latch);
    let (installed, _) = installed(scope, &caller, "non-static context");
    let mut run = Box::pin(run_port(installed, AbandonBody));
    let Poll::Ready(PortEnd::Abandoned {
        outcome: Abandon::Stopped(_),
        quarantined,
    }) = poll(run.as_mut())
    else {
        panic!("stopped abandonment retains the core-owned driver");
    };
    drop(quarantined);
    drop(run);
    assert_eq!(stops.get(), 1);
    assert_eq!(drops.get(), 0);
    assert!(latch.into_return().is_none());
}

#[test]
fn common_retirement_deadline_cancels_a_live_step_into_permanent_quarantine() {
    let (route, slot, platform) = make_route();
    let mut h = hardware(&platform);
    let block = Block::new();
    h.block = Some(block.clone());
    let stops = h.stops.clone();
    let drops = h.drops.clone();
    let resumed = h.accesses_after_pause.clone();
    let caller = Caller::default();
    let context = std::string::String::from("non-static context");
    let mut latch = Latch::new();
    let scope = new_scope(lease(), h, Interrupts::default(), route, &mut latch);
    let (installed, body) = installed(scope, &caller, &context);
    let mut future = Box::pin(run_port(installed, body));
    assert!(poll(future.as_mut()).is_pending());
    let actor = thread::spawn(move || dispatch(slot));
    block.entered.wait();
    caller.end.store(true, Ordering::Release);
    caller.deadline.store(true, Ordering::Release);
    assert!(matches!(
        poll(future.as_mut()),
        Poll::Ready(PortEnd::FailStopped {
            output: 7,
            outcome: Abandon::Unconfirmed(_),
            ..
        })
    ));
    drop(future);
    assert!(platform.masked.load(Ordering::Acquire));
    assert!(!slot.open.load(Ordering::Acquire));
    assert_eq!((stops.get(), drops.get(), caller.detached.get()), (0, 0, 0));
    assert!(
        latch.into_return().is_none(),
        "core quarantines Driver without Drop"
    );
    block.resume.wait();
    actor.join().unwrap();
    assert_eq!(resumed.get(), 1);
    assert_eq!(drops.get(), 0);
}
