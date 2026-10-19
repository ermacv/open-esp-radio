use std::boxed::Box;

use super::{
    COMPRESSED_LINK_MASK, LINK_STATE_RX_CLASS_WORD, LINK_STATE_TX_HEAD_WORD,
    LINK_STATE_TX_TAIL_WORD, LINK_STATE_TX_WORD, LegacyScanError, LegacyScanPool,
    LegacyScanStorage, SCHEDULER_ITEM_ALLOCATION_NUMBER_WORD, SCHEDULER_ITEM_COEX_PRIORITIES_WORD,
    SCHEDULER_ITEM_KIND_MASK, SCHEDULER_ITEM_LINK_STATE_WORD,
};
use crate::{
    LegacyScanCoexistencePriorities, LegacyScanPrimaryChannel, LegacyScanResetConfig,
    LegacyScanSchedulerWindow, LegacyScanStartSelection, SchedulerItemCoexistencePriority,
    coexistence::lanes_image,
    le_phy_packet::{LeAccessAddress, LeCrcInit},
    le_rx_chain::{LeRxChain, LeRxChainModelAddress, LeRxChainStorage},
    legacy_scanning_event_image::LegacyScanRxHeadProjection,
    rx_memory_list::RxMemoryListClass,
    scheduler_pool::{
        SchedulerAllocationConfig, SchedulerPoolError, SchedulerPoolModelAddress,
        SchedulerRoleInstance, SchedulerRolePoolStorage,
    },
};

