//! The real driver lifecycle under strict P2-before-P1 scheduling.
use super::*;
use oer_time::Duration;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::task::Wake;

const FRAMES: usize = 256;
const RETURN_BOUND: Duration = Duration::from_micros(250);

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
            for _ in 0..FRAMES {
                let frame = match phase.next_event().await {
                    Ok(Ok(LowerMacEvent::Received { frame, .. })) => frame,
                    _ => panic!("continuous RX frame"),
                };
                let now = self.clock.now();
                let gap = now.saturating_duration_since(last_return);
                assert!(
                    gap <= RETURN_BOUND,
                    "P1 must return RX within its model bound"
                );
                longest_gap = longest_gap.max(gap);
                last_return = now;
                phase.recycle(frame);
            }
            let mut observed = self.observed.borrow_mut();
            assert!(observed.received.is_empty());
            observed.continuous_rx = false;
        }
        let owner = owner
            .disable(&mut |_| core::ops::ControlFlow::Continue(()))
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
fn continuous_rx_yields_p2_and_returns_p1_buffers_within_a_virtual_time_bound() {
    let (parts, observed, clock) = installed(Vec::new());
    clock.1.set(true);
    observed.borrow_mut().check_context = true;
    observed.borrow_mut().context = 2;
    let released = super::Released::default();
    let p1_ready = Arc::new(Ready(AtomicBool::new(true)));
    let p2_ready = Arc::new(Ready(AtomicBool::new(true)));
    let p1_waker = Waker::from(p1_ready.clone());
    let p2_waker = Waker::from(p2_ready.clone());
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
                // P1 may only run after P2 has no ready work. A stale ready
                // timer must fail this finite device-step budget, not hang.
                observed.borrow_mut().context = 2;
                observed.borrow_mut().rx_poll_steps = 0;
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
                observed.borrow_mut().context = 1;
                if let Poll::Ready(end) = p1.as_mut().poll(&mut Context::from_waker(&p1_waker)) {
                    let (joined, output) = match end {
                        radio::PortEnd::Uninstalled { joined, output } => (joined, output),
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
        assert!(
            p1_done && p2_done,
            "both priorities finish in bounded steps"
        );
    }));
    assert_eq!(observed.borrow().rx_delivered, FRAMES);
    assert!(observed.borrow().rx_saturated_polls > 0);
    assert_eq!(super::count(&released), FRAMES);
}
