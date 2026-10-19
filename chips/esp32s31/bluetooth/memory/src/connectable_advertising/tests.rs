use std::boxed::Box;

use super::{
    COMPRESSED_LINK_MASK, LINK_STATE_RX_HEAD, LINK_STATE_RX_LIST_CLASS, LINK_STATE_RX_TAIL,
    LINK_STATE_TX_HEAD, LINK_STATE_TX_TAIL, LINK_STATE_WORD_04, LINK_STATE_WORD_08,
    LegacyConnectableAdvIndPacketInput, LegacyConnectableAdvertisingError,
    LegacyConnectableAdvertisingMemoryInput, LegacyConnectableAdvertisingOwnAddress,
    LegacyConnectableAdvertisingPduFitError, LegacyConnectableAdvertisingPool,
    LegacyConnectableAdvertisingState, LegacyConnectableAdvertisingStorage,
    LegacyConnectableScanResponsePacketInput, SCHEDULER_ITEM_ALLOCATION_NUMBER,
    SCHEDULER_ITEM_COEX_PRIORITIES,
};
use crate::{
    AdvertisingCoexistencePriorities, LegacyAdvertisingPrimaryChannelPlan,
    SchedulerItemCoexistencePriority,
    coexistence::lanes_image,
    le_rx_chain::{LeRxChain, LeRxChainModelAddress, LeRxChainStorage},
    le_rx_packet::LeRxOutcome,
    rx_memory_list::RxMemoryListClass,
    scheduler_pool::{
        SchedulerAllocationConfig, SchedulerPoolError, SchedulerPoolModelAddress,
        SchedulerRoleInstance, SchedulerRolePoolStorage,
    },
};

const ADVERTISER: [u8; 6] = [1, 2, 3, 4, 5, 6];
const ADV_IND_PDU: [u8; 11] = [0x60, 9, 1, 2, 3, 4, 5, 6, 2, 1, 6];
const SCAN_RESPONSE_PDU: [u8; 8] = [0x44, 6, 1, 2, 3, 4, 5, 6];

type Pool = LegacyConnectableAdvertisingPool<1>;

fn priorities(lanes: [u8; 4]) -> AdvertisingCoexistencePriorities {
    AdvertisingCoexistencePriorities {
        lanes: lanes.map(|lane| SchedulerItemCoexistencePriority::new(lane).unwrap()),
    }
}

fn pool() -> Pool {
    let storage = Box::leak(Box::new(SchedulerRolePoolStorage::<
        LegacyConnectableAdvertisingStorage,
        1,
    >::new()));
    let numbers = SchedulerAllocationConfig::new(4, 1, 0)
        .unwrap()
        .advertising(3, 1)
        .unwrap();
    Pool::bind_model(
        storage,
        SchedulerPoolModelAddress::new(0x2f00_0100).expect("test base is aligned"),
        numbers,
    )
    .expect("the pool fits controller SRAM")
}

fn chain(class: RxMemoryListClass) -> LeRxChain<2> {
    let storage = Box::leak(Box::new(LeRxChainStorage::<2>::new()));
    LeRxChain::bind_model(
        storage,
        class,
        LeRxChainModelAddress::new(0x2f00_8000).expect("test base is aligned"),
    )
    .unwrap()
}

fn input() -> LegacyConnectableAdvertisingMemoryInput<'static> {
    LegacyConnectableAdvertisingMemoryInput::new(
        LegacyConnectableAdvIndPacketInput::try_from_encoded_extent(&ADV_IND_PDU, 9).unwrap(),
        Some(
            LegacyConnectableScanResponsePacketInput::try_from_encoded_extent(
                &SCAN_RESPONSE_PDU,
                6,
            )
            .unwrap(),
        ),
        LegacyConnectableAdvertisingOwnAddress::Random(ADVERTISER),
    )
}

fn prepared(pool: &mut Pool, chain: &LeRxChain<2>) -> SchedulerRoleInstance {
    let instance = pool.acquire().unwrap();
    pool.prepare(
        &instance,
        input(),
        chain,
        crate::LeTxPower::from_dbm(0).expect("provider level"),
    )
    .unwrap();
    instance
}

