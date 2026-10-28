use super::*;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use oer_ieee80211_wire::channel::WifiChannel;

fn binding() -> MonitorInjectionChannelBinding {
    MonitorInjectionChannelBinding::new(1, 1, WifiChannel::mhz20(1).unwrap()).unwrap()
}

fn request(bytes: &[u8]) -> MonitorInjectionRequest<'_> {
    MonitorInjectionRequest::new(
        binding(),
        MonitorInjectionRate::Ofdm6M,
        MonitorInjectionMpdu::from_no_fcs(bytes).unwrap(),
    )
}

fn marked_frame(value: u8) -> [u8; 24] {
    let mut bytes = [0; 24];
    bytes[10] = value;
    bytes
}

#[test]
fn the_last_mailbox_epoch_can_close_without_wrapping_or_reopening() {
    let mut resources = MonitorInjectionMailboxResources::<NoopRawMutex, 2, 32>::new();
    resources.generation.store(u32::MAX - 2, Ordering::Release);
    {
        let (handle, mut owner) = resources.begin_epoch().unwrap();
        assert_eq!(handle.mailbox_epoch(), u32::MAX - 1);
        owner.begin_bound_dwell(binding()).unwrap();
        owner.close();
        assert!(!owner.is_open());
        assert_eq!(handle.active_binding(), None);
        assert_eq!(handle.generation.load(Ordering::Acquire), u32::MAX);
        owner.close();
        assert_eq!(handle.generation.load(Ordering::Acquire), u32::MAX);
    }
    assert!(matches!(
        resources.begin_epoch(),
        Err(MonitorInjectionMailboxEpochError::GenerationExhausted)
    ));
    assert_eq!(resources.generation.load(Ordering::Acquire), u32::MAX);
}

#[test]
fn an_already_invalidated_mailbox_still_closes_its_binding() {
    let mut resources = MonitorInjectionMailboxResources::<NoopRawMutex, 2, 32>::new();
    let (handle, mut owner) = resources.begin_epoch().unwrap();
    owner.begin_bound_dwell(binding()).unwrap();
    handle.generation.store(100, Ordering::Release);
    owner.close();
    assert!(!owner.is_open());
    assert_eq!(handle.active_binding(), None);
    assert_eq!(handle.generation.load(Ordering::Acquire), 100);
}

#[test]
fn request_id_exhaustion_keeps_the_last_request_and_its_slot() {
    let mut resources = MonitorInjectionMailboxResources::<NoopRawMutex, 2, 32>::new();
    let (handle, mut owner) = resources.begin_epoch().unwrap();
    owner.begin_bound_dwell(binding()).unwrap();
    handle.next_request.store(u32::MAX - 1, Ordering::Release);
    let ticket = handle.try_send(request(&[0; 24])).unwrap();
    assert_eq!(ticket.request(), u32::MAX);
    assert_eq!(
        handle.try_send(request(&[0; 24])),
        Err(MonitorInjectionBackpressure::RequestIdExhausted)
    );
    assert_eq!(handle.outstanding(), 1);
    assert_eq!(owner.try_take(), Ok(Some(ticket)));
    assert_eq!(
        owner.complete_active(MonitorInjectionTerminal::DeadlineExpired),
        Ok(ticket)
    );
    assert_eq!(handle.try_receive().unwrap().ticket, ticket);
    assert_eq!(handle.outstanding(), 0);
    assert!(
        handle
            .slots
            .lock(|slots| slots.iter().all(|slot| slot.header.get().is_none()))
    );
}

#[test]
fn each_queued_index_returns_its_own_payload_and_completion() {
    let mut resources = MonitorInjectionMailboxResources::<NoopRawMutex, 3, 32>::new();
    let (handle, mut owner) = resources.begin_epoch().unwrap();
    owner.begin_bound_dwell(binding()).unwrap();
    let tickets = [1, 2, 3].map(|value| {
        let mut bytes = [0; 24];
        bytes[10] = value;
        handle.try_send(request(&bytes)).unwrap()
    });
    assert_eq!(handle.outstanding(), 3);
    for (ticket, value) in tickets.into_iter().zip([1, 2, 3]) {
        assert_eq!(owner.try_take(), Ok(Some(ticket)));
        assert_eq!(
            owner.with_active_request(|request| request.mpdu().bytes()[10]),
            Ok(value)
        );
        assert_eq!(
            owner.complete_active(MonitorInjectionTerminal::DeadlineExpired),
            Ok(ticket)
        );
        assert_eq!(handle.try_receive().unwrap().ticket, ticket);
    }
    assert_eq!(handle.outstanding(), 0);
    assert!(
        handle
            .slots
            .lock(|slots| slots.iter().all(|slot| slot.header.get().is_none()))
    );
    let reused = handle.try_send(request(&[0; 24])).unwrap();
    assert_eq!(
        owner.cancel_pending(MonitorInjectionCancelReason::MonitorStopped),
        Ok(1)
    );
    assert_eq!(handle.try_receive().unwrap().ticket, reused);
    assert_eq!(handle.outstanding(), 0);
    assert!(
        handle
            .slots
            .lock(|slots| slots.iter().all(|slot| slot.header.get().is_none()))
    );
}

