use crate::roles::access_point::{
    PendingApBufferedReleases,
    network_tx::{ApTxSelection, BufferedReleaseCause},
    retain_ap_power_save_action,
};

use oer_ieee80211_mac::ap::ApPowerSaveObservation;

use oer_ieee80211_datapath::DestinationTxQueues;

use super::{super::support::with_authorized_ap, *};

#[test]
fn awake_ps_and_producer_share_turns_without_reserving_on_the_wake_edge() {
    with_authorized_ap(|engine| {
        let pool = allocator::<6>();
        let mut resources = OwnedEndpointResources::<NoopRawMutex, 1, 6>::new();
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
        let mut pending = PendingApBufferedReleases::new();
        let identity = engine.admit_downlink([4; 6]).unwrap().identity();
        engine
            .observe_power_save(
                ApPowerSaveObservation::Sleeping { peer: [4; 6] },
                oer_time::Instant::from_micros(2),
            )
            .unwrap();
        for n in 0..3 {
            device.transmit(packet(pool, 4, n)).unwrap();
            ap.retain_active_frame(engine, radio.try_take_for([4; 6]).unwrap())
                .unwrap();
        }
        assert!(!ap.has_prepared());
        let action = engine
            .observe_power_save(
                ApPowerSaveObservation::Active { peer: [4; 6] },
                oer_time::Instant::from_micros(3),
            )
            .unwrap();
        retain_ap_power_save_action(engine, &mut pending, action).unwrap();
        ap.refresh_awake_demand(engine);
        assert!(
            pending.is_empty(),
            "PM=0 must not bypass destination selection"
        );
        assert!(
            ap.has_prepared(),
            "wake-only demand must reach the TX scheduler"
        );
        assert_eq!(ap.buffered_unicast.len, 3);
        assert!(ap.prepared_buffered_release.is_none());
        assert!(
            !engine
                .association_status(identity)
                .unwrap()
                .buffered_release_in_flight
        );
        for n in 0..2 {
            device.transmit(packet(pool, 6, n)).unwrap();
        }
        device.transmit(packet(pool, 4, 3)).unwrap();
        assert!(
            ap.take_matching_active_or_network(engine, ApTxFlowKey::associated(identity), &source)
                .unwrap()
                .is_none(),
            "aggregate refill cannot skip the older PS prefix"
        );
        assert_eq!(radio.tx_queue_len(), 3);
        for (peer, sequence) in [(4, 0), (6, 0), (4, 1), (6, 1), (4, 2), (4, 3)] {
            match ap
                .take_scheduled_active_or_network(engine, &source)
                .unwrap()
                .unwrap()
            {
                ApTxSelection::Buffered(owned) => {
                    assert_eq!(peer, 4);
                    assert_eq!(
                        owned.buffered.frame(&ap.frame_arena).as_slice()[14],
                        sequence
                    );
                    assert!(owned.can_publish(engine));
                    assert!(
                        engine
                            .association_status(identity)
                            .unwrap()
                            .buffered_release_in_flight
                    );
                    ap.finish_buffered_release(engine, owned, true).unwrap();
                }
                ApTxSelection::Frame(key, frame) => {
                    assert_eq!(key.destination, [peer; 6]);
                    assert_eq!(frame.ethernet()[14], sequence);
                }
            }
        }
        assert!(!ap.has_prepared());
        assert_eq!(
            engine
                .association_status(identity)
                .unwrap()
                .buffered_unicast_frames,
            0
        );
        let owners: std::vec::Vec<_> = (0..6).map(|_| pool.try_alloc().unwrap()).collect();
        assert!(pool.try_alloc().is_none());
        drop(owners);
    });
}

