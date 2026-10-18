use core::sync::atomic::{AtomicU64, Ordering};
use oer_embassy_net_owned::{NetworkInterfaceId, NoopRawMutex, OwnedEndpointResources};
use oer_esp32s31_ieee80211_sta::connected_rx::{ConnectedRxEvent, ConnectedRxSink};
use oer_ieee80211_mac::data::EthernetFrameParts;
use xarxa_driver::{PacketPool, PacketPoolStorage};

use super::*;

#[derive(Default)]
struct Observer(u32);

impl ConnectedRxSink for Observer {
    fn publish(&mut self, _event: ConnectedRxEvent<'_>) {
        self.0 += 1;
    }
}

#[derive(Default)]
struct PipelineObserver {
    clock: AtomicU64,
    observations: std::sync::Mutex<std::vec::Vec<RxPipelineObservation>>,
}

impl RxPipelineObserver for PipelineObserver {
    fn now_micros(&self) -> u64 {
        self.clock.fetch_add(1, Ordering::Relaxed)
    }

    fn observe(&self, observation: RxPipelineObservation) {
        self.observations.lock().unwrap().push(observation);
    }
}

#[test]
fn sink_has_rx_only_capability_and_reports_bounded_backpressure() {
    const FRAME_CAPACITY: usize = 64;
    const QUEUE_DEPTH: usize = 1;
    type Resources = OwnedEndpointResources<NoopRawMutex, QUEUE_DEPTH, QUEUE_DEPTH>;

    let resources = std::boxed::Box::leak(std::boxed::Box::new(Resources::new()));
    let rx_storage =
        std::boxed::Box::leak(std::boxed::Box::new(PacketPoolStorage::<QUEUE_DEPTH>::new()));
    let rx_pool = std::boxed::Box::leak(std::boxed::Box::new(PacketPool::new(rx_storage)));
    let (mut device, runner) = resources.split(
        NetworkInterfaceId::new(0),
        [2, 3, 4, 5, 6, 7],
        rx_pool.allocator(),
    );
    runner.link_controller().set_link_up(true);
    let pipeline_observer = PipelineObserver::default();
    let mut sink = EmbassyNetConnectedRxSink::new(runner.rx_publisher(), Observer::default())
        .with_pipeline_observer(&pipeline_observer);
    assert_eq!(
        <EmbassyNetConnectedRxSink<'_, _, _> as ConnectedRxProtocolSink<
            FRAME_CAPACITY,
            QUEUE_DEPTH,
        >>::staged_rx_admission(&sink),
        StagedRxAdmission::AwaitCapacity
    );
    let ethernet = [0_u8; 14];
    let event = ConnectedRxEvent::Ethernet {
        frame: EthernetFrameParts {
            destination: [0; 6],
            source: [0; 6],
            ether_type: 0,
            payload: &[],
        },
        raw: &ethernet,
        amsdu: false,
        metadata: oer_ieee80211_softmac::MacRxMetadata::unavailable(),
    };

    sink.publish(event);
    sink.publish(event);

    assert_eq!(
        *pipeline_observer.observations.lock().unwrap(),
        [
            RxPipelineObservation::NetworkPublication {
                bytes: 14,
                micros: 1,
                outcome: RxNetworkPublicationOutcome::Enqueued,
            },
            RxPipelineObservation::NetworkPublication {
                bytes: 14,
                micros: 1,
                outcome: RxNetworkPublicationOutcome::PoolExhausted,
            },
        ]
    );
    assert_eq!(sink.observer().0, 2);
    let held = device.receive().unwrap();
    assert!(device.receive().is_none());
    sink.publish(event);
    assert_eq!(
        pipeline_observer.observations.lock().unwrap().last(),
        Some(&RxPipelineObservation::NetworkPublication {
            bytes: 14,
            micros: 1,
            outcome: RxNetworkPublicationOutcome::PoolExhausted,
        }),
        "an empty RX queue can still lack a packet-pool owner",
    );
    drop(held);

    let eapol = ConnectedRxEvent::Ethernet {
        frame: EthernetFrameParts {
            destination: [0; 6],
            source: [1; 6],
            ether_type: 0x888e,
            payload: &[1, 2, 3],
        },
        raw: &ethernet,
        amsdu: false,
        metadata: oer_ieee80211_softmac::MacRxMetadata::unavailable(),
    };
    sink.publish(eapol);
    assert_eq!(sink.observer().0, 4);
    assert!(device.receive().is_none());
}