#[test]
fn preparation_chains_both_pdus_and_joins_the_non_scanning_chain() {
    let mut pool = pool();
    let chain = chain(RxMemoryListClass::NonScanning);
    let instance = prepared(&mut pool, &chain);
    assert_eq!(pool.adv_ind_pdu(&instance), Some(&ADV_IND_PDU[..]));
    assert_eq!(
        pool.scan_response_pdu(&instance),
        Some(&SCAN_RESPONSE_PDU[..])
    );

    let (graph, binding, state) = pool.shared(&instance).unwrap();
    let words = &graph.link_state.words;
    assert_eq!(words[LINK_STATE_RX_LIST_CLASS].get() >> 28, 2);
    assert_eq!(
        words[LINK_STATE_WORD_04].get() & COMPRESSED_LINK_MASK,
        binding.scan_response_header.compressed_image()
    );
    assert_eq!(words[LINK_STATE_WORD_08].get() & COMPRESSED_LINK_MASK, 0);
    let (head, tail, _) = chain.snapshot();
    assert_eq!(
        (
            words[LINK_STATE_RX_HEAD].get(),
            words[LINK_STATE_RX_TAIL].get()
        ),
        (head, tail)
    );
    assert_eq!(
        words[LINK_STATE_TX_HEAD].get(),
        binding.adv_ind_header.controller_address().address()
    );
    assert_eq!(
        words[LINK_STATE_TX_TAIL].get(),
        binding.scan_response_header.controller_address().address()
    );
    let LegacyConnectableAdvertisingState::Prepared(prepared) = *state else {
        panic!("the instance holds its advertisement")
    };
    assert_eq!(
        graph
            .adv_ind_packet
            .model_transmitted_pdu(prepared.adv_ind, ADVERTISER),
        ADV_IND_PDU
    );
    assert_eq!(
        graph
            .scan_response_packet
            .model_transmitted_pdu(prepared.scan_response.unwrap(), ADVERTISER),
        SCAN_RESPONSE_PDU
    );
}

#[test]
fn a_set_without_scan_response_transmits_one_node_and_still_receives() {
    let mut pool = pool();
    let chain = chain(RxMemoryListClass::NonScanning);
    let instance = pool.acquire().unwrap();
    let only_receiving = LegacyConnectableAdvertisingMemoryInput::new(
        LegacyConnectableAdvIndPacketInput::try_from_encoded_extent(&ADV_IND_PDU, 9).unwrap(),
        None,
        LegacyConnectableAdvertisingOwnAddress::Random(ADVERTISER),
    );
    let power = crate::LeTxPower::from_dbm(0).expect("provider level");
    pool.prepare(&instance, only_receiving, &chain, power)
        .unwrap();
    assert_eq!(pool.adv_ind_pdu(&instance), Some(&ADV_IND_PDU[..]));
    assert_eq!(pool.scan_response_pdu(&instance), None);
    {
        let (graph, binding, _) = pool.shared(&instance).unwrap();
        let words = &graph.link_state.words;
        assert_eq!(graph.adv_ind_header.successor_image(), 0);
        assert_eq!(words[LINK_STATE_WORD_04].get() & COMPRESSED_LINK_MASK, 0);
        assert_eq!(
            words[LINK_STATE_TX_TAIL].get(),
            binding.adv_ind_header.controller_address().address()
        );
        // Receptions still join the non-scanning chain.
        assert_eq!(words[LINK_STATE_RX_LIST_CLASS].get() >> 28, 2);
    }

    // Clearing restores the two-node chain for the next set.
    pool.clear(&instance).unwrap();
    pool.prepare(&instance, input(), &chain, power).unwrap();
    let (graph, binding, _) = pool.shared(&instance).unwrap();
    assert_eq!(
        graph.adv_ind_header.successor_image(),
        binding.scan_response_header.compressed_image()
    );
    assert_eq!(
        graph.link_state.words[LINK_STATE_TX_TAIL].get(),
        binding.scan_response_header.controller_address().address()
    );
}

#[test]
fn the_item_carries_its_number_and_each_event_its_coexistence_lanes() {
    let mut pool = pool();
    let chain = chain(RxMemoryListClass::NonScanning);
    let instance = prepared(&mut pool, &chain);
    {
        let (graph, _, _) = pool.shared(&instance).unwrap();
        for item in &graph.items {
            assert_eq!(item.words[SCHEDULER_ITEM_ALLOCATION_NUMBER].get(), 3);
        }
    }
    let plan = LegacyAdvertisingPrimaryChannelPlan::new(true, true, true).unwrap();
    let lanes = priorities([9, 0, 13, 13]);
    pool.prepare_event(&instance, plan, 6_000, 200, 214, lanes)
        .unwrap();
    let (graph, _, _) = pool.shared(&instance).unwrap();
    for item in &graph.items {
        assert_eq!(
            item.words[SCHEDULER_ITEM_COEX_PRIORITIES].get() & 0x000f_ffff,
            lanes_image(&lanes.lanes)
        );
    }
}

