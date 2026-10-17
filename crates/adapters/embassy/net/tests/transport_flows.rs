//! Real packet owners through publication, nested selection and readiness.

use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use oer_embassy_net_owned::{NetworkInterfaceId, OwnedEndpointResources};
use oer_ieee80211_datapath::DestinationTxQueues;
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    task::{Context, Poll, Wake, Waker},
};
use xarxa_driver::{PacketBuf, PacketBufAllocator, PacketPool, PacketPoolStorage};

fn allocator<const N: usize>() -> PacketBufAllocator {
    let storage = Box::leak(Box::new(PacketPoolStorage::<N>::new()));
    Box::leak(Box::new(PacketPool::new(storage))).allocator()
}

fn udp(pool: PacketBufAllocator, destination: u8, port: u16, sequence: u8) -> PacketBuf {
    let mut packet = pool.try_alloc().unwrap();
    packet.set_len(43);
    packet.fill(0);
    packet[..6].fill(destination);
    packet[12..14].copy_from_slice(&[8, 0]);
    packet[14] = 0x45;
    packet[16..18].copy_from_slice(&29_u16.to_be_bytes());
    packet[23] = 17;
    packet[26..34].copy_from_slice(&[10, 0, 0, 1, 10, 0, 0, 2]);
    packet[34..36].copy_from_slice(&1000_u16.to_be_bytes());
    packet[36..38].copy_from_slice(&port.to_be_bytes());
    packet[38..40].copy_from_slice(&9_u16.to_be_bytes());
    packet[42] = sequence;
    packet
}

#[test]
fn sparse_transport_flow_shares_the_peer_burst_without_draining_bulk() {
    let pool = allocator::<16>();
    let mut resources = OwnedEndpointResources::<NoopRawMutex, 1, 16>::new();
    let (mut device, radio) = resources.split(NetworkInterfaceId::new(0), [2; 6], allocator::<1>());
    radio.link_controller().set_link_up(true);
    for n in 0..14 {
        device.transmit(udp(pool, 4, 100, n)).unwrap();
    }
    let sparse = udp(pool, 4, 200, 99);
    let sparse_address = sparse.as_ptr();
    device.transmit(sparse).unwrap();
    device.transmit(udp(pool, 6, 100, 100)).unwrap();
    let first = radio.try_take_for([4; 6]).unwrap();
    assert_eq!(first.ethernet()[42], 0);
    let sparse = radio.try_take_for([4; 6]).unwrap();
    assert_eq!(sparse.ethernet()[42], 99);
    assert_eq!(sparse.ethernet().as_ptr(), sparse_address);
    assert_eq!(
        radio.pending_for([4; 6]),
        13,
        "other flows still fill the selected peer's burst"
    );
    assert_eq!(radio.pending_for([6; 6]), 1);
    assert!(!device.can_transmit(), "selected owners retain admission");
    drop((first, sparse));
    for n in 1..14 {
        assert_eq!(radio.try_take_for([4; 6]).unwrap().ethernet()[42], n);
    }
    assert_eq!(radio.try_take_for([6; 6]).unwrap().ethernet()[42], 100);
}

