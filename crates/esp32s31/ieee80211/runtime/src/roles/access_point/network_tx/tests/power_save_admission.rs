//! Full endpoint admission must fit AP retention through PS release/rollback.

use crate::datapath::{PinnedTxPool, PinnedTxResources, owned::DatapathTxConsumer};

use embassy_sync::blocking_mutex::raw::NoopRawMutex;

use oer_embassy_net_owned::{NetworkInterfaceId, OwnedEndpointResources};

use oer_esp32s31_ieee80211_ap::{protocol::*, security::ApPairwiseKeyStorage};

use oer_ieee80211_mac::{
    ap::{ApAssociationSecurityObservation, ApPowerSaveObservation},
    beacon::AP_BEACON_CAPACITY,
    channel::WifiChannel,
    ssid::WifiSsid,
};

use std::{
    boxed::Box,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    task::{Wake, Waker},
};

use super::{
    super::{AP_SOFTWARE_TX_CAPACITY, AccessPointNetworkTx, ApEngine},
    support::{Hardware, allocator, packet},
};

#[derive(Default)]
struct Wakes(AtomicUsize);

impl Wake for Wakes {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn full_admission_can_be_retained_by_one_sleeping_peer() {
    exercise_retention(|_| 4);
}

#[test]
fn full_admission_can_be_retained_for_dtim_group_delivery() {
    exercise_retention(|_| 255);
}

#[test]
fn sleeping_peers_and_group_delivery_share_all_admission_credits() {
    exercise_retention(|n| [4, 6, 255][n % 3]);
}

fn exercise_retention(destination: impl Fn(usize) -> u8) {
    let mut peers = AccessPointPeerStorage::new();
    let mut service = AccessPointService::new_open(
        [2; 6],
        AccessPointClientLimit::new(2).unwrap(),
        AccessPointInactiveTimeout::default(),
        &mut peers,
    );
    for peer in [[4; 6], [6; 6]] {
        service.authenticate_open(peer, oer_time::Instant::from_micros(0));
        service
            .associate_open(
                peer,
                ApAssociationSecurityObservation {
                    privacy: false,
                    rsn_ie: None,
                    rsn_ie_count: 0,
                    rsnxe: None,
                    rsnxe_count: 0,
                    legacy_wpa_present: false,
                    malformed_elements: false,
                },
                ApAssociationCapabilities {
                    maximum_legacy_rate_500kbps: 108,
                    short_preamble: true,
                    ht: None,
                    qos_supported: false,
                },
                oer_time::Instant::from_micros(1),
            )
            .unwrap();
        service
            .observe_power_save(
                ApPowerSaveObservation::Sleeping { peer },
                oer_time::Instant::from_micros(2),
            )
            .unwrap();
    }
    let mut beacon = [0; AP_BEACON_CAPACITY];
    let mut keys = ApPairwiseKeyStorage::new();
    let mut engine = ApEngine::start(
        &mut Hardware,
        service,
        &mut beacon,
        &mut keys,
        &WifiSsid::new(b"ps-test").unwrap(),
        WifiChannel::mhz20(13).unwrap(),
        100,
        2,
        1,
    )
    .unwrap_or_else(|_| panic!("open AP startup"));
    let identities = [[4; 6], [6; 6]].map(|peer| engine.admit_downlink(peer).unwrap().identity());
    let pool = allocator::<{ AP_SOFTWARE_TX_CAPACITY + 1 }>();
    let mut resources = Box::new(OwnedEndpointResources::<
        NoopRawMutex,
        1,
        AP_SOFTWARE_TX_CAPACITY,
    >::new());
    let interface = NetworkInterfaceId::new(0);
    let (mut device, radio) = resources.split(interface, [2; 6], allocator::<1>());
    radio.link_controller().set_link_up(true);
    let dma = PinnedTxPool::<64, 16, 8, 1>::pin_static(Box::leak(Box::new(PinnedTxPool::new())));
    let dma_resources = Box::leak(Box::new(
        PinnedTxResources::<NoopRawMutex, 64, 16, 8, 1>::new(),
    ));
    let source = DatapathTxConsumer::new(&radio, dma_resources.split(dma).for_interface(interface));
    let mut tx_storage = super::super::AccessPointTxStorage::new();
    let mut ap = AccessPointNetworkTx::new(&mut tx_storage, None);
    let wakes = Arc::new(Wakes::default());
    device.register_waker(&Waker::from(wakes.clone()));
    for n in 0..AP_SOFTWARE_TX_CAPACITY {
        device
            .transmit(packet(pool, destination(n), n as u8))
            .unwrap();
    }
    let rejected = packet(pool, 4, 255);
    let address = rejected.as_ptr();
    let rejected = device
        .transmit(rejected)
        .expect_err("all admission occupied");
    assert_eq!(rejected.as_ptr(), address);
    drop(rejected);
    while radio.tx_queue_len() != 0 {
        assert!(
            ap.take_scheduled_active_or_network(&mut engine, &source)
                .unwrap()
                .is_none()
        );
        assert!(
            !device.can_transmit(),
            "PS retention must not drop an admitted owner"
        );
    }
    assert_eq!(wakes.0.load(Ordering::Relaxed), 0);
    assert_eq!(ap.frame_arena.remaining_capacity(), 0);
    for identity in identities {
        let expected = (0..AP_SOFTWARE_TX_CAPACITY)
            .filter(|&n| [destination(n); 6] == identity.address())
            .count();
        assert_eq!(
            usize::from(
                engine
                    .association_status(identity)
                    .unwrap()
                    .buffered_unicast_frames
            ),
            expected
        );
    }

    // Drain each PS queue through the real portable release contract. At full
    // capacity, preparing and rolling back must neither return admission nor
    // disturb the oldest packet. Terminal completion must return it once.
    for identity in identities {
        let expected =
            (0..AP_SOFTWARE_TX_CAPACITY).filter(|&n| [destination(n); 6] == identity.address());
        for n in expected {
            let index = ap.buffered_unicast.oldest_index_for(identity).unwrap();
            let buffered = ap.buffered_unicast.take_at(index).unwrap();
            let release = engine
                .begin_buffered_unicast_release(identity)
                .unwrap()
                .unwrap();
            assert_eq!(buffered.frame(&ap.frame_arena).ethernet()[14], n as u8);
            let before = wakes.0.load(Ordering::Relaxed);
            engine
                .complete_buffered_unicast_release(release, false)
                .unwrap();
            ap.buffered_unicast.restore(buffered);
            assert_eq!(wakes.0.load(Ordering::Relaxed), before);
            let index = ap.buffered_unicast.oldest_index_for(identity).unwrap();
            let buffered = ap.buffered_unicast.take_at(index).unwrap();
            assert_eq!(buffered.frame(&ap.frame_arena).ethernet()[14], n as u8);
            let release = engine
                .begin_buffered_unicast_release(identity)
                .unwrap()
                .unwrap();
            engine
                .complete_buffered_unicast_release(release, true)
                .unwrap();
            buffered.complete(&mut ap.frame_arena);
        }
    }
    for n in (0..AP_SOFTWARE_TX_CAPACITY).filter(|&n| destination(n) == 255) {
        let index = ap.buffered_group.oldest_index().unwrap();
        let buffered = ap.buffered_group.take_at(index).unwrap();
        let release = engine.begin_buffered_group_release().unwrap().unwrap();
        assert_eq!(buffered.frame(&ap.frame_arena).ethernet()[14], n as u8);
        let before = wakes.0.load(Ordering::Relaxed);
        engine
            .complete_buffered_group_release(release, false)
            .unwrap();
        ap.buffered_group.restore(buffered);
        assert_eq!(wakes.0.load(Ordering::Relaxed), before);
        let index = ap.buffered_group.oldest_index().unwrap();
        let buffered = ap.buffered_group.take_at(index).unwrap();
        let release = engine.begin_buffered_group_release().unwrap().unwrap();
        engine
            .complete_buffered_group_release(release, true)
            .unwrap();
        buffered.complete(&mut ap.frame_arena);
    }
    assert!(device.can_transmit());
    assert_eq!(
        wakes.0.load(Ordering::Relaxed),
        1,
        "one full-to-available edge"
    );
    assert_eq!(ap.frame_arena.remaining_capacity(), AP_SOFTWARE_TX_CAPACITY);
    assert!(engine.begin_buffered_group_release().unwrap().is_none());
    for identity in identities {
        assert_eq!(
            engine
                .association_status(identity)
                .unwrap()
                .buffered_unicast_frames,
            0
        );
    }
    // Reuse every returned packet and admission credit; this also detects
    // missing returns hidden by a smaller follow-up workload.
    for n in 0..AP_SOFTWARE_TX_CAPACITY {
        device.transmit(packet(pool, 4, n as u8)).unwrap();
    }
    assert!(!device.can_transmit());
    for _ in 0..AP_SOFTWARE_TX_CAPACITY {
        drop(radio.try_receive_tx().unwrap());
    }
    assert!(device.can_transmit());

    // Stop with a full retained power-save backlog. Returning the AP storage
    // must return the real packet/admission owners, not merely reset indices.
    for n in 0..AP_SOFTWARE_TX_CAPACITY {
        device.transmit(packet(pool, 4, n as u8)).unwrap();
        ap.retain_active_frame(&mut engine, radio.try_receive_tx().unwrap())
            .unwrap();
    }
    assert!(!device.can_transmit());
    let storage = ap.into_storage();
    assert!(device.can_transmit());
    assert_eq!(
        storage.borrow().remaining_capacity(),
        AP_SOFTWARE_TX_CAPACITY
    );
    for n in 0..AP_SOFTWARE_TX_CAPACITY {
        device.transmit(packet(pool, 4, n as u8)).unwrap();
    }
    for _ in 0..AP_SOFTWARE_TX_CAPACITY {
        drop(radio.try_receive_tx().unwrap());
    }
}
