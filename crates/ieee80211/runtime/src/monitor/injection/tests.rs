use super::*;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use oer_ieee80211_softmac::WifiChannel;

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
            .lock(|slots| slots.borrow().iter().all(|slot| slot.header.is_none()))
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
            .lock(|slots| slots.borrow().iter().all(|slot| slot.header.is_none()))
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
            .lock(|slots| slots.borrow().iter().all(|slot| slot.header.is_none()))
    );
}
