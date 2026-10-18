//! The real protocol retains DMA ownership while the separate RX pool is full.

use core::task::{Context, Poll, Waker};

use crate::roles::station::network::EmbassyNetConnectedRxSink;

use oer_embassy_net_owned::{NetworkInterfaceId, OwnedEndpointResources};

use std::{sync::Arc, task::Wake};

use super::*;

use xarxa_driver::{PacketPool, PacketPoolStorage};

#[derive(Default)]
struct WakeCounter(AtomicUsize);

impl Wake for WakeCounter {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn occupied_network_pool_retains_staging_owner_until_credit_return() {
    const COUNT: usize = 2;
    const CAPACITY: usize = 192;
    const MPDU: usize = 26 + 8 + 8 + 4 + 8;
    const SIGNAL: usize = MPDU + 4;
    const RECEIVED: usize = PUBLIC_HEADER_SIZE + SIGNAL;
    const PAYLOAD: [u8; 4] = [1, 3, 5, 7];

    let storage = Box::leak(Box::new(ReceiveDmaStorage::<COUNT>::new()));
    let buffer = storage.buffer_mut(0).unwrap();
    buffer[0x38..0x3c]
        .copy_from_slice(&(((SIGNAL + 4) as u32) << 16 | SIGNAL as u32).to_le_bytes());
    let frame = &mut buffer[PUBLIC_HEADER_SIZE..PUBLIC_HEADER_SIZE + MPDU];
    frame[..2].copy_from_slice(&0x4288_u16.to_le_bytes());
    frame[4..10].copy_from_slice(&dispatcher_config().station_address);
    frame[10..16].copy_from_slice(&dispatcher_config().bssid);
    frame[16..22].copy_from_slice(&[14, 15, 16, 17, 18, 19]);
    frame[22..24].copy_from_slice(&16_u16.to_le_bytes());
    frame[26..34].copy_from_slice(&[1, 0, 0, 0x20, 0, 0, 0, 0]);
    frame[34..42].copy_from_slice(&[0xaa, 0xaa, 0x03, 0, 0, 0, 0x08, 0x00]);
    frame[42..46].copy_from_slice(&PAYLOAD);

    let addresses = [0x2f00_2000, 0x2f00_3200];
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
    storage.descriptors()[0].write_word0(
        ESP32S31_RX_BUFFER_SIZE as u32 | ((RECEIVED as u32) << LENGTH_SHIFT) | BIT_30 | BIT_31,
    );
    hardware.release_through(COUNT - 1, None);
    let pool = RxStagePool::<1, CAPACITY>::new();
    let queue = StagedRxQueue::<NoopRawMutex, 1, CAPACITY, 1>::new();
    let (sender, receiver) = queue.split();
    let mut producer = StagedRxProducer::new(ring, storage, &pool, NoDelay::new(), sender)
        .with_stage_admission_policy(UnreservedRxStageAdmission);
    embassy_futures::block_on(producer.service(&mut hardware)).unwrap();

    let resources = Box::leak(Box::new(OwnedEndpointResources::<NoopRawMutex, 1, 1>::new()));
    let packets = Box::leak(Box::new(PacketPoolStorage::<1>::new()));
    let packets = Box::leak(Box::new(PacketPool::new(packets)));
    let held_packet = packets.allocator().try_alloc().unwrap();
    let (mut device, network) = resources.split(
        NetworkInterfaceId::new(0),
        dispatcher_config().station_address,
        packets.allocator(),
    );
    network.link_controller().set_link_up(true);
    let sink = EmbassyNetConnectedRxSink::new(network.rx_publisher(), Observer::default());
    let irq = EmbassyMacIrqRuntime::<NoopRawMutex>::new();
    let mut mpdu = [0; CAPACITY];
    let mut ethernet = [0; CAPACITY];
    let runtime = Box::leak(Box::new(ConnectedReceiveStorage::new()));
    configure_dispatcher(runtime);
    let mut protocol = ConnectedReceiveProtocol::new(
        receiver,
        &irq,
        sink,
        &mut mpdu,
        &mut ethernet,
        runtime,
        oer_time_virtual::SkipClock::new(),
    );
    let wake = Arc::new(WakeCounter::default());
    let waker = Waker::from(Arc::clone(&wake));
    let mut context = Context::from_waker(&waker);
    {
        let mut turn = core::pin::pin!(protocol.service_bounded(1));
        assert!(turn.as_mut().poll(&mut context).is_pending());
        assert_eq!(pool.claimed_slots(), 1);
        assert_eq!(storage.detached_buffer_count(), 1);
        assert_eq!(storage.released_buffer_count(), 0);
        assert!(device.receive().is_none());
        let wakes_before = wake.0.load(Ordering::Relaxed);
        drop(held_packet);
        assert!(wake.0.load(Ordering::Relaxed) > wakes_before);
        let Poll::Ready(result) = turn.as_mut().poll(&mut context) else {
            panic!("returning the output credit completes the retained frame");
        };
        assert_eq!(result.consumed_frames, 1);
        #[cfg(feature = "core0-rx-coarse-telemetry")]
        {
            assert_eq!(result.direct_frames.value(), 0);
            assert_eq!(result.asynchronous_frames.value(), 1);
        }
    }
    assert_eq!(pool.claimed_slots(), 0);
    assert_eq!(storage.released_buffer_count(), 1);
    let packet = device
        .receive()
        .expect("one output owner after credit return");
    assert_eq!(&packet[14..], &PAYLOAD);
    assert!(device.receive().is_none());
    drop(packet);
    let stopped = protocol.into_stopped();
    assert_eq!(stopped.shutdown().queued_frames, 0);
    embassy_futures::block_on(producer.service(&mut hardware)).unwrap();
    assert_eq!(storage.detached_buffer_count(), 0);
    producer
        .try_stop(&mut hardware)
        .unwrap_or_else(|_| panic!("all RX owners returned"));
}
