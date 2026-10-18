use core::{cell::Cell, convert::Infallible};
use std::boxed::Box;

use super::{
    BLUETOOTH_DTM_MAX_PACKET_CAPACITY, DtmError, DtmPool, DtmPositionalEventSeed, DtmPrepareError,
    DtmRxResultProjection, DtmRxRotationError, DtmSchedulerItemCompletionStatus, DtmStorage,
    DtmTxPacketPrepareError, codec::LINK_STATE_RX_TAIL_OFFSET,
};
use crate::{
    dtm_event_image::{DtmPositionalEventWords, DtmRole},
    le_phy_packet::{LeAccessAddress, LeCrcInit},
    scheduler_pool::{
        SchedulerAllocationConfig, SchedulerItemSpace, SchedulerPoolError,
        SchedulerPoolModelAddress, SchedulerRoleInstance, SchedulerRolePoolStorage,
    },
};

type Pool = DtmPool<1>;

fn pool() -> Pool {
    let storage = Box::leak(Box::new(SchedulerRolePoolStorage::<DtmStorage, 1>::new()));
    let numbers = SchedulerAllocationConfig::new(2, 3, 4)
        .unwrap()
        .direct_test_mode();
    Pool::bind_model(
        storage,
        SchedulerPoolModelAddress::new(0x2f00_0100).expect("test base is aligned"),
        numbers,
    )
    .expect("the pool fits controller SRAM")
}

fn candidate_words(seed: DtmPositionalEventSeed) -> DtmPositionalEventWords {
    let current = seed.words();
    let link_state = current.link_state().apply_reset(
        Some(seed.tx_header_head_projection()),
        Some(seed.rx_header_tail_projection()),
        crate::LeTxPower::from_dbm(0).expect("provider level"),
        0,
        DtmRole::Transmitter,
    );
    DtmPositionalEventWords::new(link_state, current.scheduler_item())
}

/// Prepare, submit and retire one event whose item recorded `status`.
fn run_event(pool: &mut Pool, instance: &SchedulerRoleInstance, status: u32) {
    pool.prepare_event(instance, |seed| Ok::<_, Infallible>(candidate_words(seed)))
        .unwrap();
    let id = pool.submit(instance, 0).unwrap();
    SchedulerItemSpace::new()
        .with(&*pool)
        .prepare_for_list(id, None, None);
    let (graph, _, _) = pool.shared(instance).unwrap();
    graph.scheduler_item.header().set_status(status);
    pool.retire(id).unwrap();
}

/// Hardware returns one packet through the current RX tail.
fn return_packet(pool: &Pool, instance: &SchedulerRoleInstance, result_word: u32, auxiliary: u16) {
    let (graph, binding, _) = pool.shared(instance).unwrap();
    let tail = graph.link_state.read_word(LINK_STATE_RX_TAIL_OFFSET);
    let header = if tail == binding.rx_header.controller_address().address() {
        &graph.rx_header
    } else {
        assert_eq!(tail, binding.rx_swap_reserve.controller_address().address());
        &graph.rx_swap_reserve
    };
    header.model_controller_completion_observed();
    graph
        .rx_packet
        .model_controller_completion(result_word, auxiliary);
}

#[test]
fn the_reset_selects_the_dtm_identity_and_profile() {
    let storage = DtmStorage::new();
    let reset = storage.link_state.reviewed_words().apply_reset(
        None,
        None,
        crate::LeTxPower::from_dbm(0).expect("provider level"),
        0,
        DtmRole::Transmitter,
    );
    assert_eq!(reset.access_address(), LeAccessAddress::DIRECT_TEST_MODE);
    assert_eq!(reset.crc_init(), LeCrcInit::LE_PRESET);
    assert!(reset.profile_word_14.direct_test_mode_is_selected());
    storage.link_state.write_reviewed_words(reset);
    assert_eq!(
        storage.link_state.reviewed_words().access_address(),
        LeAccessAddress::DIRECT_TEST_MODE
    );
}

#[test]
fn the_item_carries_the_dtm_number() {
    let mut pool = pool();
    let instance = pool.acquire().unwrap();
    let (graph, _, _) = pool.shared(&instance).unwrap();
    // Two advertising instances, one reserved number, three connections,
    // nine private numbers and four periodic synchronizations.
    assert_eq!(graph.scheduler_item.read_word(0x20 / 4), 2 + 1 + 3 + 9 + 4);
}

