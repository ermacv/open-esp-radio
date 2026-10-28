use super::*;
use oer_ieee80211_wire::{channel::WifiChannel, monitor_injection::MonitorInjectionMpdu};

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
    let mut resources = MonitorInjectionMailboxResources::<2, 32>::new();
    resources.generation.store(u32::MAX - 2, Ordering::Release);
    {
        let (handle, mut owner) = resources.begin_epoch().unwrap();
        assert_eq!(handle.mailbox_epoch(), u32::MAX - 1);
        owner.begin_bound_dwell(binding()).unwrap();
        owner.close().unwrap();
        assert!(!owner.is_open());
        assert_eq!(handle.active_binding(), Ok(None));
        assert_eq!(handle.generation.load(Ordering::Acquire), u32::MAX);
        owner.close().unwrap();
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
    let mut resources = MonitorInjectionMailboxResources::<2, 32>::new();
    let (handle, mut owner) = resources.begin_epoch().unwrap();
    owner.begin_bound_dwell(binding()).unwrap();
    handle.generation.store(100, Ordering::Release);
    owner.close().unwrap();
    assert!(!owner.is_open());
    assert_eq!(handle.active_binding(), Ok(None));
    assert_eq!(handle.generation.load(Ordering::Acquire), 100);
}

#[test]
fn request_id_exhaustion_keeps_the_last_request_and_its_slot() {
    let mut resources = MonitorInjectionMailboxResources::<2, 32>::new();
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
    assert!(!owner.has_pending());
    assert_eq!(owner.active_ticket(), None);
}

#[test]
fn each_queued_index_returns_its_own_payload_and_completion() {
    let mut resources = MonitorInjectionMailboxResources::<3, 32>::new();
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
    assert!(!owner.has_pending());
    assert_eq!(owner.active_ticket(), None);
    let reused = handle.try_send(request(&[0; 24])).unwrap();
    assert_eq!(
        owner.cancel_pending(MonitorInjectionCancelReason::MonitorStopped),
        Ok(1)
    );
    assert_eq!(handle.try_receive().unwrap().ticket, reused);
    assert_eq!(handle.outstanding(), 0);
    assert!(!owner.has_pending());
    assert_eq!(owner.active_ticket(), None);
}

#[test]
fn an_active_payload_callback_can_publish_another_request_without_aliasing() {
    let mut resources = MonitorInjectionMailboxResources::<2, 32>::new();
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
            assert_eq!(handle.active_binding(), Ok(Some(binding())));
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
fn a_binding_lent_at_that_instant_refuses_both_sides_without_waiting() {
    let mut resources = MonitorInjectionMailboxResources::<1, 32>::new();
    let (handle, mut owner) = resources.begin_epoch().unwrap();
    owner.begin_bound_dwell(binding()).unwrap();
    handle
        .active_binding
        .lend(|_| {
            assert_eq!(
                handle.try_send(request(&marked_frame(0x55))),
                Err(MonitorInjectionBackpressure::DwellChanging)
            );
            assert_eq!(
                owner.end_dwell(),
                Err(MonitorInjectionMailboxInvariantError::BindingBusy)
            );
            assert_eq!(
                owner.try_take(),
                Err(MonitorInjectionMailboxInvariantError::BindingBusy)
            );
        })
        .unwrap();
    assert_eq!(handle.outstanding(), 0);
    assert_eq!(owner.publishers_in_flight(), 0);
    assert_eq!(handle.active_binding(), Ok(Some(binding())));
    let ticket = handle.try_send(request(&marked_frame(0x55))).unwrap();
    assert_eq!(owner.end_dwell(), Ok(1));
    assert_eq!(handle.try_receive().unwrap().ticket, ticket);
}
