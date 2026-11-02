//! The real driver and body tasks on one executor, the driver's task polled
//! first when both are ready.
use super::*;
use oer_time::Duration;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::task::Wake;

const FRAMES: usize = 256;
const RETURN_BOUND: Duration = Duration::from_micros(250);
/// The units the body takes before it yields, as the station's turn does.
const TURN: usize = 32;

struct Ready(AtomicBool);
impl Wake for Ready {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.0.store(true, Ordering::Relaxed);
    }
}

struct ContinuousReceive {
    observed: Rc<RefCell<Observed>>,
    released: super::Released,
    clock: TestTimer,
    rx_interrupt: Waker,
}
impl<Id: radio::SlotIdentity> PortBody<Id, Backend<Id>> for ContinuousReceive {
    type Output = Duration;
    async fn run<'p, 'id>(
        self,
        owner: radio::Owner<'p, 'id, Id, Backend<Id>, radio::Disabled>,
        resources: Res<'id, Id>,
    ) -> radio::Ended<Id, Backend<Id>, Duration>
    where
        Backend<Id>: 'id,
        Id: 'id,
        'id: 'p,
    {
        let mut owner = owner.enable().await.ok().expect("model enable");
        let mut longest_gap = Duration::ZERO;
        {
            let mut phase = owner.phase();
            phase.station().configure(
                resources
                    .station_commands
                    .configure
                    .with_context(Tag(7, None)),
                StationReceive::Bss {
                    bssid: IndividualAddress::new(BSSID).expect("BSSID"),
                    filter: SupportedFilter::new(ReceiveFilter::BSS_MEMBER).expect("filter"),
                },
            );
            assert!(matches!(
                phase.next_event().await,
                Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::Configured(_))))
            ));
            {
                let mut observed = self.observed.borrow_mut();
                observed.continuous_rx = true;
                observed.received.extend((0..FRAMES).map(|_| TestReceive {
                    bytes: super::super::data_frame(STATION),
                    view: 0..26,
                    released: self.released.clone(),
                }));
            }
            self.rx_interrupt.wake_by_ref();
            let mut last_return = self.clock.now();
            for taken in 1..=FRAMES {
                let frame = match phase.next_event().await {
                    Ok(Ok(LowerMacEvent::Received { frame, .. })) => frame,
                    _ => panic!("continuous RX frame"),
                };
                let now = self.clock.now();
                let gap = now.saturating_duration_since(last_return);
                assert!(
                    gap <= RETURN_BOUND,
                    "the body must return RX within its model bound"
                );
                longest_gap = longest_gap.max(gap);
                last_return = now;
                phase.recycle(frame);
                if taken % TURN == 0 {
                    rx_capacity_yield().await;
                }
            }
            let mut observed = self.observed.borrow_mut();
            assert!(observed.received.is_empty());
            observed.continuous_rx = false;
        }
        let owner = owner
            .disable(None, &mut |_| core::ops::ControlFlow::Continue(()))
            .await
            .ok()
            .expect("model disable");
        assert_eq!(
            super::count(&self.released),
            FRAMES,
            "each RX owner recycled once"
        );
        radio::Ended::uninstall(owner, (), longest_gap)
    }
}