#[test]
fn awake_release_rollback_preserves_fifo_and_ps_poll_can_serve_a_sleeping_peer() {
    with_authorized_ap(|engine| {
        let pool = allocator::<3>();
        let mut resources = OwnedEndpointResources::<NoopRawMutex, 1, 3>::new();
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
        for peer in [4, 6] {
            engine
                .observe_power_save(
                    ApPowerSaveObservation::Sleeping { peer: [peer; 6] },
                    oer_time::Instant::from_micros(2),
                )
                .unwrap();
        }
        // Global arrival order must not give peer 6 two turns before peer 4.
        for (peer, sequence) in [(6, 0), (6, 1), (4, 0)] {
            device.transmit(packet(pool, peer, sequence)).unwrap();
            ap.retain_active_frame(engine, radio.try_take_for([peer; 6]).unwrap())
                .unwrap();
        }
        for peer in [4, 6] {
            engine
                .observe_power_save(
                    ApPowerSaveObservation::Active { peer: [peer; 6] },
                    oer_time::Instant::from_micros(3),
                )
                .unwrap();
        }
        let ApTxSelection::Buffered(owned) = ap
            .take_scheduled_active_or_network(engine, &source)
            .unwrap()
            .unwrap()
        else {
            panic!("PS head")
        };
        let identity = owned.release.identity();
        assert_eq!(identity.address(), [4; 6]);
        engine
            .observe_power_save(
                ApPowerSaveObservation::Sleeping { peer: [4; 6] },
                oer_time::Instant::from_micros(4),
            )
            .unwrap();
        assert!(
            !owned.can_publish(engine),
            "resleep invalidates voluntary release"
        );
        ap.finish_buffered_release(engine, owned, false).unwrap();
        assert_eq!(ap.buffered_unicast.len, 3);
        assert!(
            pool.try_alloc().is_none(),
            "rollback retains the original admission credit"
        );
        let status = engine.association_status(identity).unwrap();
        assert_eq!(status.buffered_unicast_frames, 1);
        assert!(!status.buffered_release_in_flight);

        let mut pending = PendingApBufferedReleases::new();
        let action = engine
            .observe_power_save(
                ApPowerSaveObservation::PsPoll {
                    peer: [4; 6],
                    association_id: identity.association_id(),
                },
                oer_time::Instant::from_micros(5),
            )
            .unwrap();
        retain_ap_power_save_action(engine, &mut pending, action).unwrap();
        let release = pending.pop().expect("PS-Poll explicitly owns one release");
        let index = ap.buffered_unicast.oldest_index_for(identity).unwrap();
        let owned = super::super::super::BufferedUnicastRelease {
            release,
            buffered: ap.buffered_unicast.take_at(index).unwrap(),
            cause: BufferedReleaseCause::PsPoll,
        };
        assert!(owned.can_publish(engine));
        assert_eq!(owned.buffered.frame(&ap.frame_arena).as_slice()[14], 0);
        ap.finish_buffered_release(engine, owned, true).unwrap();
        for sequence in 0..2 {
            let ApTxSelection::Buffered(owned) = ap
                .take_scheduled_active_or_network(engine, &source)
                .unwrap()
                .unwrap()
            else {
                panic!("PS head")
            };
            assert_eq!(owned.release.identity().address(), [6; 6]);
            assert_eq!(
                owned.buffered.frame(&ap.frame_arena).as_slice()[14],
                sequence
            );
            if sequence == 0 {
                ap.finish_buffered_release(engine, owned, false).unwrap();
                let ApTxSelection::Buffered(retried) = ap
                    .take_scheduled_active_or_network(engine, &source)
                    .unwrap()
                    .unwrap()
                else {
                    panic!("retried PS head")
                };
                assert_eq!(retried.buffered.frame(&ap.frame_arena).as_slice()[14], 0);
                ap.finish_buffered_release(engine, retried, true).unwrap();
            } else {
                ap.finish_buffered_release(engine, owned, true).unwrap();
            }
        }
        assert!(!ap.has_prepared());
        assert_eq!(ap.buffered_unicast.len, 0);
    });
}