fn pool() -> LegacyScanPool<1> {
    let storage = Box::leak(Box::new(
        SchedulerRolePoolStorage::<LegacyScanStorage, 1>::new(),
    ));
    let numbers = SchedulerAllocationConfig::new(2, 3, 0).unwrap().scanning();
    LegacyScanPool::bind_model(
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

fn config() -> LegacyScanResetConfig {
    LegacyScanResetConfig::le_1m_public(
        crate::LeTxPower::from_dbm(0).expect("provider level"),
        crate::LegacyScanType::Passive,
        crate::LegacyScanFilterPolicy::AcceptAll,
    )
}

fn window() -> LegacyScanSchedulerWindow {
    LegacyScanSchedulerWindow::from_controller_ticks(1_000, 2_000).unwrap()
}

fn reset(pool: &mut LegacyScanPool<1>) -> SchedulerRoleInstance {
    let instance = pool.acquire().unwrap();
    pool.reset(&instance, &chain(RxMemoryListClass::Scanning), config())
        .unwrap();
    instance
}

fn priorities(lanes: [u8; 4]) -> LegacyScanCoexistencePriorities {
    LegacyScanCoexistencePriorities {
        lanes: lanes.map(|lane| SchedulerItemCoexistencePriority::new(lane).unwrap()),
    }
}

fn prepare(
    pool: &mut LegacyScanPool<1>,
    instance: &SchedulerRoleInstance,
) -> super::LegacyScanEvent {
    pool.prepare_event(
        instance,
        LegacyScanPrimaryChannel::Channel38,
        super::LegacyScanEventTiming {
            window: window(),
            window_ticks: super::LegacyScanWindowTicks::new(0x5555).unwrap(),
            raw_sequence_lead: 0x5c,
        },
        LegacyScanStartSelection::Requested,
        priorities([4, 11, 0, 0]),
    )
    .unwrap()
}

#[test]
fn the_reset_joins_the_scanning_chain() {
    let mut pool = pool();
    let instance = pool.acquire().unwrap();
    let chain = chain(RxMemoryListClass::Scanning);
    pool.reset(&instance, &chain, config()).unwrap();
    let (graph, binding, _) = pool.shared(&instance).unwrap();
    let image = graph.link_state.image();
    assert!(image.retains_rx_head(LegacyScanRxHeadProjection::from_bound(chain.head_link())));
    assert_eq!(image.crc_init(), LeCrcInit::LE_PRESET);
    // The start marks the reset link state before the first window.
    assert!(image.started_flag());
    assert_eq!(image.access_address(), LeAccessAddress::PRIMARY_ADVERTISING);
    assert_eq!(image.window_ticks(), 0);
    assert_eq!(
        graph.link_state.words[LINK_STATE_RX_CLASS_WORD].get() >> 28,
        1
    );
    // The free chain survives the reset.
    assert_eq!(
        graph.link_state.free_head(),
        binding.items[2].controller_address().address()
    );
}

#[test]
fn items_are_numbered_after_the_connections() {
    let pool = pool();
    let mut pool = pool;
    let instance = pool.acquire().unwrap();
    let (graph, _, _) = pool.shared(&instance).unwrap();
    for (index, item) in graph.items.iter().enumerate() {
        assert_eq!(
            item.words[SCHEDULER_ITEM_ALLOCATION_NUMBER_WORD].get(),
            2 + 1 + 3 + index as u32
        );
    }
}

#[test]
fn a_window_takes_the_free_head_and_finishing_returns_it() {
    let mut pool = pool();
    let instance = reset(&mut pool);
    let event = prepare(&mut pool, &instance);
    assert_eq!(event.item(), 2);
    assert_eq!(event.raw_window(), (1_000, 2_000));
    {
        let (graph, binding, _) = pool.shared(&instance).unwrap();
        assert_eq!(graph.items[2].header().hardware_next_image(), 0);
        assert_eq!(
            graph.link_state.free_head(),
            binding.items[1].controller_address().address()
        );
        assert_eq!(graph.link_state.image().window_ticks(), 0x5555);
        // The sequence starts one lead after the item and lasts its window;
        // hardware ends an item without one at once.
        assert_eq!(graph.items[2].header().sequence_start(), 1_000 + 0x5c);
        assert_eq!(graph.items[2].header().sequence_duration(), 1_000);
        // The window's item carries its coexistence lanes.
        assert_eq!(
            graph.items[2].words[SCHEDULER_ITEM_COEX_PRIORITIES_WORD].get() & 0x000f_ffff,
            lanes_image(&priorities([4, 11, 0, 0]).lanes)
        );
        assert_eq!(
            (
                graph.items[2].header().raw_start(),
                graph.items[2].header().raw_end()
            ),
            (1_000, 2_000)
        );
    }
    assert_eq!(
        pool.submit(&instance, 1),
        Err(SchedulerPoolError::NotPrepared)
    );
    let id = pool.submit(&instance, 2).unwrap();
    assert_eq!(
        pool.finish_event(&instance),
        Err(LegacyScanError::Pool(SchedulerPoolError::InstanceBusy))
    );
    pool.retire(id).unwrap();
    pool.finish_event(&instance).unwrap();

    let (graph, binding, _) = pool.shared(&instance).unwrap();
    assert_eq!(
        graph.link_state.free_head(),
        binding.items[2].controller_address().address()
    );
    assert_eq!(
        graph.items[2].header().hardware_next_image(),
        binding.items[1].compressed_image()
    );
}

#[test]
fn each_item_receives_under_its_own_number() {
    let mut pool = pool();
    let instance = reset(&mut pool);
    let first = pool.receive_source(&instance, 0).unwrap();
    let last = pool.receive_source(&instance, 2).unwrap();
    assert_ne!(first, last);
    assert_eq!(last.class(), RxMemoryListClass::Scanning);
}

#[test]
fn a_non_scanning_chain_is_refused() {
    let mut pool = pool();
    let instance = pool.acquire().unwrap();
    assert_eq!(
        pool.reset(&instance, &chain(RxMemoryListClass::NonScanning), config()),
        Err(LegacyScanError::ForeignReceiveClass)
    );
    assert_eq!(
        pool.prepare_event(
            &instance,
            LegacyScanPrimaryChannel::Channel37,
            super::LegacyScanEventTiming {
                window: window(),
                window_ticks: super::LegacyScanWindowTicks::new(1).unwrap(),
                raw_sequence_lead: 0x5c,
            },
            LegacyScanStartSelection::Requested,
            priorities([15; 4]),
        ),
        Err(LegacyScanError::State)
    );
}

#[test]
fn only_an_active_scanner_queues_its_scan_request() {
    let mut passive_pool = pool();
    let passive = reset(&mut passive_pool);
    {
        let (graph, _, _) = passive_pool.shared(&passive).unwrap();
        let words = &graph.link_state.words;
        assert_eq!(words[LINK_STATE_TX_WORD].get() & COMPRESSED_LINK_MASK, 0);
        assert_eq!(words[LINK_STATE_TX_HEAD_WORD].get(), 0);
    }

    let mut pool = pool();
    let active = pool.acquire().unwrap();
    pool.reset(
        &active,
        &chain(RxMemoryListClass::Scanning),
        LegacyScanResetConfig::le_1m_public(
            crate::LeTxPower::from_dbm(0).expect("provider level"),
            crate::LegacyScanType::Active,
            crate::LegacyScanFilterPolicy::AcceptAll,
        ),
    )
    .unwrap();
    let (graph, binding, _) = pool.shared(&active).unwrap();
    let words = &graph.link_state.words;
    let header = binding.scan_request_header;
    assert_eq!(
        words[LINK_STATE_TX_WORD].get() & COMPRESSED_LINK_MASK,
        header.compressed_image()
    );
    assert_eq!(
        words[LINK_STATE_TX_HEAD_WORD].get(),
        header.controller_address().address()
    );
    assert_eq!(
        words[LINK_STATE_TX_TAIL_WORD].get(),
        header.controller_address().address()
    );
    // SCAN_REQ with a 12-octet payload the Controller fills.
    assert_eq!(graph.scan_request_packet.pdu_header(), [0x03, 12]);
}

#[test]
fn every_item_carries_the_kind_of_its_scan_type() {
    for scan_type in [
        crate::LegacyScanType::Passive,
        crate::LegacyScanType::Active,
    ] {
        let mut pool = pool();
        let instance = pool.acquire().unwrap();
        pool.reset(
            &instance,
            &chain(RxMemoryListClass::Scanning),
            LegacyScanResetConfig::le_1m_public(
                crate::LeTxPower::from_dbm(0).expect("provider level"),
                scan_type,
                crate::LegacyScanFilterPolicy::AcceptAll,
            ),
        )
        .unwrap();
        let (graph, binding, _) = pool.shared(&instance).unwrap();
        for item in &graph.items {
            let word = item.words[SCHEDULER_ITEM_LINK_STATE_WORD].get();
            assert_eq!(word & SCHEDULER_ITEM_KIND_MASK, scan_type.item_kind());
            // The link-state pointer beside the kind is unchanged.
            assert_eq!(
                word & COMPRESSED_LINK_MASK,
                binding.link_state.compressed_image()
            );
        }
    }
}

#[test]
fn the_filter_policy_changes_only_the_filter_words_and_a_later_reset_restores_them() {
    let link_state = |filter_policy| {
        let mut pool = pool();
        let reset = |pool: &mut LegacyScanPool<1>, filter_policy| {
            let instance = pool.acquire().unwrap();
            pool.reset(
                &instance,
                &chain(RxMemoryListClass::Scanning),
                LegacyScanResetConfig::le_1m_public(
                    crate::LeTxPower::from_dbm(0).expect("provider level"),
                    crate::LegacyScanType::Active,
                    filter_policy,
                ),
            )
            .unwrap();
            instance
        };
        // A filtering scanner before, released, leaves nothing behind.
        let earlier = reset(&mut pool, crate::LegacyScanFilterPolicy::AcceptListOnly);
        pool.release(earlier).unwrap();
        let instance = reset(&mut pool, filter_policy);
        let (graph, _, _) = pool.shared(&instance).unwrap();
        graph
            .link_state
            .words
            .iter()
            .map(|word| word.get())
            .collect::<std::vec::Vec<_>>()
    };
    let all = link_state(crate::LegacyScanFilterPolicy::AcceptAll);
    let listed = link_state(crate::LegacyScanFilterPolicy::AcceptListOnly);
    let changed = all
        .iter()
        .zip(&listed)
        .filter(|(all, listed)| all != listed)
        .count();
    assert_eq!(changed, 2);
    // Filtering only adds bits to the accept-all image.
    for (all, listed) in all.iter().zip(&listed) {
        assert_eq!(all & listed, *all);
    }
}