#[test]
fn an_event_lowers_one_item_per_channel_and_finishing_restores_them() {
    let mut pool = pool();
    let chain = chain(RxMemoryListClass::NonScanning);
    let instance = prepared(&mut pool, &chain);
    assert_eq!(
        pool.submit(&instance, 0),
        Err(SchedulerPoolError::NotPrepared)
    );
    let plan = LegacyAdvertisingPrimaryChannelPlan::new(true, false, true).unwrap();
    let event = pool
        .prepare_event(&instance, plan, 6_000, 200, 214, priorities([15; 4]))
        .unwrap();
    assert_eq!(
        event.items().collect::<std::vec::Vec<_>>(),
        [(0, 6_000, 6_200), (1, 6_200, 6_400)]
    );
    {
        let (graph, _, _) = pool.shared(&instance).unwrap();
        for (index, start) in [(0, 6_000), (1, 6_200)] {
            let header = graph.items[index].header();
            assert_eq!((header.raw_start(), header.raw_end()), (start, start + 200));
            assert_eq!(header.sequence_start(), start + 214);
            assert_eq!(header.sequence_duration(), 200);
        }
        // The unselected third item keeps its allocation-time image.
        assert_eq!(graph.items[2].header().raw_start(), 0);
        assert_eq!(graph.link_state.scheduler_head(), 0);
    }
    assert_eq!(
        pool.submit(&instance, 2),
        Err(SchedulerPoolError::NotPrepared)
    );
    let first = pool.submit(&instance, 0).unwrap();
    let second = pool.submit(&instance, 1).unwrap();
    pool.retire(first).unwrap();
    pool.retire(second).unwrap();
    pool.finish_event(&instance).unwrap();
    assert_eq!(
        pool.finish_event(&instance),
        Err(LegacyConnectableAdvertisingError::State)
    );
    let (graph, binding, _) = pool.shared(&instance).unwrap();
    assert_eq!(
        graph.link_state.scheduler_head(),
        binding.items[0].controller_address().address()
    );
    assert!(
        graph
            .items
            .iter()
            .all(|item| item.header().raw_start() == 0)
    );
}

#[test]
fn requests_arrive_under_the_advertising_number() {
    let mut pool = pool();
    let mut chain = chain(RxMemoryListClass::NonScanning);
    let instance = prepared(&mut pool, &chain);
    let source = pool.receive_source(&instance).unwrap();
    let scan_request = [0x03, 12, 9, 9, 9, 9, 9, 9, 1, 2, 3, 4, 5, 6];
    assert!(chain.emulate_receive(&scan_request, 3, 1_000));
    let Some(LeRxOutcome::Received(pdu)) = chain.take(source).unwrap() else {
        panic!("the request belongs to this instance")
    };
    assert_eq!(pdu.as_bytes(), scan_request);
}

#[test]
fn a_scanning_chain_and_early_events_are_refused() {
    let mut pool = pool();
    let instance = pool.acquire().unwrap();
    assert_eq!(
        pool.prepare(
            &instance,
            input(),
            &chain(RxMemoryListClass::Scanning),
            crate::LeTxPower::from_dbm(0).expect("provider level")
        ),
        Err(LegacyConnectableAdvertisingError::ForeignReceiveClass)
    );
    assert_eq!(
        pool.prepare_event(
            &instance,
            LegacyAdvertisingPrimaryChannelPlan::new(true, false, false).unwrap(),
            0,
            1,
            0,
            priorities([15; 4])
        )
        .err(),
        Some(LegacyConnectableAdvertisingError::State)
    );
    assert_eq!(pool.adv_ind_pdu(&instance), None);
}

#[test]
fn packet_fit_requires_the_advertiser_address() {
    assert_eq!(
        LegacyConnectableAdvIndPacketInput::try_from_encoded_extent(&[0x00, 3, 1, 2, 3], 3),
        Err(LegacyConnectableAdvertisingPduFitError::AdvertiserAddressMissing { payload_bytes: 3 })
    );
    assert_eq!(
        LegacyConnectableScanResponsePacketInput::try_from_encoded_extent(&SCAN_RESPONSE_PDU, 7),
        Err(
            LegacyConnectableAdvertisingPduFitError::EncodedExtentMismatch {
                expected_bytes: 9,
                actual_bytes: 8
            }
        )
    );
}
