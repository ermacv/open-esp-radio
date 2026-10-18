use std::{boxed::Box, vec::Vec};

use super::{
    EVENT_ITEM, LINK_STATE_PACKET_CONTROL, LINK_STATE_RX_HEAD, LINK_STATE_RX_PATH,
    LINK_STATE_SCHEDULER_HEAD, LINK_STATE_TX_PATH, PeripheralConnectionCapturedAnchorAvailability,
    PeripheralConnectionDataChannel, PeripheralConnectionError, PeripheralConnectionEventSpan,
    PeripheralConnectionFirstEvent, PeripheralConnectionIdentity, PeripheralConnectionPool,
    PeripheralConnectionReceiveTime, PeripheralConnectionReceiveWait,
    PeripheralConnectionRecurringEvent, PeripheralConnectionRecurringReceiveWait,
    PeripheralConnectionSchedulerItemCompletionStatus, PeripheralConnectionSchedulerPriority,
    PeripheralConnectionSchedulerWindow, PeripheralConnectionStorage,
    PeripheralConnectionTransmitPduKind, SCHEDULER_ITEM_ALLOCATION_NUMBER,
    SCHEDULER_ITEM_CAPTURE_AVAILABLE, SCHEDULER_ITEM_CAPTURED_ANCHOR,
    SCHEDULER_ITEM_FREQUENCY_AND_PRIORITY, SCHEDULER_ITEM_RADIO_REQUEST_PRIORITIES,
    SCHEDULER_ITEM_RECEIVE_WAIT_CONFIGURATION,
};
use crate::{
    ConnectionCoexistencePriorities, DirectionFindingWorkspaceLink,
    DirectionFindingWorkspaceModelAddress, DirectionFindingWorkspaceStorage,
    PeripheralConnectionCoexistenceProtection, SchedulerItemCoexistencePriority,
    coexistence::lanes_image,
    le_rx_packet::LeRxOutcome,
    scheduler_pool::{
        SchedulerAllocationConfig, SchedulerPoolError, SchedulerPoolModelAddress,
        SchedulerRoleInstance, SchedulerRolePoolStorage,
    },
};

type Pool = PeripheralConnectionPool<2>;

const IDENTITY: PeripheralConnectionIdentity =
    PeripheralConnectionIdentity::new([0xd4, 0xc3, 0xb2, 0xa1], [0x33, 0x22, 0x11]);

fn pool() -> Pool {
    let storage = Box::leak(Box::new(SchedulerRolePoolStorage::<
        PeripheralConnectionStorage,
        2,
    >::new()));
    let numbers = SchedulerAllocationConfig::new(2, 2, 0)
        .unwrap()
        .connections();
    Pool::bind_model(
        storage,
        SchedulerPoolModelAddress::new(0x2f00_0100).expect("test base is aligned"),
        numbers,
    )
    .expect("the pool fits controller SRAM")
}

fn workspace() -> DirectionFindingWorkspaceLink {
    let storage = Box::leak(Box::new(DirectionFindingWorkspaceStorage::new()));
    DirectionFindingWorkspaceStorage::pin_static_model(
        storage,
        DirectionFindingWorkspaceModelAddress::new(0x2f02_0000).unwrap(),
    )
    .unwrap()
    .binding()
    .link()
}

fn lanes(event: u8, base: u8) -> ConnectionCoexistencePriorities {
    ConnectionCoexistencePriorities {
        event: SchedulerItemCoexistencePriority::new(event).unwrap(),
        base: SchedulerItemCoexistencePriority::new(base).unwrap(),
    }
}

fn first_event() -> PeripheralConnectionFirstEvent {
    PeripheralConnectionFirstEvent {
        channel: PeripheralConnectionDataChannel::new(0).unwrap(),
        receive_time: PeripheralConnectionReceiveTime::from_controller_ticks(24_000),
        event_span: PeripheralConnectionEventSpan::new(23_000).unwrap(),
        window: PeripheralConnectionSchedulerWindow::new(25_000, 26_000).unwrap(),
        receive_wait: PeripheralConnectionReceiveWait::new(1_250, 50).unwrap(),
        default_tx_power: crate::LeTxPower::from_dbm(0).expect("provider level"),
        priority: PeripheralConnectionSchedulerPriority::FIRST_EVENT,
        coexistence: lanes(9, 4),
        protection: None,
        raw_sequence_lead: 100,
    }
}

