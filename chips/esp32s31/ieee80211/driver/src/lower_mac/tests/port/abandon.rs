//! The real S31 port adapter with model hardware and owner-driven virtual time.
use super::*;
use core::pin::Pin;

fn poll_p1<F: Future>(future: Pin<&mut F>, observed: &RefCell<Observed>) -> Poll<F::Output> {
    observed.borrow_mut().context = 1;
    future.poll(&mut Context::from_waker(Waker::noop()))
}
fn poll_p2<F: Future>(future: Pin<&mut F>, observed: &RefCell<Observed>) -> Poll<F::Output> {
    observed.borrow_mut().context = 2;
    future.poll(&mut Context::from_waker(Waker::noop()))
}
fn fixture() -> (PortParts<TestDevice, 512>, Rc<RefCell<Observed>>, TestTimer) {
    let mut hardware = Hardware::default();
    oer_esp32s31_ieee80211_mac::crypto::install_sta_pairwise_ccmp(
        &mut hardware,
        super::super::PEER,
        &[7; 16],
    )
    .expect("seed one key before the driver's first poll");
    let (parts, observed, timer) = installed_with_hardware(Vec::new(), hardware);
    observed.borrow_mut().check_context = true;
    observed.borrow_mut().context = 2;
    (parts, observed, timer)
}

struct FailStop;
impl<Id: radio::SlotIdentity> PortBody<Id, Backend<Id>> for FailStop {
    type Output = ();
    async fn run<'p, 'id>(
        self,
        owner: radio::Owner<'p, 'id, Id, Backend<Id>, radio::Disabled>,
        _: Res<'id, Id>,
    ) -> radio::Ended<Id, Backend<Id>, ()>
    where
        Backend<Id>: 'id,
        Id: 'id,
        'id: 'p,
    {
        radio::Ended::fail_stop(owner.fail_stop(), ())
    }
}

struct QueueThenFail {
    requested: Rc<Cell<bool>>,
    dropped: Rc<Cell<usize>>,
}
impl<Id: radio::SlotIdentity> PortBody<Id, Backend<Id>> for QueueThenFail {
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
            let [buffer, ..] = resources.buffers;
            let [ticket, ..] = resources.tickets;
            submit_mpdu(
                &mut phase,
                buffer,
                ticket,
                Some(Payload(self.dropped)),
                None,
            );
        }
        self.requested.set(true);
        radio::Ended::fail_stop(owner.fail_stop(), ())
    }
}

#[test]
fn abandon_preempts_queued_publication_and_rx_and_erases_keys_only_on_p2() {
    let (parts, observed, _) = fixture();
    let dropped = Rc::new(Cell::new(0));
    let released = Rc::new(Cell::new(0));
    let requested = Rc::new(Cell::new(false));
    block_on(radio::with_local_slot(async |claim| {
        let mut slot = claim.place();
        let backend = Backend::new(parts).ok().expect("model backend");
        let (front, registration) = slot.register(backend).ok().expect("one installation");
        let mut p2 = Box::pin(registration.run());
        let mut p1 = Box::pin(radio::run_port(
            front,
            QueueThenFail {
                requested: requested.clone(),
                dropped: dropped.clone(),
            },
        ));
        for _ in 0..20 {
            assert!(poll_p1(p1.as_mut(), &observed).is_pending());
            if requested.get() {
                break;
            }
            assert!(poll_p2(p2.as_mut(), &observed).is_pending());
        }
        assert!(requested.get());
        assert!(observed.borrow().abandon_contexts.is_empty());
        let polls = observed.borrow().device_polls;
        observed.borrow_mut().received.push_back(TestReceive {
            bytes: [0; 26],
            released: released.clone(),
        });
        assert!(matches!(poll_p2(p2.as_mut(), &observed), Poll::Ready(None)));
        assert_eq!(observed.borrow().abandon_contexts, [2]);
        assert!(observed.borrow().erased_keys > 0);
        assert_eq!(observed.borrow().abandoned_publications, 0);
        assert_eq!(observed.borrow().device_polls, polls);
        assert_eq!(observed.borrow().received.len(), 1);
        assert_eq!(released.get(), 0);
        assert_eq!(dropped.get(), 0, "quarantine retains queued DMA and body");
        assert!(matches!(
            poll_p1(p1.as_mut(), &observed),
            Poll::Ready(radio::PortEnd::FailStopped { .. })
        ));
        drop(p1);
        observed.borrow_mut().context = 2;
        drop(p2);
        assert_eq!(observed.borrow().abandon_contexts, [2]);
        assert!(!observed.borrow().detached);
    }));
}

