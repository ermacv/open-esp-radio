//! Deficit selection over real producer, retained and power-save owners.

use crate::datapath::{PinnedTxPool, PinnedTxResources, owned::DatapathTxConsumer};

use embassy_sync::blocking_mutex::raw::NoopRawMutex;

use oer_embassy_net_owned::{NetworkInterfaceId, OwnedEndpointResources};

use oer_ieee80211_mac::ap::ApPowerSaveObservation;

use oer_ieee80211_datapath::DestinationTxQueues;

use std::boxed::Box;

use super::{
    super::{
        super::ApTxSelection,
        support::{allocator, packet},
    },
    *,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Case {
    Weighted,
    Retained,
    Frontier,
    Awake,
    Sleeping,
    Idle,
    Pipeline,
}

#[test]
fn sustained_peer_selection_balances_modelled_cost_instead_of_packet_count() {
    run(Case::Weighted);
}

#[test]
fn retained_and_producer_heads_share_one_deficit_account() {
    run(Case::Retained);
}

#[test]
fn unreserved_aggregate_frontier_competes_with_other_destinations() {
    run(Case::Frontier);
}

#[test]
fn awake_buffered_prefix_participates_in_deficit_selection() {
    run(Case::Awake);
}

#[test]
fn sleeping_source_is_buffered_without_earning_a_deficit_grant() {
    run(Case::Sleeping);
}

#[test]
fn empty_demand_clears_unused_positive_credit() {
    run(Case::Idle);
}

#[test]
fn active_and_standby_selection_share_the_same_deficit_horizon() {
    run(Case::Pipeline);
}

fn run(case: Case) {
    with_authorized_ap(|engine| {
        let pool = allocator::<8>();
        let resources = Box::leak(Box::new(OwnedEndpointResources::<NoopRawMutex, 1, 8>::new()));
        let interface = NetworkInterfaceId::new(0);
        let (mut device, radio) = resources.split(interface, [2; 6], allocator::<1>());
        radio.link_controller().set_link_up(true);
        let dma =
            PinnedTxPool::<64, 16, 8, 1>::pin_static(Box::leak(Box::new(PinnedTxPool::new())));
        let resources = Box::leak(Box::new(
            PinnedTxResources::<NoopRawMutex, 64, 16, 8, 1>::new(),
        ));
        let source = DatapathTxConsumer::new(&radio, resources.split(dma).for_interface(interface));
        let mut storage = AccessPointTxStorage::new();
        let mut ledger = AccessPointAirtimeStorage::new(us(1000));
        let mut ap = AccessPointNetworkTx::new_with_airtime_scheduling(
            &mut storage,
            None,
            &mut ledger,
            us(100),
            tariff,
            |_| None, // No HT publication in these software-selection tests.
        );
        let first = ApTxFlowKey::associated(engine.admit_downlink([4; 6]).unwrap().identity());
        let second = ApTxFlowKey::associated(engine.admit_downlink([6; 6]).unwrap().identity());

        if case == Case::Idle {
            device.transmit(packet(pool, 4, 1)).unwrap();
            let (key, frame) = ap
                .take_scheduled_active_or_network(engine, &source)
                .unwrap()
                .unwrap()
                .expect_frame();
            drop(frame);
            let accounting = ap.airtime.as_mut().unwrap();
            accounting.reserve_active(engine, key).unwrap();
            accounting.publish_active();
            accounting.complete_active(work(1)).unwrap();
            let peer = AccessPointAirtimePeer::Unicast(key.association().unwrap());
            assert_eq!(ap.airtime_balance_micros(peer), Some(400));
            assert!(
                ap.take_scheduled_active_or_network(engine, &source)
                    .unwrap()
                    .is_none()
            );
            assert_eq!(ap.airtime_balance_micros(peer), Some(0));
            return;
        }

        if case == Case::Sleeping {
            engine
                .observe_power_save(
                    ApPowerSaveObservation::Sleeping { peer: [4; 6] },
                    oer_time::Instant::from_micros(2),
                )
                .unwrap();
            device.transmit(packet(pool, 4, 1)).unwrap();
            device.transmit(packet(pool, 6, 2)).unwrap();
            assert!(
                ap.take_scheduled_active_or_network(engine, &source)
                    .unwrap()
                    .is_none()
            );
            assert_eq!(ap.buffered_unicast.len, 1);
            assert_eq!(
                ap.airtime_balance_micros(AccessPointAirtimePeer::Unicast(
                    first.association().unwrap()
                )),
                None
            );
            let (key, frame) = ap
                .take_scheduled_active_or_network(engine, &source)
                .unwrap()
                .unwrap()
                .expect_frame();
            assert_eq!(key, second);
            assert_eq!(frame.ethernet()[14], 2);
            ap.airtime.as_mut().unwrap().cancel_selection().unwrap();
            return;
        }
        if case == Case::Awake {
            engine
                .observe_power_save(
                    ApPowerSaveObservation::Sleeping { peer: [6; 6] },
                    oer_time::Instant::from_micros(2),
                )
                .unwrap();
            device.transmit(packet(pool, 6, 10)).unwrap();
            ap.retain_active_frame(engine, radio.try_take_for([6; 6]).unwrap())
                .unwrap();
            engine
                .observe_power_save(
                    ApPowerSaveObservation::Active { peer: [6; 6] },
                    oer_time::Instant::from_micros(3),
                )
                .unwrap();
        }
        if matches!(case, Case::Retained | Case::Frontier) {
            device.transmit(packet(pool, 4, 10)).unwrap();
            let frame = radio.try_take_for([4; 6]).unwrap();
            if case == Case::Frontier {
                ap.prepared_first_key = Some(first);
                ap.prepared_first = Some(frame);
            } else {
                ap.retain_active_frame(engine, frame).unwrap();
            }
        }
        device.transmit(packet(pool, 4, 20)).unwrap();
        device.transmit(packet(pool, 6, 20)).unwrap();

        if case == Case::Pipeline {
            let (key, frame) = ap
                .take_scheduled_active_or_network(engine, &source)
                .unwrap()
                .unwrap()
                .expect_frame();
            assert_eq!(key, first);
            drop(frame);
            let accounting = ap.airtime.as_mut().unwrap();
            accounting.reserve_active(engine, key).unwrap();
            accounting.publish_active();
            let (key, frame) = ap
                .take_scheduled_active_or_network(engine, &source)
                .unwrap()
                .unwrap()
                .expect_frame();
            assert_eq!(key, second);
            drop(frame);
            let accounting = ap.airtime.as_mut().unwrap();
            accounting.reserve_standby(engine, key).unwrap();
            assert_eq!(
                accounting.require_selection_idle(),
                Err(AccessPointAirtimeError::TransactionBusy)
            );
            accounting.complete_active(work(5)).unwrap();
            accounting.publish_standby();
            accounting.complete_active(work(1)).unwrap();
            assert_eq!(
                ap.airtime_balance_micros(AccessPointAirtimePeer::Unicast(
                    first.association().unwrap()
                )),
                Some(-2000)
            );
            assert_eq!(
                ap.airtime_balance_micros(AccessPointAirtimePeer::Unicast(
                    second.association().unwrap()
                )),
                Some(400)
            );
            return;
        }

        if case != Case::Weighted {
            // A completed expensive exchange leaves debt for peer 4. RR would
            // still pick its oldest address; deficit selection must pick peer 6.
            let accounting = ap.airtime.as_mut().unwrap();
            accounting.reserve_active(engine, first).unwrap();
            accounting.publish_active();
            accounting.complete_active(work(5)).unwrap();
            if case == Case::Frontier {
                ap.reconsider_airtime_frontier().unwrap();
                assert!(ap.prepared_first.is_none());
            }
            let selection = ap
                .take_scheduled_active_or_network(engine, &source)
                .unwrap()
                .unwrap();
            match selection {
                ApTxSelection::Buffered(release) => {
                    assert!(case == Case::Awake);
                    assert_eq!(release.release.identity(), second.association().unwrap());
                    assert_eq!(release.buffered.frame(&ap.frame_arena).as_slice()[14], 10);
                    ap.finish_buffered_release(engine, release, true).unwrap();
                }
                ApTxSelection::Frame(key, frame) => {
                    assert!(case != Case::Awake);
                    assert_eq!(key, second);
                    assert_eq!(frame.ethernet()[14], 20);
                }
            }
            let accounting = ap.airtime.as_mut().unwrap();
            accounting.reserve_active(engine, second).unwrap();
            accounting.publish_active();
            accounting.complete_active(work(1)).unwrap();
            assert_eq!(
                radio.pending_for([4; 6]),
                1,
                "losing source head is untouched"
            );
            if matches!(case, Case::Retained | Case::Frontier) {
                let index = ap.active_frames.pop_key(first).unwrap();
                assert_eq!(ap.frame_arena.take(index).ethernet()[14], 10);
            } else {
                assert_eq!(
                    radio.try_take_for([6; 6]).unwrap().ethernet()[14],
                    20,
                    "awake prefix precedes younger producer data"
                );
            }
            return;
        }

        let mut visits = [0_u32; 2];
        for _ in 0..240 {
            let (key, frame) = ap
                .take_scheduled_active_or_network(engine, &source)
                .unwrap()
                .unwrap()
                .expect_frame();
            let index = usize::from(key == second);
            visits[index] += 1;
            drop(frame);
            let accounting = ap.airtime.as_mut().unwrap();
            accounting.reserve_active(engine, key).unwrap();
            accounting.publish_active();
            accounting
                .complete_active(work(if index == 0 { 5 } else { 1 }))
                .unwrap();
            device
                .transmit(packet(pool, key.destination[0], 20))
                .unwrap();
        }
        assert!(
            visits[1] >= visits[0] * 4 && visits[1] <= visits[0] * 6,
            "visits={visits:?}"
        );
        assert!(
            (visits[0] * 3000).abs_diff(visits[1] * 600) <= 3000,
            "one expensive exchange bounds modelled service difference: {visits:?}"
        );
        assert_eq!(radio.pending_for([4; 6]), 1);
        assert_eq!(radio.pending_for([6; 6]), 1);
    });
}
