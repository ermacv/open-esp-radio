use std::boxed::Box;

use super::{
    LegacyAdvertisingError, LegacyAdvertisingPool, LegacyAdvertisingStorage,
    SCHEDULER_ITEM_ALLOCATION_NUMBER_OFFSET,
};
use crate::{
    AdvertisingCoexistencePriorities, LegacyAdvertisingPduError,
    LegacyAdvertisingPrimaryChannelPlan, SchedulerItemCoexistencePriority,
    coexistence::lanes_image,
    scheduler_pool::{
        SchedulerAllocationConfig, SchedulerItemSpace, SchedulerPoolError,
        SchedulerPoolModelAddress, SchedulerRoleInstance, SchedulerRolePoolStorage,
    },
};

const PDU: [u8; 8] = [0x02, 6, 1, 2, 3, 4, 5, 6];

fn pool() -> LegacyAdvertisingPool<2> {
    let storage = Box::leak(Box::new(SchedulerRolePoolStorage::<
        LegacyAdvertisingStorage,
        2,
    >::new()));
    let numbers = SchedulerAllocationConfig::new(4, 1, 0)
        .unwrap()
        .advertising(2, 2)
        .unwrap();
    LegacyAdvertisingPool::bind_model(
        storage,
        SchedulerPoolModelAddress::new(0x2f00_0100).expect("test base is aligned"),
        numbers,
    )
    .expect("the pool fits controller SRAM")
}

fn priorities(lanes: [u8; 4]) -> AdvertisingCoexistencePriorities {
    AdvertisingCoexistencePriorities {
        lanes: lanes.map(|lane| SchedulerItemCoexistencePriority::new(lane).unwrap()),
    }
}

fn all_channels() -> LegacyAdvertisingPrimaryChannelPlan {
    LegacyAdvertisingPrimaryChannelPlan::new(true, true, true).unwrap()
}

fn reset(pool: &mut LegacyAdvertisingPool<2>) -> SchedulerRoleInstance {
    let instance = pool.acquire().unwrap();
    pool.prepare_packet(&instance, &PDU).unwrap();
    pool.reset_link_state(
        &instance,
        crate::LeTxPower::from_dbm(0).expect("provider level"),
    )
    .unwrap();
    instance
}

fn item_word(
    pool: &LegacyAdvertisingPool<2>,
    instance: &SchedulerRoleInstance,
    item: usize,
    offset: usize,
) -> u32 {
    let (graph, _, _) = pool.shared(instance).unwrap();
    graph.items[item].words[offset].get()
}

#[test]
fn every_item_carries_its_instance_number() {
    let mut pool = pool();
    let first = pool.acquire().unwrap();
    let second = pool.acquire().unwrap();
    for item in 0..3 {
        assert_eq!(
            item_word(&pool, &first, item, SCHEDULER_ITEM_ALLOCATION_NUMBER_OFFSET),
            2
        );
        assert_eq!(
            item_word(
                &pool,
                &second,
                item,
                SCHEDULER_ITEM_ALLOCATION_NUMBER_OFFSET
            ),
            3
        );
    }
}

#[test]
fn an_event_lowers_one_unlinked_item_per_channel() {
    let mut pool = pool();
    let instance = reset(&mut pool);
    let event = pool
        .prepare_event(
            &instance,
            all_channels(),
            1_000,
            128,
            priorities([4, 0, 13, 13]),
        )
        .unwrap();
    assert_eq!(
        event.items().collect::<std::vec::Vec<_>>(),
        [(0, 1_000, 1_128), (1, 1_128, 1_256), (2, 1_256, 1_384)]
    );
    let (graph, _, _) = pool.shared(&instance).unwrap();
    for item in &graph.items {
        assert_eq!(item.header().hardware_next_image(), 0);
        // Each channel item carries the event's coexistence lanes.
        assert_eq!(
            item.words[super::SCHEDULER_ITEM_COEX_PRIORITIES_OFFSET].get() & 0x000f_ffff,
            lanes_image(&priorities([4, 0, 13, 13]).lanes)
        );
    }
    assert_eq!(graph.link_state.scheduler_head(), 0);
    assert_eq!(pool.pdu(&instance), Some(&PDU[..]));
}

