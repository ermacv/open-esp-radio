//! Cancel the actual Driver future at each await, using run_port's Retire.
use super::*;

#[derive(Clone, Copy, Debug)]
enum AwaitPoint {
    CoreRequest,
    ExecutorRetirement,
}
#[derive(Clone, Copy, Debug)]
enum AfterCancel {
    Finish,
    Retire,
    AbandonRequested,
}

#[derive(Clone)]
pub(super) struct Cut {
    point: AwaitPoint,
    after: AfterCancel,
    /// Whether the fixture's stop reads its DMA as quiescent.
    quiescent: bool,
    slot: &'static ScopeSlot,
    pub cancel: Arc<AtomicBool>,
    polls: Count,
    finished: Count,
    work: work::Audit,
}
impl Cut {
    pub fn polled(&self, storage: &Storage) {
        self.polls.increment();
        assert_eq!(
            storage.irq().lifecycle.snapshot() == DriverStatus::RetireRequested,
            matches!(self.point, AwaitPoint::ExecutorRetirement),
            "the poll must reach the selected await"
        );
        assert_eq!(self.work.descriptors.get(), 2);
        assert_eq!(self.work.queued.get(), 2);
        assert_eq!(self.work.buffer_drops.get(), 0);
    }

    pub fn cancelled(&self, storage: &Storage) {
        assert_eq!(self.work.descriptors.get(), 2, "cancel keeps DMA ownership");
        assert_eq!(self.work.queued.get(), 2);
        assert_eq!(self.work.buffer_drops.get(), 0);
        let retired = matches!(self.after, AfterCancel::Retire) && self.polls.get() != 0;
        match self.after {
            AfterCancel::Retire if retired => {
                // Same retirement signal and bounded step as Driver::drive uses.
                // No owner is held by the cancelled future; the step accepts
                // the intact backing and accounts for every descriptor.
                storage.irq().lifecycle.request_retire();
                dispatch(self.slot);
                assert_eq!(storage.irq().lifecycle.snapshot(), DriverStatus::Retired);
                assert_eq!(self.work.descriptors.get(), 0);
            }
            AfterCancel::AbandonRequested => {
                // Leave the complete state abandoned before consuming finish.
                // The real backend's once-only physical abandon is exercised
                // by the fail-stop / cancel tests alongside this matrix.
                storage.irq().lifecycle.abandon();
            }
            // Before first poll there is no registration to retire. finish
            // handles the initialized, unstarted owner synchronously.
            _ => {}
        }
    }

    pub fn finished(&self, state: &S31State) {
        // The DMA fact decides, not the path: after a quiescent stop retire,
        // abandon and an unstarted finish all release; after an unconfirmed
        // one they all quarantine.
        match state {
            State::Released(released) => {
                assert!(self.quiescent, "only a quiescent stop releases");
                assert!(matches!(&released.owners, DetachedOwners::Stopped(_)));
            }
            State::Quarantined(_) => {
                assert!(!self.quiescent, "a quiescent stop releases on every path");
            }
        }
        assert!(!self.slot.open.load(Ordering::Acquire));
        assert!(self.slot.entry.load(Ordering::Acquire).is_null());
        assert_eq!(self.work.descriptors.get(), 0, "no live descriptor remains");
        assert_eq!(self.work.queued.get(), 0, "no queued DMA owner remains");
        assert_eq!(self.work.buffer_drops.get(), 2, "each buffer returned once");
        self.finished.increment();
    }
}