#[test]
fn an_active_payload_callback_can_publish_another_request_without_aliasing() {
    let mut resources = MonitorInjectionMailboxResources::<NoopRawMutex, 2, 32>::new();
    let (handle, mut owner) = resources.begin_epoch().unwrap();
    owner.begin_bound_dwell(binding()).unwrap();
    let bytes = marked_frame(0x22);
    let first = handle.try_send(request(&bytes)).unwrap();
    assert_eq!(owner.try_take(), Ok(Some(first)));

    // This used to reborrow the entire slot array mutably while its first
    // payload was shared, panicking even though the second slot was free.
    let second = owner
        .with_active_request(|active| {
            assert_eq!(active.mpdu().bytes(), bytes);
            assert_eq!(handle.active_binding(), Some(binding()));
            let ticket = handle.try_send(active).unwrap();
            assert_eq!(active.mpdu().bytes(), bytes);
            ticket
        })
        .unwrap();
    assert_ne!(first, second);
    assert_eq!(handle.outstanding(), 2);
    assert_eq!(owner.active_ticket(), Some(first));
    for ticket in [first, second] {
        assert_eq!(
            owner.with_active_request(|active| active.mpdu().bytes().to_vec()),
            Ok(bytes.to_vec())
        );
        assert_eq!(
            owner.complete_active(MonitorInjectionTerminal::DeadlineExpired),
            Ok(ticket)
        );
        assert_eq!(handle.try_receive().unwrap().ticket, ticket);
        if ticket == first {
            assert_eq!(owner.try_take(), Ok(Some(second)));
        }
    }
    assert_eq!(handle.outstanding(), 0);
    assert_eq!(owner.try_take(), Ok(None));
    assert!(handle.try_receive().is_none());
}

#[test]
fn a_borrowed_free_payload_refuses_publication_and_returns_its_credit() {
    let mut resources = MonitorInjectionMailboxResources::<NoopRawMutex, 1, 32>::new();
    let (handle, mut owner) = resources.begin_epoch().unwrap();
    owner.begin_bound_dwell(binding()).unwrap();
    handle.slots.lock(|slots| {
        let [slot] = slots;
        let bytes = slot.bytes.borrow();
        assert_eq!(
            handle.try_send(request(&marked_frame(0x33))),
            Err(MonitorInjectionBackpressure::PayloadBorrowed { slot: 0 })
        );
        assert_eq!(*bytes, [0; 32]);
        assert!(slot.header.get().is_none());
    });
    assert_eq!(handle.outstanding(), 0);
    assert_eq!(owner.publishers_in_flight(), 0);
    assert_eq!(owner.try_take(), Ok(None));
    let ticket = handle.try_send(request(&marked_frame(0x33))).unwrap();
    assert_eq!(owner.try_take(), Ok(Some(ticket)));
    owner
        .complete_active(MonitorInjectionTerminal::DeadlineExpired)
        .unwrap();
    assert_eq!(handle.try_receive().unwrap().ticket, ticket);
    assert_eq!(handle.outstanding(), 0);
}

#[test]
fn a_payload_writer_refuses_reading_without_consuming_the_active_ticket() {
    let mut resources = MonitorInjectionMailboxResources::<NoopRawMutex, 1, 32>::new();
    let (handle, mut owner) = resources.begin_epoch().unwrap();
    owner.begin_bound_dwell(binding()).unwrap();
    let ticket = handle.try_send(request(&marked_frame(0x44))).unwrap();
    assert_eq!(owner.try_take(), Ok(Some(ticket)));
    handle.slots.lock(|slots| {
        let [slot] = slots;
        let bytes = slot.bytes.borrow_mut();
        assert_eq!(
            owner.with_active_request(|_| panic!("a refused read must not invoke its callback")),
            Err::<(), _>(MonitorInjectionMailboxInvariantError::PayloadBorrowed {
                ticket,
                slot: 0
            })
        );
        assert_eq!(&bytes[..24], &marked_frame(0x44));
        assert_eq!(slot.header.get().unwrap().ticket, ticket);
    });
    assert_eq!(owner.active_ticket(), Some(ticket));
    assert_eq!(handle.outstanding(), 1);
    assert_eq!(
        owner.with_active_request(|active| active.mpdu().bytes().to_vec()),
        Ok(marked_frame(0x44).to_vec())
    );
    owner
        .complete_active(MonitorInjectionTerminal::DeadlineExpired)
        .unwrap();
    assert_eq!(handle.try_receive().unwrap().ticket, ticket);
    assert_eq!(handle.outstanding(), 0);
}