#[test]
fn only_prepared_items_are_submitted_and_the_space_links_them() {
    let mut pool = pool();
    let instance = reset(&mut pool);
    assert_eq!(
        pool.submit(&instance, 0),
        Err(SchedulerPoolError::NotPrepared)
    );
    let channel_37 = LegacyAdvertisingPrimaryChannelPlan::new(true, false, false).unwrap();
    pool.prepare_event(&instance, channel_37, 0, 100, priorities([15; 4]))
        .unwrap();
    assert_eq!(
        pool.submit(&instance, 1),
        Err(SchedulerPoolError::NotPrepared)
    );
    let id = pool.submit(&instance, 0).unwrap();

    let space = SchedulerItemSpace::new().with(&pool);
    space.prepare_for_list(id, None, None);
    assert_eq!(space.completion_status(id), None);
    // The CPU cannot touch the instance while its item is listed.
    assert_eq!(
        pool.finish_event(&instance),
        Err(LegacyAdvertisingError::Pool(
            SchedulerPoolError::InstanceBusy
        ))
    );
    pool.retire(id).unwrap();
    pool.finish_event(&instance).unwrap();
    // The next event starts from the allocation-time items again.
    pool.prepare_event(&instance, all_channels(), 500, 100, priorities([15; 4]))
        .unwrap();
}

#[test]
fn finishing_an_event_restores_the_allocation_image() {
    let mut pool = pool();
    let instance = reset(&mut pool);
    let before: std::vec::Vec<u32> = (0..3)
        .flat_map(|item| (0..0x60 / 4).map(move |word| (item, word)))
        .map(|(item, word)| item_word(&pool, &instance, item, word))
        .collect();
    pool.prepare_event(&instance, all_channels(), 1_000, 128, priorities([15; 4]))
        .unwrap();
    pool.finish_event(&instance).unwrap();
    let after: std::vec::Vec<u32> = (0..3)
        .flat_map(|item| (0..0x60 / 4).map(move |word| (item, word)))
        .map(|(item, word)| item_word(&pool, &instance, item, word))
        .collect();
    assert_eq!(before, after);
    let (graph, binding, _) = pool.shared(&instance).unwrap();
    assert_eq!(
        graph.link_state.scheduler_head(),
        binding.items[0].controller_address().address()
    );
}

#[test]
fn refused_steps_keep_the_instance_state() {
    let mut pool = pool();
    let instance = pool.acquire().unwrap();
    assert_eq!(
        pool.reset_link_state(
            &instance,
            crate::LeTxPower::from_dbm(0).expect("provider level")
        ),
        Err(LegacyAdvertisingError::State)
    );
    assert!(matches!(
        pool.prepare_packet(&instance, &[0x02, 7, 1, 2, 3]),
        Err(LegacyAdvertisingError::Packet(_))
    ));
    pool.prepare_packet(&instance, &[0x00, 6, 1, 2, 3, 4, 5, 6])
        .unwrap();
    assert_eq!(
        pool.reset_link_state(
            &instance,
            crate::LeTxPower::from_dbm(0).expect("provider level")
        ),
        Err(LegacyAdvertisingError::Pdu(
            LegacyAdvertisingPduError::UnsupportedPduType
        ))
    );
    assert_eq!(
        pool.prepare_packet(&instance, &PDU),
        Err(LegacyAdvertisingError::State)
    );
    pool.clear(&instance).unwrap();
    assert_eq!(pool.pdu(&instance), None);
    pool.prepare_packet(&instance, &PDU).unwrap();
    pool.release(instance).unwrap();
}

#[test]
fn the_controller_inserts_the_advertiser_address_before_the_data() {
    let mut pool = pool();
    let instance = pool.acquire().unwrap();
    let expected = [0x42, 9, 1, 2, 3, 4, 5, 0xc6, 2, 1, 6];
    pool.prepare_packet(&instance, &expected).unwrap();
    pool.reset_link_state(
        &instance,
        crate::LeTxPower::from_dbm(7).expect("provider level"),
    )
    .unwrap();
    let (graph, _, state) = pool.shared(&instance).unwrap();
    let super::LegacyAdvertisingState::Reset(length) = *state else {
        panic!("the reset state keeps the packet length")
    };
    assert_eq!(
        graph
            .tx_packet
            .model_transmitted_pdu(length, [1, 2, 3, 4, 5, 0xc6]),
        expected
    );
    assert_eq!(pool.pdu(&instance), Some(&expected[..]));
}