#[test]
fn cancellation_at_every_drive_await_preserves_retire_abandon_and_finish() {
    for (point, quiescent) in [AwaitPoint::CoreRequest, AwaitPoint::ExecutorRetirement]
        .into_iter()
        .flat_map(|point| [true, false].map(|q| (point, q)))
    {
        for polls in 0..=3 {
            for after in [
                AfterCancel::Finish,
                AfterCancel::Retire,
                AfterCancel::AbandonRequested,
            ] {
                let (route, slot, platform) = make_route();
                let mut hardware = hardware(&platform);
                hardware.dma_quiescent = quiescent;
                let (work, audit) = work::DmaQueue::populated();
                hardware.work = work;
                let stops = hardware.stops.clone();
                let drops = hardware.drops.clone();
                let cut = Cut {
                    point,
                    after,
                    quiescent,
                    slot,
                    cancel: Arc::default(),
                    polls: Count::default(),
                    finished: Count::default(),
                    work: audit,
                };
                let caller = Caller {
                    cancellation: Some(cut.clone()),
                    ..Caller::default()
                };
                caller.end.store(
                    matches!(point, AwaitPoint::ExecutorRetirement),
                    Ordering::Release,
                );
                let context = std::string::String::from("non-static context");
                let mut latch = Latch::new();
                let scope = new_scope(lease(), hardware, Interrupts::default(), route, &mut latch);
                let (installed, body) = installed(scope, &caller, &context);
                let mut run = Box::pin(run_port(installed, body));
                for _ in 0..polls {
                    assert!(poll(run.as_mut()).is_pending());
                }
                assert_eq!(cut.polls.get(), polls, "{point:?}, {after:?}");
                cut.cancel.store(true, Ordering::Release);
                caller.end.store(true, Ordering::Release);
                let Poll::Ready(PortEnd::Uninstalled {
                    uninstalled,
                    output: 7,
                }) = poll(run.as_mut())
                else {
                    panic!("the core hands retained Driver to consuming finish");
                };
                assert_eq!(cut.finished.get(), 1, "{point:?}, {after:?}, {polls}");
                assert_eq!(stops.get(), 1);
                assert!(platform.masked.load(Ordering::Acquire));
                drop(uninstalled);
                drop(run);
                assert_eq!(stops.get(), 1, "outer cancellation must not stop twice");
                // A quiescent stop returns the hardware on every path, so
                // dropping the uninstalled owners drops it exactly once; an
                // unconfirmed one keeps it quarantined, never dropped.
                assert_eq!(
                    drops.get(),
                    usize::from(quiescent),
                    "{point:?}, {after:?}, {polls}, {quiescent}"
                );
                assert!(latch.into_return().is_none(), "finish recovered explicitly");
            }
        }
    }
}

#[test]
fn whole_run_cancellation_at_every_drive_await_abandons_once() {
    // Both DMA facts: the core's abandon maps an unconfirmed stop to
    // Abandon::Unconfirmed (lifecycle.rs Backend::abandon); either way the
    // core quarantines Driver ().
    for (point, quiescent) in [AwaitPoint::CoreRequest, AwaitPoint::ExecutorRetirement]
        .into_iter()
        .flat_map(|point| [true, false].map(|q| (point, q)))
    {
        for polls in 0..=3 {
            let (route, slot, platform) = make_route();
            let mut hardware = hardware(&platform);
            hardware.dma_quiescent = quiescent;
            let (work, audit) = work::DmaQueue::populated();
            hardware.work = work;
            let stops = hardware.stops.clone();
            let drops = hardware.drops.clone();
            let cut = Cut {
                point,
                after: AfterCancel::Finish,
                quiescent,
                slot,
                cancel: Arc::default(),
                polls: Count::default(),
                finished: Count::default(),
                work: audit.clone(),
            };
            let caller = Caller {
                cancellation: Some(cut.clone()),
                ..Caller::default()
            };
            caller.end.store(
                matches!(point, AwaitPoint::ExecutorRetirement),
                Ordering::Release,
            );
            let context = std::string::String::from("non-static context");
            let mut latch = Latch::new();
            let scope = new_scope(lease(), hardware, Interrupts::default(), route, &mut latch);
            let (installed, body) = installed(scope, &caller, &context);
            let mut run = Box::pin(run_port(installed, body));
            for _ in 0..polls {
                assert!(poll(run.as_mut()).is_pending());
            }
            assert_eq!(cut.polls.get(), polls, "{point:?}");
            assert_eq!(audit.descriptors.get(), 2);
            assert_eq!(audit.queued.get(), 2);
            assert_eq!(audit.buffer_drops.get(), 0);
            // The core ends the driving loan, invokes physical abandon once,
            // and quarantines Driver. Before first poll, owned Driver's Drop
            // provides that same physical safety path without entering core.
            drop(run);
            assert_eq!(stops.get(), 1, "{point:?}, {polls}");
            assert_eq!(cut.finished.get(), 0, "core abandon never consumes finish");
            assert!(platform.masked.load(Ordering::Acquire));
            assert!(!slot.open.load(Ordering::Acquire));
            assert!(slot.entry.load(Ordering::Acquire).is_null());
            assert_eq!(audit.descriptors.get(), 0);
            assert_eq!(audit.queued.get(), 0);
            assert_eq!(audit.buffer_drops.get(), 2);
            assert_eq!(drops.get(), 0, "quarantine does not destroy Driver owners");
            assert!(!matches!(
                latch.into_return(),
                Some(ScopeReturn::Released(_))
            ));
            assert_eq!(stops.get(), 1, "discarding the receipt cannot stop twice");
        }
    }
}
