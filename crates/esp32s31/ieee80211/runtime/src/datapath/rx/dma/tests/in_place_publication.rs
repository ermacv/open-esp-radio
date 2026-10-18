//! A staged data frame reaches the network in its own DMA buffer.

use crate::{
    datapath::rx::staging::{StagedEthernetPublication, StagedRxDisposition},
    roles::station::{network::EmbassyNetConnectedRxSink, rx_protocol::ConnectedRxProtocolSink},
};

use super::*;

const DESTINATION: [u8; 6] = [2, 3, 4, 5, 6, 7];
const SOURCE: [u8; 6] = [14, 15, 16, 17, 18, 19];
const PAYLOAD_OFFSET: usize = PUBLIC_HEADER_SIZE + 42;
const PAYLOAD: [u8; 4] = [1, 3, 5, 7];

#[derive(Default)]
struct InPlaceNetworkRx {
    admit: bool,
    admissions: usize,
    published: std::vec::Vec<u8>,
    copied: usize,
}

impl DatapathNetworkRx for InPlaceNetworkRx {
    fn queue_len(&self) -> usize {
        self.copied
    }

    fn try_send(&mut self, _frame: &[u8]) -> Result<(), RxEnqueueError> {
        self.copied += 1;
        Ok(())
    }

    fn try_send_parts(&mut self, _frame: EthernetFrameParts<'_>) -> Result<(), RxEnqueueError> {
        self.copied += 1;
        Ok(())
    }

    fn admit_in_place(&mut self) -> bool {
        self.admissions += 1;
        self.admit
    }

    fn publish_in_place(&mut self, index: u8) -> Result<(), RxEnqueueError> {
        self.published.push(index);
        Ok(())
    }

    fn poll_ready(&mut self, _context: &mut core::task::Context<'_>) -> core::task::Poll<()> {
        core::task::Poll::Ready(())
    }
}

#[derive(Default)]
struct EthernetPayloads(std::vec::Vec<(u16, std::vec::Vec<u8>)>);

impl ConnectedRxSink for EthernetPayloads {
    fn publish(&mut self, event: ConnectedRxEvent<'_>) {
        if let ConnectedRxEvent::Ethernet { frame, .. } = event {
            self.0.push((frame.ether_type, frame.payload.to_vec()));
        }
    }
}

fn publication(ether_type: u16) -> StagedEthernetPublication {
    StagedEthernetPublication {
        destination: DESTINATION,
        source: SOURCE,
        ether_type,
        payload_offset: PAYLOAD_OFFSET,
        payload_length: PAYLOAD.len(),
        metadata: oer_ieee80211_softmac::MacRxMetadata::unavailable(),
    }
}

#[test]
fn admitted_data_frame_is_published_in_its_dma_buffer_and_others_are_copied() {
    const COUNT: usize = 4;
    const RECEIVED: usize = PAYLOAD_OFFSET + PAYLOAD.len() + 4;
    const STAGED: usize = 3;
    const CAPACITY: usize = 192;

    let storage = Box::leak(Box::new(ReceiveDmaStorage::<COUNT>::new()));
    let mut addresses = [0_u32; COUNT];
    for (index, address) in addresses.iter_mut().enumerate() {
        *address = 0x2f00_2000 + index as u32 * 0x1200;
        storage.buffer_mut(index).unwrap()[PAYLOAD_OFFSET..PAYLOAD_OFFSET + PAYLOAD.len()]
            .copy_from_slice(&PAYLOAD);
    }
    let mut hardware = MockRxDma::default();
    let ring = RxRingStopped::prepare(
        &mut hardware,
        storage.descriptors(),
        BASE,
        &addresses,
        ESP32S31_RX_BUFFER_SIZE as u32,
        |_| Ok(()),
    )
    .unwrap()
    .try_start(&mut hardware)
    .map_err(|(_, error)| error)
    .unwrap();
    for descriptor in &storage.descriptors()[..STAGED] {
        descriptor.write_word0(
            ESP32S31_RX_BUFFER_SIZE as u32 | ((RECEIVED as u32) << LENGTH_SHIFT) | BIT_30 | BIT_31,
        );
    }
    hardware.release_through(STAGED - 1, Some(STAGED));
    let pool = RxStagePool::<STAGED, CAPACITY>::new();
    let queue = StagedRxQueue::<NoopRawMutex, STAGED, CAPACITY, STAGED>::new();
    let (sender, receiver) = queue.split();
    let mut producer = StagedRxProducer::new(ring, storage, &pool, NoDelay::new(), sender)
        .with_stage_admission_policy(UnreservedRxStageAdmission);
    embassy_futures::block_on(producer.service(&mut hardware)).unwrap();
    assert_eq!(pool.claimed_slots(), STAGED as u32);

    let network = InPlaceNetworkRx {
        admit: true,
        ..InPlaceNetworkRx::default()
    };
    let mut sink = EmbassyNetConnectedRxSink::new(network, EthernetPayloads::default());

    // Admitted IPv4: the 14 dead bytes before the payload become the
    // Ethernet header and the slot moves to the network untouched otherwise.
    let frame = receiver.try_receive().unwrap();
    let slot = frame.slot();
    assert_eq!(
        sink.publish_staged(frame, publication(0x0800)),
        StagedRxDisposition::RetainedByNetwork
    );
    assert_eq!(sink.network().published, [slot as u8]);
    assert_eq!(sink.network().copied, 0);
    // Published, not yet claimed: the slot stays out of radio reuse.
    assert_eq!(pool.network_slots(), 0);
    assert_eq!(pool.claimed_slots(), STAGED as u32);
    let lease = pool.external_handoff_pool().claim_network(slot as u8);
    assert_eq!(pool.network_slots(), 1);
    let mut expected = std::vec::Vec::new();
    expected.extend_from_slice(&DESTINATION);
    expected.extend_from_slice(&SOURCE);
    expected.extend_from_slice(&0x0800_u16.to_be_bytes());
    expected.extend_from_slice(&PAYLOAD);
    assert_eq!(lease.frame(), expected);
    lease.release();
    assert_eq!(pool.claimed_slots(), (STAGED - 1) as u32);

    // A refused admission falls back to the copying publisher.
    sink.network_mut().admit = false;
    let frame = receiver.try_receive().unwrap();
    assert_eq!(
        sink.publish_staged(frame, publication(0x0800)),
        StagedRxDisposition::Released
    );
    assert_eq!(sink.network().admissions, 2);
    assert_eq!(sink.network().copied, 1);

    // EAPOL stays with the control owner and never asks for admission.
    sink.network_mut().admit = true;
    let frame = receiver.try_receive().unwrap();
    assert_eq!(
        sink.publish_staged(frame, publication(0x888e)),
        StagedRxDisposition::Released
    );
    assert_eq!(sink.network().admissions, 2);
    assert_eq!(sink.network().copied, 1);
    assert_eq!(sink.network().published.len(), 1);
    assert_eq!(pool.claimed_slots(), 0);

    assert_eq!(
        sink.observer().0,
        [
            (0x0800, PAYLOAD.to_vec()),
            (0x0800, PAYLOAD.to_vec()),
            (0x888e, PAYLOAD.to_vec()),
        ]
    );
    producer
        .try_stop(&mut hardware)
        .unwrap_or_else(|_| panic!("test RX service must stop"));
}