#[test]
fn a_burst_reaches_the_body_through_its_own_pulls_within_a_virtual_time_bound() {
    let (parts, observed, clock) = installed(Vec::new());
    clock.1.set(true);
    observed.borrow_mut().check_context = true;
    observed.borrow_mut().context = 2;
    let released = super::Released::default();
    let p1_ready = Arc::new(Ready(AtomicBool::new(true)));
    let p2_ready = Arc::new(Ready(AtomicBool::new(true)));
    let p1_waker = Waker::from(p1_ready.clone());
    let p2_waker = Waker::from(p2_ready.clone());
    let mut driver_polls = 0;
    let mut driver_took = 0;
    block_on(radio::with_local_slot(async |claim| {
        let mut slot = claim.place();
        let (front, registration) = slot
            .register(Backend::new(parts).ok().expect("model backend"))
            .ok()
            .expect("one installation");
        let mut p2 = Some(Box::pin(crate::port::Registration::new(registration).run()));
        let mut p1 = Box::pin(radio::run_port(
            front,
            ContinuousReceive {
                observed: observed.clone(),
                released: released.clone(),
                clock: clock.clone(),
                rx_interrupt: p2_waker.clone(),
            },
        ));
        let (mut p1_done, mut p2_done) = (false, false);
        for _ in 0..2000 {
            if !p2_done && p2_ready.0.swap(false, Ordering::Relaxed) {
                // A stale ready timer must fail this finite step budget, not
                // hang.
                observed.borrow_mut().context = 2;
                driver_polls += 1;
                let before = observed.borrow().rx_delivered;
                let polled = p2
                    .as_mut()
                    .expect("registered P2 task")
                    .as_mut()
                    .poll(&mut Context::from_waker(&p2_waker));
                driver_took += observed.borrow().rx_delivered - before;
                if let Poll::Ready(retired) = polled {
                    drop(p2.take());
                    retired.expect("clean driver retirement").unregistered();
                    p2_done = true;
                }
            } else if !p1_done && p1_ready.0.swap(false, Ordering::Relaxed) {
                observed.borrow_mut().context = 1;
                observed.borrow_mut().rx_poll_steps = 0;
                if let Poll::Ready(end) = p1.as_mut().poll(&mut Context::from_waker(&p1_waker)) {
                    let (joined, output) = match end {
                        radio::PortEnd::Uninstalled { joined, output, .. } => (joined, output),
                        _ => panic!(
                            "clean RX uninstall: delivered={}, released={}, time={:?}",
                            observed.borrow().rx_delivered,
                            super::count(&released),
                            clock.now()
                        ),
                    };
                    assert!(output > Duration::ZERO && output <= RETURN_BOUND);
                    drop(joined);
                    p1_done = true;
                }
            } else {
                assert!(
                    clock.0.advance_to_next_deadline().is_some(),
                    "a sleeping task has a virtual deadline"
                );
                p2_ready.0.store(true, Ordering::Relaxed);
                p1_ready.0.store(true, Ordering::Relaxed);
            }
            if p1_done && p2_done {
                break;
            }
        }
        assert!(p1_done && p2_done, "both tasks finish in bounded steps");
    }));
    assert_eq!(observed.borrow().rx_delivered, FRAMES);
    assert_eq!(driver_took, 0, "the driver's turn takes no unit");
    assert!(
        driver_polls < FRAMES / 4,
        "{driver_polls} driver turns for {FRAMES} units"
    );
    assert_eq!(super::count(&released), FRAMES);
}
/// The body keeps every received unit, as a reorder window with a hole
/// does, until it holds `RX_HELD`, then returns them all.
struct RetainingReceive {
    observed: Rc<RefCell<Observed>>,
    released: super::Released,
    rx_interrupt: Waker,
}
impl<Id: radio::SlotIdentity> PortBody<Id, Backend<Id>> for RetainingReceive {
    type Output = ();
    async fn run<'p, 'id>(
        self,
        owner: radio::Owner<'p, 'id, Id, Backend<Id>, radio::Disabled>,
        resources: Res<'id, Id>,
    ) -> radio::Ended<Id, Backend<Id>, ()>
    where
        Backend<Id>: 'id,
        Id: 'id,
        'id: 'p,
    {
        let mut owner = owner.enable().await.ok().expect("model enable");
        {
            let mut phase = owner.phase();
            phase.station().configure(
                resources
                    .station_commands
                    .configure
                    .with_context(Tag(7, None)),
                StationReceive::Bss {
                    bssid: IndividualAddress::new(BSSID).expect("BSSID"),
                    filter: SupportedFilter::new(ReceiveFilter::BSS_MEMBER).expect("filter"),
                },
            );
            assert!(matches!(
                phase.next_event().await,
                Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::Configured(_))))
            ));
            {
                let mut observed = self.observed.borrow_mut();
                observed.continuous_rx = true;
                observed.received.extend((0..=RX_HELD).map(|_| TestReceive {
                    bytes: super::super::data_frame(STATION),
                    view: 0..26,
                    released: self.released.clone(),
                }));
            }
            self.rx_interrupt.wake_by_ref();
            let mut kept = Vec::new();
            for _ in 0..RX_HELD {
                match phase.next_event().await {
                    Ok(Ok(LowerMacEvent::Received { frame, .. })) => kept.push(frame),
                    _ => panic!("a retained RX frame"),
                }
            }
            // The body holds its bound: the last unit stays in the ring.
            assert_eq!(self.observed.borrow().received.len(), 1);
            for frame in kept {
                phase.recycle(frame);
            }
            match phase.next_event().await {
                Ok(Ok(LowerMacEvent::Received { frame, .. })) => phase.recycle(frame),
                _ => panic!("the unit after the held ones"),
            }
            let mut observed = self.observed.borrow_mut();
            assert!(observed.received.is_empty());
            observed.continuous_rx = false;
        }
        let owner = owner
            .disable(None, &mut |_| core::ops::ControlFlow::Continue(()))
            .await
            .ok()
            .expect("model disable");
        radio::Ended::uninstall(owner, (), ())
    }
}