#[test]
fn every_standard_pdu_type_prepares_a_packet_and_others_are_refused() {
    let mut pool = pool();
    let instance = pool.acquire().unwrap();
    let mut payload = [0; BLUETOOTH_DTM_MAX_PACKET_CAPACITY];
    payload[..3].copy_from_slice(&[0xaa, 0xbb, 0xcc]);
    for payload_type in 0..8 {
        pool.prepare_tx_packet(&instance, payload_type, 3, &payload)
            .unwrap();
        assert!(pool.prepared_packet_bytes(&instance).is_some());
    }
    assert_eq!(
        pool.prepare_tx_packet(&instance, 8, 3, &payload),
        Err(DtmError::Packet(
            DtmTxPacketPrepareError::UnsupportedPayloadType
        ))
    );
}

#[test]
fn a_foreign_rx_packet_is_refused_before_the_builder_runs() {
    let mut pool = pool();
    let instance = pool.acquire().unwrap();
    {
        let (graph, binding, _) = pool.shared(&instance).unwrap();
        let foreign = super::codec::DtmBinding::new(0x2f00_4000, 0).unwrap();
        let _ = binding;
        graph.rx_header.model_retarget_rx_packet(foreign.rx_packet);
    }
    let called = Cell::new(false);
    let words = pool.words(&instance);
    assert_eq!(
        pool.prepare_event(&instance, |seed| {
            called.set(true);
            Ok::<_, Infallible>(candidate_words(seed))
        }),
        Err(DtmPrepareError::CurrentRxTailPacketMismatch)
    );
    assert!(!called.get());
    assert_eq!(pool.words(&instance), words);
}

#[test]
fn a_builder_failure_changes_nothing() {
    let mut pool = pool();
    let instance = pool.acquire().unwrap();
    let words = pool.words(&instance);
    assert_eq!(
        pool.prepare_event(&instance, |_| Err::<DtmPositionalEventWords, _>("rejected")),
        Err(DtmPrepareError::Build("rejected"))
    );
    assert_eq!(pool.words(&instance), words);
    assert_eq!(
        pool.submit(&instance, 0),
        Err(SchedulerPoolError::NotPrepared)
    );
}

#[test]
fn a_transmitter_event_recycles_the_item() {
    let mut pool = pool();
    let instance = pool.acquire().unwrap();
    run_event(&mut pool, &instance, 7);
    let result = pool.finish_event(&instance, false).unwrap();
    assert!(matches!(
        result.status,
        DtmSchedulerItemCompletionStatus::NonZero(status) if status.get() == 7
    ));
    assert_eq!(result.received, None);
    assert_eq!(pool.finish_event(&instance, false), Err(DtmError::State));
}

#[test]
fn an_unexecuted_item_finishes_as_aborted() {
    let mut pool = pool();
    let instance = pool.acquire().unwrap();
    run_event(&mut pool, &instance, u32::MAX);
    assert_eq!(
        pool.finish_event(&instance, true).unwrap().status,
        DtmSchedulerItemCompletionStatus::Aborted
    );
}

#[test]
fn receiver_events_rotate_both_headers() {
    let mut pool = pool();
    let instance = pool.acquire().unwrap();
    run_event(&mut pool, &instance, 0);
    assert_eq!(pool.finish_event(&instance, true).unwrap().received, None);

    for (word, auxiliary) in [(0xa500_0000, 0), (0x3100_0001, 7), (0x4200_0000, 3)] {
        run_event(&mut pool, &instance, 0);
        return_packet(&pool, &instance, word, auxiliary);
        assert_eq!(
            pool.finish_event(&instance, true).unwrap().received,
            Some(DtmRxResultProjection::from_word(word))
        );
    }
    run_event(&mut pool, &instance, 0);
    assert_eq!(pool.finish_event(&instance, true).unwrap().received, None);
}

#[test]
fn a_returned_packet_with_a_sentinel_is_refused_unchanged() {
    let mut pool = pool();
    let instance = pool.acquire().unwrap();
    run_event(&mut pool, &instance, 0);
    return_packet(&pool, &instance, 0x00ff_ffff, 0);
    let words = pool.words(&instance);
    assert_eq!(
        pool.finish_event(&instance, true),
        Err(DtmError::Rotation(
            DtmRxRotationError::ReturnedResultNotProduced
        ))
    );
    assert_eq!(pool.words(&instance), words);
}