#[test]
fn stale_ps_completion_does_not_consume_the_new_associations_buffered_owner() {
    with_authorized_ap(|engine| {
        let pool = allocator::<2>();
        let mut resources = OwnedEndpointResources::<NoopRawMutex, 1, 2>::new();
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
        engine
            .observe_power_save(
                ApPowerSaveObservation::Sleeping { peer: [4; 6] },
                oer_time::Instant::from_micros(2),
            )
            .unwrap();
        device.transmit(packet(pool, 4, 0)).unwrap();
        ap.retain_active_frame(engine, radio.try_take_for([4; 6]).unwrap())
            .unwrap();
        engine
            .observe_power_save(
                ApPowerSaveObservation::Active { peer: [4; 6] },
                oer_time::Instant::from_micros(3),
            )
            .unwrap();
        let ApTxSelection::Buffered(old) = ap
            .take_scheduled_active_or_network(engine, &source)
            .unwrap()
            .unwrap()
        else {
            panic!("old PS owner")
        };
        let old_identity = old.release.identity();

        reauthenticate_peer(engine, [4; 6]);
        let new_identity = engine.admit_downlink([4; 6]).unwrap().identity();
        assert_ne!(old_identity, new_identity);
        assert!(!old.can_publish(engine));
        engine
            .observe_power_save(
                ApPowerSaveObservation::Sleeping { peer: [4; 6] },
                oer_time::Instant::from_micros(12),
            )
            .unwrap();
        device.transmit(packet(pool, 4, 1)).unwrap();
        ap.retain_active_frame(engine, radio.try_take_for([4; 6]).unwrap())
            .unwrap();
        assert!(pool.try_alloc().is_none());
        ap.finish_buffered_release(engine, old, true).unwrap();
        let freed_old = pool.try_alloc().unwrap();
        assert!(
            pool.try_alloc().is_none(),
            "new generation retains its own packet"
        );
        let status = engine.association_status(new_identity).unwrap();
        assert_eq!(status.buffered_unicast_frames, 1);
        assert!(!status.buffered_release_in_flight);
        assert_eq!(ap.buffered_unicast.len, 1);
        drop(freed_old);
    });
}

fn reauthenticate_peer(engine: &mut ApEngine<'_>, peer: [u8; 6]) {
    let mut authentication = [0; 30];
    authentication[..2].copy_from_slice(&0x00b0_u16.to_le_bytes());
    authentication[4..10].fill(2);
    authentication[10..16].copy_from_slice(&peer);
    authentication[16..22].fill(2);
    authentication[26..28].copy_from_slice(&1_u16.to_le_bytes());
    let mut output = [0; 160];
    engine
        .handle_management(
            &mut Hardware,
            &authentication,
            [0; 32],
            0,
            oer_time::Instant::from_micros(10),
            &mut output,
        )
        .unwrap();
    let mut association = [0; 34];
    association[4..10].fill(2);
    association[10..16].copy_from_slice(&peer);
    association[16..22].fill(2);
    association[28..].copy_from_slice(&[1, 4, 12, 24, 48, 108]);
    engine
        .handle_management(
            &mut Hardware,
            &association,
            [0; 32],
            0,
            oer_time::Instant::from_micros(11),
            &mut output,
        )
        .unwrap();
}

#[test]
fn pending_ps_poll_discards_old_generations_and_preserves_current_requests() {
    with_authorized_ap(|engine| {
        let mut pending = PendingApBufferedReleases::new();
        // The protocol owns tickets, while the caller retains both packets.
        let pool = allocator::<3>();
        let old_packet = packet(pool, 4, 0);
        let current_packet = packet(pool, 6, 0);
        for peer in [[4; 6], [6; 6]] {
            let identity = engine.admit_downlink(peer).unwrap().identity();
            engine
                .observe_power_save(
                    ApPowerSaveObservation::Sleeping { peer },
                    oer_time::Instant::from_micros(2),
                )
                .unwrap();
            engine.commit_buffered_unicast(identity).unwrap();
            let action = engine
                .observe_power_save(
                    ApPowerSaveObservation::PsPoll {
                        peer,
                        association_id: identity.association_id(),
                    },
                    oer_time::Instant::from_micros(3),
                )
                .unwrap();
            retain_ap_power_save_action(engine, &mut pending, action).unwrap();
        }
        reauthenticate_peer(engine, [4; 6]);
        let identity = engine.admit_downlink([4; 6]).unwrap().identity();
        engine
            .observe_power_save(
                ApPowerSaveObservation::Sleeping { peer: [4; 6] },
                oer_time::Instant::from_micros(12),
            )
            .unwrap();
        let new_packet = packet(pool, 4, 1);
        engine.commit_buffered_unicast(identity).unwrap();
        let release = pending
            .pop_current(engine)
            .expect("current request survives a stale predecessor");
        assert_eq!(release.identity().address(), [6; 6]);
        engine
            .complete_buffered_unicast_release(release, false)
            .unwrap();
        assert!(pending.pop_current(engine).is_none());
        assert_eq!(
            engine
                .association_status(identity)
                .unwrap()
                .buffered_unicast_frames,
            1
        );
        assert!(
            !engine
                .association_status(identity)
                .unwrap()
                .buffered_release_in_flight
        );
        drop((old_packet, current_packet, new_packet));
    });
}
