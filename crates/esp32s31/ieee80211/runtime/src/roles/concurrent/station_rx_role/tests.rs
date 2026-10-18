use super::*;

#[derive(Default)]
struct Observer(usize);

impl ConnectedRxSink for Observer {
    fn publish(&mut self, _event: ConnectedRxEvent<'_>) {
        self.0 += 1;
    }
}

struct Network {
    exhausted: bool,
    pool_policy: crate::datapath::network::RxPoolExhaustion,
    capacity: usize,
    ether_types: std::vec::Vec<u16>,
}

impl DatapathNetworkRx for Network {
    fn pool_exhaustion(&self) -> crate::datapath::network::RxPoolExhaustion {
        self.pool_policy
    }
    fn queue_len(&self) -> usize {
        self.ether_types.len()
    }

    fn try_send(&mut self, _frame: &[u8]) -> Result<(), RxEnqueueError> {
        unreachable!("station batch publishes structured Ethernet records")
    }

    fn try_send_parts(
        &mut self,
        frame: oer_ieee80211_mac::data::EthernetFrameParts<'_>,
    ) -> Result<(), RxEnqueueError> {
        if self.exhausted {
            return Err(RxEnqueueError::PoolExhausted);
        }
        if self.capacity == 0 {
            return Err(RxEnqueueError::QueueFull);
        }
        self.capacity -= 1;
        self.ether_types.push(frame.ether_type);
        Ok(())
    }

    fn poll_ready(&mut self, _context: &mut core::task::Context<'_>) -> core::task::Poll<()> {
        if self.capacity == 0 {
            core::task::Poll::Pending
        } else {
            core::task::Poll::Ready(())
        }
    }

    fn try_send_observed(
        &mut self,
        frame: &[u8],
        before_publish: &mut dyn FnMut(),
    ) -> Result<(), RxEnqueueError> {
        let result = self.try_send(frame);
        if result.is_ok() {
            before_publish();
        }
        result
    }

    fn try_send_parts_observed(
        &mut self,
        frame: oer_ieee80211_mac::data::EthernetFrameParts<'_>,
        before_publish: &mut dyn FnMut(),
    ) -> Result<(), RxEnqueueError> {
        let result = self.try_send_parts(frame);
        if result.is_ok() {
            before_publish();
        }
        result
    }
}

fn event(ether_type: u16) -> ConnectedRxEvent<'static> {
    ConnectedRxEvent::Ethernet {
        frame: oer_ieee80211_mac::data::EthernetFrameParts {
            destination: [1; 6],
            source: [2; 6],
            ether_type,
            payload: &[3, 4],
        },
        raw: &[0; 32],
        amsdu: false,
        metadata: oer_ieee80211_softmac::MacRxMetadata::unavailable(),
    }
}

#[test]
fn network_backpressure_retains_exact_station_batch_cursor() {
    let mut storage = [0_u8; VENDOR_LARGE_RX_PAYLOAD_CAPACITY];
    let mut sink = StaApStationRxSink::new(&mut storage, Observer::default());
    assert_eq!(
        <StaApStationRxSink<'_, _> as ConnectedRxProtocolSink<
            VENDOR_LARGE_RX_PAYLOAD_CAPACITY,
            VENDOR_LARGE_RX_SLOT_COUNT,
        >>::staged_rx_admission(&sink),
        StagedRxAdmission::AwaitCapacity,
        "paired STA ordinary frames must finish staging ownership before owned publication",
    );
    sink.publish(event(0x0800));
    sink.publish(event(0x0806));
    let mut network = Network {
        exhausted: false,
        pool_policy: crate::datapath::network::RxPoolExhaustion::WaitForRelease,
        capacity: 1,
        ether_types: std::vec::Vec::new(),
    };

    assert_eq!(
        sink.publish_pending(&mut network),
        Ok(DatapathRxProgress::NetworkBackpressured)
    );
    assert!(sink.has_pending());
    assert_eq!(network.ether_types, [0x0800]);

    network.capacity = 1;
    assert_eq!(
        sink.publish_pending(&mut network),
        Ok(DatapathRxProgress::Drained)
    );
    assert!(!sink.has_pending());
    assert_eq!(network.ether_types, [0x0800, 0x0806]);
    assert_eq!(sink.observer().0, 2);
}

#[test]
fn packet_pool_notification_contract_controls_retention() {
    use crate::datapath::network::RxPoolExhaustion;
    for policy in [
        RxPoolExhaustion::WaitForRelease,
        RxPoolExhaustion::DropFrame,
    ] {
        let mut storage = [0; VENDOR_LARGE_RX_PAYLOAD_CAPACITY];
        let mut sink = StaApStationRxSink::new(&mut storage, Observer::default());
        sink.publish(event(0x0800));
        sink.publish(event(0x0806));
        let mut network = Network {
            exhausted: true,
            pool_policy: policy,
            capacity: 2,
            ether_types: std::vec::Vec::new(),
        };
        let expected = if policy == RxPoolExhaustion::WaitForRelease {
            DatapathRxProgress::NetworkBackpressured
        } else {
            DatapathRxProgress::Drained
        };
        assert_eq!(sink.publish_pending(&mut network), Ok(expected));
        assert_eq!(
            sink.has_pending(),
            policy == RxPoolExhaustion::WaitForRelease
        );
        network.exhausted = false;
        assert_eq!(
            sink.publish_pending(&mut network),
            Ok(DatapathRxProgress::Drained)
        );
        assert_eq!(
            network.ether_types.len(),
            if policy == RxPoolExhaustion::WaitForRelease {
                2
            } else {
                0
            }
        );
    }
}