#[test]
fn shutdown_bound_expiry_is_unconfirmed_and_late_p2_cleanup_never_reissues_storage() {
    let (parts, observed, clock) = fixture();
    block_on(radio::with_local_slot(async |claim| {
        let mut slot = claim.place();
        let (front, registration) = slot
            .register(Backend::new(parts).ok().expect("backend"))
            .ok()
            .expect("installation");
        let mut p1 = Box::pin(radio::run_port(front, FailStop));
        assert!(poll_p1(p1.as_mut(), &observed).is_pending());
        clock
            .0
            .advance(SHUTDOWN_BOUND)
            .expect("owner advances the named bound");
        let Poll::Ready(radio::PortEnd::FailStopped { outcome, .. }) =
            poll_p1(p1.as_mut(), &observed)
        else {
            panic!("shutdown expires without polling P2");
        };
        assert!(matches!(outcome.outcome(), radio::Abandon::Unconfirmed(_)));
        assert!(observed.borrow().abandon_contexts.is_empty());
        observed.borrow_mut().context = 2;
        assert!(registration.run().await.is_none());
        assert_eq!(observed.borrow().abandon_contexts, [2]);
        assert!(observed.borrow().erased_keys > 0);
        assert!(matches!(outcome.outcome(), radio::Abandon::Unconfirmed(_)));
        drop(p1);
        let (other, _, _) = installed(Vec::new());
        assert!(
            slot.register(Backend::new(other).ok().expect("candidate backend"))
                .is_err()
        );
        assert!(!observed.borrow().detached);
    }));
}

#[test]
fn cancelling_before_first_driver_poll_drops_the_armed_guard_once_on_p2() {
    for construct_driver_future in [false, true] {
        let (parts, observed, _) = fixture();
        block_on(radio::with_local_slot(async |claim| {
            let mut slot = claim.place();
            let (front, registration) = slot
                .register(Backend::new(parts).ok().expect("backend"))
                .ok()
                .expect("installation");
            observed.borrow_mut().context = 1;
            drop(radio::run_port(front, FailStop));
            assert!(observed.borrow().abandon_contexts.is_empty());
            observed.borrow_mut().context = 2;
            if construct_driver_future {
                drop(registration.run());
            } else {
                drop(registration);
            }
            assert_eq!(observed.borrow().device_polls, 0);
            assert_eq!(observed.borrow().starts, 0);
            assert_eq!(observed.borrow().abandon_contexts, [2]);
            assert!(observed.borrow().erased_keys > 0);
            assert!(!observed.borrow().detached);
        }));
    }
}

#[test]
fn fail_stop_receives_the_actual_p2_answer_before_its_virtual_bound() {
    let (parts, observed, clock) = fixture();
    let began = clock.now();
    block_on(radio::with_local_slot(async |claim| {
        let mut slot = claim.place();
        let (front, registration) = slot
            .register(Backend::new(parts).ok().expect("backend"))
            .ok()
            .expect("installation");
        let mut p1 = Box::pin(radio::run_port(front, FailStop));
        assert!(poll_p1(p1.as_mut(), &observed).is_pending());
        assert!(observed.borrow().abandon_contexts.is_empty());
        observed.borrow_mut().context = 2;
        assert!(registration.run().await.is_none());
        let Poll::Ready(radio::PortEnd::FailStopped { outcome, .. }) =
            poll_p1(p1.as_mut(), &observed)
        else {
            panic!("one P2 answer completes fail_stop without advancing the clock");
        };
        // S31 has no DMA/entered-ISR stop proof; its timely answer is Unconfirmed.
        assert!(matches!(outcome.outcome(), radio::Abandon::Unconfirmed(_)));
        assert_eq!(clock.now(), began);
        assert_eq!(observed.borrow().abandon_contexts, [2]);
        assert_eq!(observed.borrow().device_polls, 0);
        drop(p1);
    }));
}
