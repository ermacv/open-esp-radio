use oer_ieee80211_mac::ap::ApPowerSaveObservation;

use oer_ieee80211_datapath::DestinationTxQueues;

use super::{super::support::with_authorized_ap, *};

#[test]
fn retained_and_source_backlogs_share_turns_and_recheck_power_save() {
    with_authorized_ap(|engine| {
        let pool = allocator::<7>();
        let mut resources = OwnedEndpointResources::<NoopRawMutex, 1, 7>::new();
        let interface = NetworkInterfaceId::new(0);
        let (mut device, radio) = resources.split(interface, [2; 6], allocator::<1>());
        radio.link_controller().set_link_up(true);
        let dma =
            PinnedTxPool::<64, 16, 8, 1>::pin_static(Box::leak(Box::new(PinnedTxPool::new())));
        let dma_resources = Box::leak(Box::new(
            PinnedTxResources::<NoopRawMutex, 64, 16, 8, 1>::new(),
        ));
        let source =
            DatapathTxConsumer::new(&radio, dma_resources.split(dma).for_interface(interface));
        let mut storage = super::super::super::AccessPointTxStorage::new();
        let mut ap = AccessPointNetworkTx::new(&mut storage, None);
        let identity = engine.admit_downlink([4; 6]).unwrap().identity();
        let key = ApTxFlowKey::associated(identity);
        for n in 0..3 {
            device.transmit(packet(pool, 4, n)).unwrap();
            ap.retain_active_frame(engine, radio.try_take_for([4; 6]).unwrap())
                .unwrap();
            device.transmit(packet(pool, 6, n)).unwrap();
        }
        for turn in 0..24_u8 {
            let peer = if turn % 2 == 0 { 4 } else { 6 };
            let (selected, frame) = ap
                .take_scheduled_active_or_network(engine, &source)
                .unwrap()
                .unwrap()
                .expect_frame();
            assert_eq!(selected.destination, [peer; 6]);
            assert_eq!(frame.ethernet()[14], turn / 2);
            drop(frame);
            device.transmit(packet(pool, peer, turn / 2 + 3)).unwrap();
            if peer == 4 {
                ap.retain_active_frame(engine, radio.try_take_for([4; 6]).unwrap())
                    .unwrap();
            }
            assert_eq!(ap.active_frames.len(), 3);
            assert_eq!(
                radio.tx_queue_len(),
                3,
                "unselected owners remain at source"
            );
        }
        engine
            .observe_power_save(
                ApPowerSaveObservation::Sleeping { peer: [4; 6] },
                oer_time::Instant::from_micros(10),
            )
            .unwrap();
        assert!(
            ap.take_scheduled_active_or_network(engine, &source)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            engine
                .association_status(identity)
                .unwrap()
                .buffered_unicast_frames,
            1
        );
        let (selected, frame) = ap
            .take_scheduled_active_or_network(engine, &source)
            .unwrap()
            .unwrap()
            .expect_frame();
        assert_eq!(selected.destination, [6; 6]);
        drop(frame);
        device.transmit(packet(pool, 6, 15)).unwrap();
        // Selected aggregate refill must also reclassify retained owners if
        // the peer sleeps after preparation started.
        assert!(
            ap.take_matching_active_or_network(engine, key, &source)
                .unwrap()
                .is_none()
        );
        assert_eq!(ap.active_frames.len(), 0);
        assert_eq!(
            engine
                .association_status(identity)
                .unwrap()
                .buffered_unicast_frames,
            3
        );
        assert_eq!(ap.buffered_unicast.len, 3);

        // Stale retained work must release admission even if another valid
        // queue stays nonempty. It cannot be rebound by address to this peer.
        device.transmit(packet(pool, 4, 99)).unwrap();
        let stale = ApAssociationIdentity::new(
            [4; 6],
            identity.association_id(),
            identity.association_epoch() + 1,
        )
        .unwrap();
        ap.restore_active_frame_front(
            ApTxFlowKey::associated(stale),
            radio.try_take_for([4; 6]).unwrap(),
        );
        assert!(!device.can_transmit());
        let (selected, frame) = ap
            .take_scheduled_active_or_network(engine, &source)
            .unwrap()
            .unwrap()
            .expect_frame();
        assert_eq!(selected.destination, [6; 6]);
        drop(frame);
        let released_stale = pool.try_alloc().unwrap();
        let released_selected = pool.try_alloc().unwrap();
        assert!(
            pool.try_alloc().is_none(),
            "five unselected owners remain retained"
        );
        assert_eq!(ap.active_frames.len(), 0);
        assert_eq!(ap.buffered_unicast.len, 3);
        drop((released_stale, released_selected));
        drop(ap);
        while let Some(frame) = radio.try_receive_tx() {
            drop(frame);
        }
    });
}

#[test]
fn a_destination_in_both_sources_gets_one_turn_and_retained_fifo_precedence() {
    with_authorized_ap(|engine| {
        let pool = allocator::<4>();
        let mut resources = OwnedEndpointResources::<NoopRawMutex, 1, 4>::new();
        let interface = NetworkInterfaceId::new(0);
        let (mut device, radio) = resources.split(interface, [2; 6], allocator::<1>());
        radio.link_controller().set_link_up(true);
        let dma =
            PinnedTxPool::<64, 16, 8, 1>::pin_static(Box::leak(Box::new(PinnedTxPool::new())));
        let dma_resources = Box::leak(Box::new(
            PinnedTxResources::<NoopRawMutex, 64, 16, 8, 1>::new(),
        ));
        let source =
            DatapathTxConsumer::new(&radio, dma_resources.split(dma).for_interface(interface));
        let mut storage = super::super::super::AccessPointTxStorage::new();
        let mut ap = AccessPointNetworkTx::new(&mut storage, None);
        device.transmit(packet(pool, 4, 1)).unwrap();
        ap.retain_active_frame(engine, radio.try_take_for([4; 6]).unwrap())
            .unwrap();
        device.transmit(packet(pool, 4, 2)).unwrap();
        for n in 1..=2 {
            device.transmit(packet(pool, 6, n)).unwrap();
        }
        for (peer, sequence) in [(4, 1), (6, 1), (4, 2), (6, 2)] {
            let (key, frame) = ap
                .take_scheduled_active_or_network(engine, &source)
                .unwrap()
                .unwrap()
                .expect_frame();
            assert_eq!(key.destination, [peer; 6]);
            assert_eq!(frame.ethernet()[14], sequence);
        }
        assert!(
            ap.take_scheduled_active_or_network(engine, &source)
                .unwrap()
                .is_none()
        );
        assert_eq!(ap.active_frames.len(), 0);
        assert_eq!(radio.tx_queue_len(), 0);
    });
}