fn recurring_event(receive_wait_micros: u32) -> PeripheralConnectionRecurringEvent {
    PeripheralConnectionRecurringEvent {
        channel: PeripheralConnectionDataChannel::new(20).unwrap(),
        event_span: PeripheralConnectionEventSpan::new(23_000).unwrap(),
        window: PeripheralConnectionSchedulerWindow::new(50_000, 51_000).unwrap(),
        receive_wait: PeripheralConnectionRecurringReceiveWait::new(u64::from(receive_wait_micros))
            .unwrap(),
        priority: PeripheralConnectionSchedulerPriority::RECURRING_BASELINE,
        coexistence: lanes(4, 4),
        raw_sequence_lead: 100,
    }
}

/// An acquired connection whose first event has run and been finished.
fn active(pool: &mut Pool) -> SchedulerRoleInstance {
    let instance = pool.acquire().unwrap();
    pool.prepare_identity(&instance, IDENTITY).unwrap();
    pool.prepare_first_event(&instance, first_event(), workspace())
        .unwrap();
    let id = pool.submit(&instance, EVENT_ITEM).unwrap();
    pool.retire(id).unwrap();
    pool.finish_event(&instance).unwrap();
    instance
}

fn item_word(pool: &Pool, instance: &SchedulerRoleInstance, word: usize) -> u32 {
    let (graph, _, _) = pool.shared(instance).unwrap();
    graph.items[EVENT_ITEM].words[word].get()
}

/// Hardware receives one PDU into the private chain.
fn receive(pool: &mut Pool, instance: &SchedulerRoleInstance, pdu: &[u8]) {
    let cpu = pool.cpu(instance).unwrap();
    let rx = cpu.state.rx.as_mut().unwrap();
    assert!(rx.view(&cpu.graph.rx).emulate_receive(pdu, 0, 1_000));
}

/// Hardware transmits the packet after its TX cursor and, when the peer
/// acknowledges it, completes it and advances the cursor.
fn transmit(
    pool: &mut Pool,
    instance: &SchedulerRoleInstance,
    acknowledged: bool,
) -> Option<Vec<u8>> {
    let cpu = pool.cpu(instance).unwrap();
    let graph = &cpu.graph;
    let headers = [
        (&graph.tx_sentinel, cpu.binding.tx_sentinel),
        (&graph.tx_successor, cpu.binding.tx_successor),
    ];
    let path = &graph.link_state.words[LINK_STATE_TX_PATH];
    let current = path.get() & 0x000f_ffff;
    let (cursor, _) = headers
        .iter()
        .find(|(_, address)| address.compressed_image() == current)?;
    let next = cursor.snapshot()[0] & 0x000f_ffff;
    let (header, address) = headers
        .iter()
        .find(|(_, address)| address.compressed_image() == next)?;
    if header.transmission_completed() {
        return None;
    }
    let pdu = graph.tx_packet.model_pdu().to_vec();
    if acknowledged {
        header.model_complete_transmission();
        path.set((path.get() & !0x000f_ffff) | address.compressed_image());
    }
    Some(pdu)
}

#[test]
fn the_first_event_uses_the_private_chain_and_takes_the_free_head() {
    let mut pool = pool();
    let instance = pool.acquire().unwrap();
    assert_eq!(
        pool.prepare_first_event(&instance, first_event(), workspace()),
        Err(PeripheralConnectionError::State)
    );
    pool.prepare_identity(&instance, IDENTITY).unwrap();
    assert_eq!(pool.identity(&instance), Ok(IDENTITY));
    let event = pool
        .prepare_first_event(&instance, first_event(), workspace())
        .unwrap();
    assert_eq!(event.item(), EVENT_ITEM);
    assert_eq!(event.raw_window(), (25_000, 26_000));

    let (graph, binding, state) = pool.shared(&instance).unwrap();
    let words = &graph.link_state.words;
    let rx = state.rx.unwrap();
    // No RX class; the hardware cursor starts at the private software head.
    assert_eq!(words[LINK_STATE_PACKET_CONTROL].get(), 0);
    assert_eq!(
        words[LINK_STATE_RX_PATH].get() & 0x000f_ffff,
        rx.head_link().compressed_image()
    );
    assert_eq!(words[LINK_STATE_RX_HEAD].get(), rx.snapshot().0);
    assert_eq!(
        words[LINK_STATE_SCHEDULER_HEAD].get(),
        binding.items[0].controller_address().address()
    );
    let header = graph.items[EVENT_ITEM].header();
    assert_eq!(header.hardware_next_image(), 0);
    assert_eq!((header.raw_start(), header.raw_end()), (25_000, 26_000));
    assert_eq!(header.sequence_start(), 25_100);
    assert_eq!(
        pool.receive_time(&instance)
            .unwrap()
            .wrapping_controller_ticks(),
        24_000
    );
}