#[test]
fn the_body_holds_at_most_its_bound_and_the_rest_waits_in_the_ring() {
    let (parts, observed, clock) = installed(Vec::new());
    clock.1.set(true);
    observed.borrow_mut().check_context = true;
    observed.borrow_mut().context = 2;
    let released = super::Released::default();
    let p1_ready = Arc::new(Ready(AtomicBool::new(true)));
    let p2_ready = Arc::new(Ready(AtomicBool::new(true)));
    let p1_waker = Waker::from(p1_ready.clone());
    let p2_waker = Waker::from(p2_ready.clone());
    let mut driver_took = 0;
    block_on(radio::with_local_slot(async |claim| {
        let mut slot = claim.place();
        let (front, registration) = slot
            .register(Backend::new(parts).ok().expect("model backend"))
            .ok()
            .expect("one installation");
        let mut p2 = Some(Box::pin(crate::port::Registration::new(registration).run()));
        let mut p1 = Box::pin(radio::run_port(
            front,
            RetainingReceive {
                observed: observed.clone(),
                released: released.clone(),
                rx_interrupt: p2_waker.clone(),
            },
        ));
        let (mut p1_done, mut p2_done) = (false, false);
        for _ in 0..2000 {
            if !p2_done && p2_ready.0.swap(false, Ordering::Relaxed) {
                observed.borrow_mut().context = 2;
                let before = observed.borrow().rx_delivered;
                if let Poll::Ready(retired) = p2
                    .as_mut()
                    .expect("registered P2 task")
                    .as_mut()
                    .poll(&mut Context::from_waker(&p2_waker))
                {
                    drop(p2.take());
                    retired.expect("clean driver retirement").unregistered();
                    p2_done = true;
                }
                driver_took += observed.borrow().rx_delivered - before;
            } else if !p1_done && p1_ready.0.swap(false, Ordering::Relaxed) {
                observed.borrow_mut().context = 1;
                observed.borrow_mut().rx_poll_steps = 0;
                if let Poll::Ready(end) = p1.as_mut().poll(&mut Context::from_waker(&p1_waker)) {
                    let radio::PortEnd::Uninstalled { joined, .. } = end else {
                        panic!("clean RX uninstall");
                    };
                    drop(joined);
                    p1_done = true;
                }
            } else {
                assert!(
                    clock.0.advance_to_next_deadline().is_some(),
                    "a sleeping task has a virtual deadline"
                );
                p2_ready.0.store(true, Ordering::Relaxed);
                p1_ready.0.store(true, Ordering::Relaxed);
            }
            if p1_done && p2_done {
                break;
            }
        }
        assert!(p1_done && p2_done, "both tasks finish in bounded steps");
    }));
    assert_eq!(observed.borrow().rx_delivered, RX_HELD + 1);
    assert_eq!(super::count(&released), RX_HELD + 1);
    assert_eq!(driver_took, 0, "the driver's turn takes no unit");
}