#[test]
fn outer_destination_turns_are_independent_of_transport_flow_count() {
    let pool = allocator::<8>();
    let mut resources = OwnedEndpointResources::<NoopRawMutex, 1, 8>::new();
    let (mut device, radio) = resources.split(NetworkInterfaceId::new(0), [2; 6], allocator::<1>());
    radio.link_controller().set_link_up(true);
    // Insert the larger address first: public selection must not expose the
    // adapter's allocation order to a scheduler merging several sources.
    device.transmit(udp(pool, 6, 1, 99)).unwrap();
    for port in 1..7 {
        device.transmit(udp(pool, 4, port, port as u8)).unwrap();
    }
    let first = radio.next_head_after(None).unwrap().0;
    assert_eq!(first, [4; 6]);
    drop(radio.try_take_for([4; 6]));
    drop(radio.try_take_for([4; 6]));
    let second = radio.next_head_after(Some(first)).unwrap().0;
    assert_eq!(second, [6; 6]);
    assert_eq!(radio.next_head_after(Some(second)).unwrap().0, [4; 6]);
    while let Some(owner) = radio.try_receive_tx() {
        drop(owner);
    }
    assert!(radio.next_head_after(Some(second)).is_none());
    device.transmit(udp(pool, 6, 1, 99)).unwrap();
    device.transmit(udp(pool, 2, 1, 98)).unwrap();
    for _ in 0..3 {
        let (destination, head) = radio.next_head_after(Some([6; 6])).unwrap();
        assert_eq!(destination, [2; 6]);
        assert_eq!((head.ethernet_bytes, head.pending_frames), (43, 1));
    }
    assert_eq!(radio.try_take_for([2; 6]).unwrap().ethernet()[42], 98);
    drop(radio.try_take_for([6; 6]));
}

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
fn aggregate_readiness_sums_flows_without_waking_for_another_destination() {
    let pool = allocator::<4>();
    let mut resources = OwnedEndpointResources::<NoopRawMutex, 1, 4>::new();
    let (mut device, radio) = resources.split(NetworkInterfaceId::new(0), [2; 6], allocator::<1>());
    radio.link_controller().set_link_up(true);
    let wakes = Arc::new(Wakes::default());
    let waker = Waker::from(wakes.clone());
    let mut cx = Context::from_waker(&waker);
    assert_eq!(radio.poll_ready_for([4; 6], 3, &mut cx), Poll::Pending);
    device.transmit(udp(pool, 4, 1, 0)).unwrap();
    device.transmit(udp(pool, 6, 1, 0)).unwrap();
    device.transmit(udp(pool, 4, 2, 0)).unwrap();
    for _ in 0..4 {
        let head = radio.head_for([4; 6]).unwrap();
        assert_eq!((head.ethernet_bytes, head.pending_frames), (43, 2));
        assert!(radio.head_for([8; 6]).is_none());
    }
    assert_eq!(wakes.0.load(Ordering::Relaxed), 0);
    device.transmit(udp(pool, 4, 3, 0)).unwrap();
    assert_eq!(wakes.0.load(Ordering::Relaxed), 1);
    assert_eq!(radio.poll_ready_for([4; 6], 3, &mut cx), Poll::Ready(()));
    while let Some(owner) = radio.try_receive_tx() {
        drop(owner);
    }
}

#[test]
fn head_inspection_preserves_flow_rotation_packet_owners_and_admission() {
    let pool = allocator::<4>();
    let mut resources = OwnedEndpointResources::<NoopRawMutex, 1, 4>::new();
    let (mut device, radio) = resources.split(NetworkInterfaceId::new(0), [2; 6], allocator::<1>());
    radio.link_controller().set_link_up(true);
    let mut addresses = Vec::new();
    for (port, length, sequence) in [(1, 100, 1), (1, 200, 2), (2, 800, 3)] {
        let mut packet = udp(pool, 4, port, sequence);
        packet.set_len(length);
        packet[43..].fill(0);
        addresses.push(packet.as_ptr());
        device.transmit(packet).unwrap();
    }
    device.transmit(udp(pool, 6, 1, 4)).unwrap();
    let wakes = Arc::new(Wakes::default());
    device.register_waker(&Waker::from(wakes.clone()));
    let mut claimed = Vec::new();
    for (length, sequence, address_index, pending) in
        [(100, 1, 0, 3), (800, 3, 2, 2), (200, 2, 1, 1)]
    {
        for _ in 0..4 {
            // Exercise the object-safe boundary used by the radio, not an
            // adapter-internal shortcut around its synchronization.
            let queues: &dyn DestinationTxQueues<Frame = _> = &radio;
            let head = queues.head_for([4; 6]).unwrap();
            assert_eq!(
                (head.ethernet_bytes, head.pending_frames),
                (length, pending)
            );
            assert_eq!(queues.head_for([6; 6]).unwrap().pending_frames, 1);
            assert!(!device.can_transmit());
            assert!(pool.try_alloc().is_none());
            assert_eq!(wakes.0.load(Ordering::Relaxed), 0);
        }
        let frame = radio.try_take_for([4; 6]).unwrap();
        assert_eq!(frame.ethernet().len(), length);
        assert_eq!(frame.ethernet()[42], sequence);
        assert_eq!(frame.ethernet().as_ptr(), addresses[address_index]);
        claimed.push(frame);
    }
    assert!(radio.head_for([4; 6]).is_none());
    assert!(
        !device.can_transmit(),
        "dequeued owners still occupy admission"
    );
    drop(claimed.pop());
    assert!(device.can_transmit());
    assert_eq!(wakes.0.load(Ordering::Relaxed), 1);
    drop(claimed);
    drop(radio.try_take_for([6; 6]));
}