#[test]
fn connections_are_numbered_after_the_advertising_instances() {
    let mut pool = pool();
    let first = pool.acquire().unwrap();
    let second = pool.acquire().unwrap();
    assert_eq!(
        item_word(&pool, &first, SCHEDULER_ITEM_ALLOCATION_NUMBER),
        3
    );
    assert_eq!(
        item_word(&pool, &second, SCHEDULER_ITEM_ALLOCATION_NUMBER),
        4
    );
}

#[test]
fn finishing_reports_the_capture_and_returns_the_item() {
    let mut pool = pool();
    let instance = pool.acquire().unwrap();
    pool.prepare_identity(&instance, IDENTITY).unwrap();
    pool.prepare_first_event(&instance, first_event(), workspace())
        .unwrap();
    assert_eq!(
        pool.submit(&instance, 0),
        Err(SchedulerPoolError::NotPrepared)
    );
    let id = pool.submit(&instance, EVENT_ITEM).unwrap();
    {
        let (graph, _, _) = pool.shared(&instance).unwrap();
        let item = &graph.items[EVENT_ITEM];
        item.words[SCHEDULER_ITEM_CAPTURED_ANCHOR].set(0x1234);
        item.header().set_status(SCHEDULER_ITEM_CAPTURE_AVAILABLE);
    }
    pool.retire(id).unwrap();
    let result = pool.finish_event(&instance).unwrap();
    assert_eq!(
        result.status,
        PeripheralConnectionSchedulerItemCompletionStatus::NonZero
    );
    let PeripheralConnectionCapturedAnchorAvailability::Available(anchor) = result.capture else {
        panic!("the item published a capture")
    };
    assert_eq!(anchor.wrapping_controller_ticks(), 0x1234);

    let (graph, binding, _) = pool.shared(&instance).unwrap();
    assert_eq!(
        graph.link_state.words[LINK_STATE_SCHEDULER_HEAD].get(),
        binding.items[EVENT_ITEM].controller_address().address()
    );
    assert_eq!(
        graph.items[EVENT_ITEM].header().hardware_next_image(),
        binding.items[0].compressed_image()
    );
}

#[test]
fn an_unexecuted_item_finishes_as_aborted() {
    let mut pool = pool();
    let instance = pool.acquire().unwrap();
    pool.prepare_identity(&instance, IDENTITY).unwrap();
    pool.prepare_first_event(&instance, first_event(), workspace())
        .unwrap();
    let id = pool.submit(&instance, EVENT_ITEM).unwrap();
    crate::SchedulerItemSpace::new()
        .with(&pool)
        .prepare_for_list(id, None, None);
    pool.retire(id).unwrap();
    assert_eq!(
        pool.finish_event(&instance).unwrap().status,
        PeripheralConnectionSchedulerItemCompletionStatus::Aborted
    );
}

#[test]
fn recurring_events_select_the_receive_wait_form() {
    let mut pool = pool();
    let instance = active(&mut pool);
    for (micros, image) in [
        (0, 1),
        (500, 0x000f_0000 | 500),
        (0x1_0000, 0x001f_0000 | 0x8000),
    ] {
        pool.prepare_recurring_event(&instance, recurring_event(micros))
            .unwrap();
        assert_eq!(
            item_word(&pool, &instance, SCHEDULER_ITEM_RECEIVE_WAIT_CONFIGURATION),
            image
        );
        assert_eq!(
            pool.prepare_recurring_event(&instance, recurring_event(micros)),
            Err(PeripheralConnectionError::State)
        );
        let id = pool.submit(&instance, EVENT_ITEM).unwrap();
        pool.retire(id).unwrap();
        pool.finish_event(&instance).unwrap();
    }
    // The identity persists across events.
    assert_eq!(pool.identity(&instance), Ok(IDENTITY));
}