/// The body lends every received unit to a network that returns it at
/// once, as a station delivering UDP does, and lets the driver's turn run
/// between each take and its lend.
struct LendingReceive {
    observed: Rc<RefCell<Observed>>,
    released: super::Released,
    rx_interrupt: Waker,
    taken: Rc<Cell<usize>>,
}
impl<Id: radio::SlotIdentity> PortBody<Id, Backend<Id>> for LendingReceive {
    type Output = ();
    async fn run<'p, 'id>(
        self,
        owner: radio::Owner<'p, 'id, Id, Backend<Id>, radio::Disabled>,
        resources: Res<'id, Id>,
    ) -> radio::Ended<Id, Backend<Id>, ()>
    where
        Backend<Id>: 'id,
        Id: 'id,
        'id: 'p,
    {
        let mut owner = owner.enable().await.ok().expect("model enable");
        {
            let mut phase = owner.phase();
            phase.station().configure(
                resources
                    .station_commands
                    .configure
                    .with_context(Tag(7, None)),
                StationReceive::Bss {
                    bssid: IndividualAddress::new(BSSID).expect("BSSID"),
                    filter: SupportedFilter::new(ReceiveFilter::BSS_MEMBER).expect("filter"),
                },
            );
            assert!(matches!(
                phase.next_event().await,
                Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::Configured(_))))
            ));
            self.observed
                .borrow_mut()
                .received
                .extend((0..LENT_FRAMES).map(|_| TestReceive {
                    bytes: super::super::data_frame(STATION),
                    view: 0..26,
                    released: self.released.clone(),
                }));
            self.rx_interrupt.wake_by_ref();
            for _ in 0..LENT_FRAMES {
                match phase.next_event().await {
                    Ok(Ok(LowerMacEvent::Received { frame, .. })) => {
                        // The driver's turn runs between the take and the
                        // lend.
                        yield_once().await;
                        let Ok(lent) = phase.lend(frame) else {
                            panic!("the network takes the unit");
                        };
                        drop(lent);
                    }
                    _ => panic!("a lent RX frame"),
                }
                self.taken.set(self.taken.get() + 1);
            }
        }
        let owner = owner
            .disable(None, &mut |_| core::ops::ControlFlow::Continue(()))
            .await
            .ok()
            .expect("model disable");
        radio::Ended::uninstall(owner, (), ())
    }
}

/// The body's turn ends: the executor runs whatever else is ready first.
async fn rx_capacity_yield() {
    yield_once().await;
}

