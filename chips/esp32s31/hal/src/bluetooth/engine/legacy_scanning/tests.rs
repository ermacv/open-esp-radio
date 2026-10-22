use bytemuck::Zeroable;
use std::boxed::Box;

use super::{LegacyScanError, LegacyScanPool, LegacyScanStorage};
use crate::bluetooth::engine::{
    LegacyScanCoexistencePriorities, LegacyScanPrimaryChannel, LegacyScanResetConfig,
    LegacyScanSchedulerWindow, LegacyScanStartSelection, SchedulerItemCoexistencePriority,
    le_phy_packet::{LeAccessAddress, LeCrcInit},
    le_rx_chain::{LeRxChain, LeRxChainStorage},
    legacy_scanning_event_image::LegacyScanRxHeadProjection,
    rx_memory_list::RxMemoryListClass,
    scheduler_pool::{
        SchedulerAllocationConfig, SchedulerPoolError, SchedulerRoleInstance,
        SchedulerRolePoolStorage,
    },
};

fn pool() -> LegacyScanPool<'static, 'static, 1> {
    let storage = Box::leak(Box::new(
        SchedulerRolePoolStorage::<LegacyScanStorage, 1>::zeroed(),
    ));
    let numbers = SchedulerAllocationConfig::new(2, 3, 0).unwrap().scanning();
    LegacyScanPool::place::<crate::bluetooth::engine::HostDma<0x2f00_0100>>(
        crate::dma_static::Placed::host(storage),
        numbers,
    )
    .expect("the pool fits controller SRAM")
}

fn chain(class: RxMemoryListClass) -> LeRxChain<2> {
    let storage = Box::leak(Box::new(LeRxChainStorage::<2>::zeroed()));
    LeRxChain::place::<crate::bluetooth::engine::HostDma<0x2f00_8000>>(
        crate::dma_static::Placed::host(storage),
        class,
    )
    .unwrap()
}

fn config() -> LegacyScanResetConfig {
    LegacyScanResetConfig::le_1m_public(
        crate::bluetooth::engine::LeTxPower::from_dbm(0).expect("provider level"),
        crate::bluetooth::engine::LegacyScanType::Passive,
        crate::bluetooth::engine::LegacyScanFilterPolicy::AcceptAll,
    )
}

fn window() -> LegacyScanSchedulerWindow {
    LegacyScanSchedulerWindow::from_controller_ticks(1_000, 2_000).unwrap()
}

fn reset(
    pool: &mut LegacyScanPool<'static, 'static, 1>,
) -> SchedulerRoleInstance<'static, 'static> {
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
    pool: &mut LegacyScanPool<'static, 'static, 1>,
    instance: &SchedulerRoleInstance<'static, 'static>,
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
    assert_eq!(graph.link_state.layout.rx_list().read().class(), 1);
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
            item.layout.allocation_number().read().bits(),
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
            {
                let image = graph.items[2].layout.coexistence_priority().read();
                [
                    image.lane_0(),
                    image.lane_1(),
                    image.lane_2(),
                    image.lane_3(),
                ]
            },
            [4, 11, 0, 0]
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
        let layout = &graph.link_state.layout;
        assert_eq!(layout.tx_path().read().header(), 0);
        assert_eq!(layout.tx_head().read().bits(), 0);
    }

    let mut pool = pool();
    let active = pool.acquire().unwrap();
    pool.reset(
        &active,
        &chain(RxMemoryListClass::Scanning),
        LegacyScanResetConfig::le_1m_public(
            crate::bluetooth::engine::LeTxPower::from_dbm(0).expect("provider level"),
            crate::bluetooth::engine::LegacyScanType::Active,
            crate::bluetooth::engine::LegacyScanFilterPolicy::AcceptAll,
        ),
    )
    .unwrap();
    let (graph, binding, _) = pool.shared(&active).unwrap();
    let layout = &graph.link_state.layout;
    let header = binding.scan_request_header;
    assert_eq!(layout.tx_path().read().header(), header.compressed_image());
    assert_eq!(
        layout.tx_head().read().bits(),
        header.controller_address().address()
    );
    assert_eq!(
        layout.tx_tail().read().bits(),
        header.controller_address().address()
    );
    // SCAN_REQ with a 12-octet payload the Controller fills.
    assert_eq!(graph.scan_request_packet.pdu_header(), [0x03, 12]);
}

#[test]
fn every_item_carries_the_kind_of_its_scan_type() {
    for scan_type in [
        crate::bluetooth::engine::LegacyScanType::Passive,
        crate::bluetooth::engine::LegacyScanType::Active,
    ] {
        let mut pool = pool();
        let instance = pool.acquire().unwrap();
        pool.reset(
            &instance,
            &chain(RxMemoryListClass::Scanning),
            LegacyScanResetConfig::le_1m_public(
                crate::bluetooth::engine::LeTxPower::from_dbm(0).expect("provider level"),
                scan_type,
                crate::bluetooth::engine::LegacyScanFilterPolicy::AcceptAll,
            ),
        )
        .unwrap();
        let (graph, binding, _) = pool.shared(&instance).unwrap();
        for item in &graph.items {
            let word = item.layout.link_state().read();
            assert_eq!(word.kind(), scan_type.item_kind());
            // The link-state pointer beside the kind is unchanged.
            assert_eq!(word.address(), binding.link_state.compressed_image());
        }
    }
}

#[test]
fn the_filter_policy_changes_only_the_filter_words_and_a_later_reset_restores_them() {
    let link_state = |filter_policy| {
        let mut pool = pool();
        let reset = |pool: &mut LegacyScanPool<'static, 'static, 1>, filter_policy| {
            let instance = pool.acquire().unwrap();
            pool.reset(
                &instance,
                &chain(RxMemoryListClass::Scanning),
                LegacyScanResetConfig::le_1m_public(
                    crate::bluetooth::engine::LeTxPower::from_dbm(0).expect("provider level"),
                    crate::bluetooth::engine::LegacyScanType::Active,
                    filter_policy,
                ),
            )
            .unwrap();
            instance
        };
        // A filtering scanner before, released, leaves nothing behind.
        let earlier = reset(
            &mut pool,
            crate::bluetooth::engine::LegacyScanFilterPolicy::AcceptListOnly,
        );
        pool.release(earlier).unwrap();
        let instance = reset(&mut pool, filter_policy);
        let (graph, _, _) = pool.shared(&instance).unwrap();
        crate::bluetooth::engine::shared_layout::LinkStateImage::read(&graph.link_state.layout)
    };
    let all = link_state(crate::bluetooth::engine::LegacyScanFilterPolicy::AcceptAll);
    let listed = link_state(crate::bluetooth::engine::LegacyScanFilterPolicy::AcceptListOnly);
    assert_ne!(all.receive_context, listed.receive_context);
    assert_ne!(all.crc_initialization, listed.crc_initialization);
    let restored = crate::bluetooth::engine::shared_layout::LinkStateImage {
        receive_context: listed.receive_context.with_scan_accept_list_filter(false),
        crc_initialization: listed
            .crc_initialization
            .with_scan_filter_policy(0)
            .expect("zero fits"),
        ..listed
    };
    assert_eq!(all, restored);
    assert!(!all.receive_context.scan_accept_list_filter());
    assert!(listed.receive_context.scan_accept_list_filter());
}