#[test]
fn received_pdus_rotate_through_the_private_chain() {
    let mut pool = pool();
    let instance = active(&mut pool);
    assert_eq!(pool.receive(&instance), Ok(None));
    for round in 0..10u8 {
        let pdu = [0x02, 1, round];
        receive(&mut pool, &instance, &pdu);
        let Ok(Some(LeRxOutcome::Received(received))) = pool.receive(&instance) else {
            panic!("the connection takes its own packet")
        };
        assert_eq!(received.as_bytes(), pdu);
        assert_eq!(pool.receive(&instance), Ok(None));
        let (graph, _, state) = pool.shared(&instance).unwrap();
        assert_eq!(
            graph.link_state.words[LINK_STATE_RX_HEAD].get(),
            state.rx.unwrap().snapshot().0
        );
    }
}

#[test]
fn one_packet_is_pending_until_the_peer_acknowledges_it() {
    let mut pool = pool();
    let instance = active(&mut pool);
    assert!(pool.can_enqueue_transmission(&instance));
    assert_eq!(
        pool.enqueue_transmission(
            &instance,
            PeripheralConnectionTransmitPduKind::Control,
            &[0x0c, 1]
        ),
        Ok(true)
    );
    assert!(!pool.can_enqueue_transmission(&instance));
    assert_eq!(
        pool.enqueue_transmission(
            &instance,
            PeripheralConnectionTransmitPduKind::Control,
            &[1]
        ),
        Ok(false)
    );
    assert_eq!(pool.reclaim_transmission(&instance), Ok(false));

    assert!(transmit(&mut pool, &instance, false).is_some());
    assert_eq!(pool.reclaim_transmission(&instance), Ok(false));
    assert!(transmit(&mut pool, &instance, true).is_some());
    assert_eq!(pool.reclaim_transmission(&instance), Ok(true));
    assert!(transmit(&mut pool, &instance, false).is_none());

    // The next packet uses the other header.
    assert_eq!(
        pool.enqueue_transmission(
            &instance,
            PeripheralConnectionTransmitPduKind::DataStartOrComplete,
            &[1, 2, 3]
        ),
        Ok(true)
    );
    assert!(transmit(&mut pool, &instance, true).is_some());
    assert_eq!(pool.reclaim_transmission(&instance), Ok(true));
}

#[test]
fn the_longest_encrypted_data_pdu_fits_and_a_longer_one_is_refused() {
    let mut pool = pool();
    let instance = active(&mut pool);
    // 251 octets of data plus a 4-octet MIC.
    assert!(matches!(
        pool.enqueue_transmission(
            &instance,
            PeripheralConnectionTransmitPduKind::DataStartOrComplete,
            &[0; 256]
        ),
        Err(PeripheralConnectionError::Transmit(_))
    ));
    assert_eq!(
        pool.enqueue_transmission(
            &instance,
            PeripheralConnectionTransmitPduKind::DataStartOrComplete,
            &[0; 255]
        ),
        Ok(true)
    );
}

#[test]
fn release_returns_a_pristine_instance() {
    let mut pool = pool();
    let instance = active(&mut pool);
    pool.release(instance).unwrap();
    let instance = pool.acquire().unwrap();
    assert_eq!(
        pool.enqueue_transmission(
            &instance,
            PeripheralConnectionTransmitPduKind::Control,
            &[1]
        ),
        Err(PeripheralConnectionError::State)
    );
    let (graph, binding, _) = pool.shared(&instance).unwrap();
    assert_eq!(
        graph.link_state.words[LINK_STATE_SCHEDULER_HEAD].get(),
        binding.items[EVENT_ITEM].controller_address().address()
    );
}

fn coexistence_lanes(pool: &Pool, instance: &SchedulerRoleInstance) -> u32 {
    item_word(pool, instance, SCHEDULER_ITEM_RADIO_REQUEST_PRIORITIES) & 0x3ff
}