/// Pending once, after waking its task: the executor runs whatever else
/// is ready first.
async fn yield_once() {
    let mut yielded = false;
    core::future::poll_fn(|cx| {
        if core::mem::replace(&mut yielded, true) {
            Poll::Ready(())
        } else {
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    })
    .await;
}

/// More units than the body holds at once.
const LENT_FRAMES: usize = 64;

#[test]
fn lent_units_beyond_the_bodys_bound_reach_it_through_wakes_alone() {
    let (parts, observed, clock) = installed(Vec::new());
    clock.1.set(true);
    let released = super::Released::default();
    let taken = Rc::new(Cell::new(0));
    let p1_ready = Arc::new(Ready(AtomicBool::new(true)));
    let p2_ready = Arc::new(Ready(AtomicBool::new(true)));
    let p1_waker = Waker::from(p1_ready.clone());
    let p2_waker = Waker::from(p2_ready.clone());
    // Timer dispatches while units still wait in the device: each one is a
    // wake the driver should have had and did not.
    let mut stalls = 0;
    block_on(radio::with_local_slot(async |claim| {
        let mut slot = claim.place();
        let (front, registration) = slot
            .register(Backend::new(parts).ok().expect("model backend"))
            .ok()
            .expect("one installation");
        let mut p2 = Some(Box::pin(crate::port::Registration::new(registration).run()));
        let mut p1 = Box::pin(radio::run_port(
            front,
            LendingReceive {
                observed: observed.clone(),
                released: released.clone(),
                rx_interrupt: p2_waker.clone(),
                taken: taken.clone(),
            },
        ));
        let (mut p1_done, mut p2_done) = (false, false);
        for _ in 0..4000 {
            if !p2_done && p2_ready.0.swap(false, Ordering::Relaxed) {
                if let Poll::Ready(retired) = p2
                    .as_mut()
                    .expect("registered P2 task")
                    .as_mut()
                    .poll(&mut Context::from_waker(&p2_waker))
                {
                    drop(p2.take());
                    retired.expect("clean driver retirement").unregistered();
                    p2_done = true;
                }
            } else if !p1_done && p1_ready.0.swap(false, Ordering::Relaxed) {
                if let Poll::Ready(end) = p1.as_mut().poll(&mut Context::from_waker(&p1_waker)) {
                    let radio::PortEnd::Uninstalled { joined, .. } = end else {
                        panic!("clean RX uninstall");
                    };
                    drop(joined);
                    p1_done = true;
                }
            } else {
                let waiting = !observed.borrow().received.is_empty();
                stalls += usize::from(waiting && taken.get() < LENT_FRAMES);
                assert!(
                    clock.0.advance_to_next_deadline().is_some(),
                    "a sleeping task has a virtual deadline"
                );
                p2_ready.0.store(true, Ordering::Relaxed);
                p1_ready.0.store(true, Ordering::Relaxed);
            }
            if p1_done && p2_done {
                break;
            }
        }
        assert!(p1_done && p2_done, "both tasks finish in bounded steps");
    }));
    assert_eq!(taken.get(), LENT_FRAMES);
    assert_eq!(super::count(&released), LENT_FRAMES);
    assert_eq!(stalls, 0, "units waited for a timer instead of a wake");
}

#[test]
fn the_network_leaves_the_pool_a_full_reorder_window_and_the_critical_reserve() {
    use crate::rx::transaction::DEFAULT_CRITICAL_RESERVE;
    for units in [RX_HELD + DEFAULT_CRITICAL_RESERVE + 1, 64, 96] {
        // P1 holds a window but its hole, the network all it may: the
        // hole's MPDU still finds a credit beyond the critical reserve.
        let outside = crate::port::lent_rx(units) + (RX_HELD - 1);
        assert!(units - outside > DEFAULT_CRITICAL_RESERVE, "{units} units");
    }
    assert_eq!(crate::port::lent_rx(RX_HELD), 0);
}

/// The body polls its next event many times with an empty ring, as the
/// station's sources do, then takes one unit the driver's turn announced.
struct IdlePolls {
    observed: Rc<RefCell<Observed>>,
    released: super::Released,
}
impl<Id: radio::SlotIdentity> PortBody<Id, Backend<Id>> for IdlePolls {
    type Output = (usize, usize);
    async fn run<'p, 'id>(
        self,
        owner: radio::Owner<'p, 'id, Id, Backend<Id>, radio::Disabled>,
        resources: Res<'id, Id>,
    ) -> radio::Ended<Id, Backend<Id>, (usize, usize)>
    where
        Backend<Id>: 'id,
        Id: 'id,
        'id: 'p,
    {
        let mut owner = owner.enable().await.ok().expect("model enable");
        let counts;
        {
            let mut phase = owner.phase();
            phase.station().configure(
                resources
                    .station_commands
                    .configure
                    .with_context(Tag(7, None)),
                StationReceive::Bss {
                    bssid: IndividualAddress::new(BSSID).expect("BSSID"),
                    filter: SupportedFilter::new(ReceiveFilter::BSS_MEMBER).expect("filter"),
                },
            );
            assert!(matches!(
                phase.next_event().await,
                Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::Configured(_))))
            ));
            let before = self.observed.borrow().pulls;
            core::future::poll_fn(|cx| {
                for _ in 0..64 {
                    assert!(core::pin::pin!(phase.next_event()).poll(cx).is_pending());
                }
                Poll::Ready(())
            })
            .await;
            let idle = self.observed.borrow().pulls - before;
            self.observed.borrow_mut().received.push_back(TestReceive {
                bytes: super::super::data_frame(STATION),
                view: 0..26,
                released: self.released.clone(),
            });
            // The driver's turn sees the staged unit and wakes the body.
            match phase.next_event().await {
                Ok(Ok(LowerMacEvent::Received { frame, .. })) => phase.recycle(frame),
                _ => panic!("the announced unit"),
            }
            counts = (idle, self.observed.borrow().pulls - before);
        }
        let owner = owner
            .disable(None, &mut |_| core::ops::ControlFlow::Continue(()))
            .await
            .ok()
            .expect("model disable");
        radio::Ended::uninstall(owner, (), counts)
    }
}

#[test]
fn polls_without_ring_work_leave_the_device_alone() {
    let (parts, observed, _clock) = installed(Vec::new());
    let released = super::Released::default();
    let report = block_on(super::run_body(
        parts,
        IdlePolls {
            observed: observed.clone(),
            released: released.clone(),
        },
    ));
    let super::Report::Uninstalled {
        output: (idle, total),
    } = report
    else {
        panic!("clean uninstall");
    };
    assert_eq!(idle, 0, "an idle poll reached the device");
    assert!(total >= 1, "the announced unit was pulled");
    assert_eq!(super::count(&released), 1);
}

/// The body lends its one unit, polls once more with an empty ring, then the
/// network returns the unit: the body's next poll, with no driver turn
/// between, rearms the ring. A walker that ran out of armed descriptors
/// raises no receive interrupt, so the return is the only edge left.
struct ReturnRearms {
    observed: Rc<RefCell<Observed>>,
    released: super::Released,
}
impl<Id: radio::SlotIdentity> PortBody<Id, Backend<Id>> for ReturnRearms {
    type Output = (usize, usize);
    async fn run<'p, 'id>(
        self,
        owner: radio::Owner<'p, 'id, Id, Backend<Id>, radio::Disabled>,
        resources: Res<'id, Id>,
    ) -> radio::Ended<Id, Backend<Id>, (usize, usize)>
    where
        Backend<Id>: 'id,
        Id: 'id,
        'id: 'p,
    {
        let mut owner = owner.enable().await.ok().expect("model enable");
        let counts;
        {
            let mut phase = owner.phase();
            phase.station().configure(
                resources
                    .station_commands
                    .configure
                    .with_context(Tag(7, None)),
                StationReceive::Bss {
                    bssid: IndividualAddress::new(BSSID).expect("BSSID"),
                    filter: SupportedFilter::new(ReceiveFilter::BSS_MEMBER).expect("filter"),
                },
            );
            assert!(matches!(
                phase.next_event().await,
                Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::Configured(_))))
            ));
            self.observed.borrow_mut().received.push_back(TestReceive {
                bytes: super::super::data_frame(STATION),
                view: 0..26,
                released: self.released.clone(),
            });
            let lent = match phase.next_event().await {
                Ok(Ok(LowerMacEvent::Received { frame, .. })) => match phase.lend(frame) {
                    Ok(lent) => lent,
                    Err(_) => panic!("the network takes the unit"),
                },
                _ => panic!("the received unit"),
            };
            // An idle poll: the ring has no work, the pull stays closed.
            core::future::poll_fn(|cx| {
                assert!(core::pin::pin!(phase.next_event()).poll(cx).is_pending());
                Poll::Ready(())
            })
            .await;
            let before = self.observed.borrow().returned;
            drop(lent);
            core::future::poll_fn(|cx| {
                let _ = core::pin::pin!(phase.next_event()).poll(cx);
                Poll::Ready(())
            })
            .await;
            counts = (before, self.observed.borrow().returned);
        }
        let owner = owner
            .disable(None, &mut |_| core::ops::ControlFlow::Continue(()))
            .await
            .ok()
            .expect("model disable");
        radio::Ended::uninstall(owner, (), counts)
    }
}

#[test]
fn a_network_return_rearms_the_ring_in_the_bodys_own_poll() {
    let (parts, observed, _clock) = installed(Vec::new());
    let released = super::Released::default();
    let report = block_on(super::run_body(
        parts,
        ReturnRearms {
            observed: observed.clone(),
            released: released.clone(),
        },
    ));
    let super::Report::Uninstalled {
        output: (before, after),
    } = report
    else {
        panic!("clean uninstall");
    };
    assert_eq!(
        after,
        before + 1,
        "the body's poll rearmed the returned unit"
    );
    assert_eq!(super::count(&released), 1);
}