fn protection(pool: &Pool, instance: &SchedulerRoleInstance) -> Option<u32> {
    let (graph, _, _) = pool.shared(instance).unwrap();
    let word = graph.link_state.words[super::LINK_STATE_PACKET_SEQUENCE].get();
    (word & super::LINK_STATE_COEXISTENCE_PROTECTED != 0)
        .then_some((word >> super::LINK_STATE_COEXISTENCE_PROTECTION_SHIFT) & 0x3f)
}

#[test]
fn every_event_carries_its_own_coexistence_lanes() {
    let mut pool = pool();
    let instance = active(&mut pool);
    assert_eq!(
        coexistence_lanes(&pool, &instance),
        lanes_image(&[lanes(9, 4).event, lanes(9, 4).base])
    );
    pool.prepare_recurring_event(&instance, recurring_event(1_000))
        .unwrap();
    assert_eq!(
        coexistence_lanes(&pool, &instance),
        lanes_image(&[lanes(4, 4).event, lanes(4, 4).base])
    );
}

#[test]
fn the_first_event_installs_the_coexistence_protection_only_when_given() {
    let mut pool = pool();
    let instance = active(&mut pool);
    assert_eq!(protection(&pool, &instance), None);

    let instance = pool.acquire().unwrap();
    pool.prepare_identity(&instance, IDENTITY).unwrap();
    let protected = PeripheralConnectionFirstEvent {
        protection: PeripheralConnectionCoexistenceProtection::new(20),
        ..first_event()
    };
    pool.prepare_first_event(&instance, protected, workspace())
        .unwrap();
    assert_eq!(protection(&pool, &instance), Some(20));
}

#[test]
fn a_recurring_event_keeps_the_power_the_first_event_stored() {
    fn power(pool: &Pool, instance: &SchedulerRoleInstance) -> u32 {
        let (graph, _, _) = pool.shared(instance).unwrap();
        u32::from(graph.link_state.power_index())
    }
    let mut pool = pool();
    let instance = active(&mut pool);
    let first = power(&pool, &instance);
    assert_eq!(
        first,
        u32::from(crate::LeTxPower::from_dbm(0).unwrap().index())
    );
    pool.prepare_recurring_event(&instance, recurring_event(1_000))
        .unwrap();
    assert_eq!(power(&pool, &instance), first);
}

#[test]
fn an_item_carries_the_event_priority_beside_a_fixed_high_nibble() {
    let mut pool = pool();
    let instance = active(&mut pool);
    let priority_byte =
        |pool: &Pool| item_word(pool, &instance, SCHEDULER_ITEM_FREQUENCY_AND_PRIORITY) & 0xff;
    let first = priority_byte(&pool);
    pool.prepare_recurring_event(&instance, recurring_event(1_000))
        .unwrap();
    let recurring = priority_byte(&pool);
    assert_eq!(
        first & 0x0f,
        u32::from(PeripheralConnectionSchedulerPriority::FIRST_EVENT.value())
    );
    assert_eq!(
        recurring & 0x0f,
        u32::from(PeripheralConnectionSchedulerPriority::RECURRING_BASELINE.value())
    );
    // The high nibble does not follow the priority.
    assert_ne!(first & 0x0f, recurring & 0x0f);
    assert_eq!(first & 0xf0, recurring & 0xf0);
}

#[test]
fn physical_receive_waits_and_event_spans_refuse_the_first_unrepresentable_value() {
    use super::{
        PeripheralConnectionReceiveWait as First,
        PeripheralConnectionRecurringReceiveWait as Recurring,
    };
    assert_eq!(First::new(65_473, 0).unwrap().total_micros(), 65_534);
    assert_eq!(First::new(65_474, 0), None);
    assert_eq!(First::new(0, 0), None);
    assert_eq!(First::new(1, u64::MAX), None);
    assert_eq!(First::new(u64::from(u32::MAX) + 1, 0), None);
    for valid in [0, 65_534, 65_536, 131_070] {
        assert_eq!(Recurring::new(valid).unwrap().total_micros(), valid as u32);
    }
    for invalid in [65_535, 131_071, u64::from(u32::MAX) + 1, u64::MAX] {
        assert_eq!(Recurring::new(invalid), None);
    }
    assert!(PeripheralConnectionEventSpan::new(i32::MAX as u32).is_some());
    assert_eq!(
        PeripheralConnectionEventSpan::new(i32::MAX as u32 + 1),
        None
    );
    assert_eq!(PeripheralConnectionEventSpan::new(0), None);
}
