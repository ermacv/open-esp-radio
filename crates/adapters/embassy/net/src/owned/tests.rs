use super::*;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use xarxa_driver::{PacketPool, PacketPoolStorage};

extern crate std;
use self::std::boxed::Box;
use self::std::sync::Arc;
use self::std::sync::atomic::{AtomicUsize, Ordering as StdOrdering};
use self::std::task::Wake;

#[derive(Default)]
struct WakeCount(AtomicUsize);

impl Wake for WakeCount {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, StdOrdering::Relaxed);
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.fetch_add(1, StdOrdering::Relaxed);
    }
}

fn allocator<const N: usize>() -> PacketBufAllocator {
    let storage = Box::leak(Box::new(PacketPoolStorage::<N>::new()));
    Box::leak(Box::new(PacketPool::new(storage))).allocator()
}

fn frame(allocator: PacketBufAllocator, marker: u8) -> PacketBuf {
    let mut packet = allocator.try_alloc().unwrap();
    packet.set_len(ETHERNET_HEADER_LEN);
    packet.fill(marker);
    packet
}

#[test]
fn tx_owner_returns_to_its_origin_after_radio_completion() {
    let general = allocator::<1>();
    let rx = allocator::<1>();
    let resources = Box::leak(Box::new(OwnedEndpointResources::<NoopRawMutex, 1, 1>::new()));
    let (mut device, radio) = resources.split(NetworkInterfaceId::new(3), [2, 0, 0, 0, 0, 3], rx);
    radio.link_controller().set_link_up(true);

    device.transmit(frame(general, 0x51)).unwrap();
    assert!(general.try_alloc().is_none());
    let queued = radio.try_receive_tx().unwrap();
    assert_eq!(queued.interface(), NetworkInterfaceId::new(3));
    assert_eq!(queued.ethernet(), &[0x51; ETHERNET_HEADER_LEN]);
    drop(queued);
    assert!(general.try_alloc().is_some());
}

#[test]
fn rx_owner_survives_handoff_and_returns_to_rx_pool() {
    let rx = allocator::<1>();
    let resources = Box::leak(Box::new(OwnedEndpointResources::<NoopRawMutex, 1, 1>::new()));
    let (mut device, radio) = resources.split(NetworkInterfaceId::new(0), [2, 0, 0, 0, 0, 1], rx);
    radio.link_controller().set_link_up(true);

    radio
        .rx_publisher()
        .try_send_parts([0x72; 6], [0x73; 6], 0x0800, &[0x74; ETHERNET_HEADER_LEN])
        .unwrap();
    assert!(rx.try_alloc().is_none());
    let packet = device.receive().unwrap();
    assert_eq!(&packet[..6], &[0x72; 6]);
    assert_eq!(&packet[6..12], &[0x73; 6]);
    assert_eq!(&packet[12..14], &[0x08, 0x00]);
    assert_eq!(&packet[14..], &[0x74; ETHERNET_HEADER_LEN]);
    drop(packet);
    assert!(rx.try_alloc().is_some());
}

#[test]
fn rx_readiness_waits_for_both_link_and_pool_capacity() {
    let rx = allocator::<1>();
    let held = rx.try_alloc().unwrap();
    let resources = Box::leak(Box::new(OwnedEndpointResources::<NoopRawMutex, 1, 1>::new()));
    let (_device, radio) = resources.split(NetworkInterfaceId::new(0), [2, 0, 0, 0, 0, 1], rx);
    let link = radio.link_controller();
    let publisher = radio.rx_publisher();
    let wake_count = Arc::new(WakeCount::default());
    let waker = Waker::from(wake_count.clone());
    let mut context = Context::from_waker(&waker);

    assert_eq!(publisher.poll_ready(&mut context), Poll::Pending);
    link.set_link_up(true);
    let after_link = wake_count.0.load(StdOrdering::Relaxed);
    assert_ne!(after_link, 0);
    assert_eq!(publisher.poll_ready(&mut context), Poll::Pending);

    drop(held);
    assert!(wake_count.0.load(StdOrdering::Relaxed) > after_link);
    assert_eq!(publisher.poll_ready(&mut context), Poll::Ready(()));
}

#[test]
fn link_epoch_prevents_stale_tx_retargeting() {
    let general = allocator::<1>();
    let rx = allocator::<1>();
    let resources = Box::leak(Box::new(OwnedEndpointResources::<NoopRawMutex, 1, 1>::new()));
    let (mut device, radio) = resources.split(NetworkInterfaceId::new(0), [2, 0, 0, 0, 0, 1], rx);
    let link = radio.link_controller();
    link.set_link_up(true);
    device.transmit(frame(general, 0x33)).unwrap();

    link.set_link_up(false);
    link.set_link_up(true);
    assert!(radio.try_receive_tx().is_none());
    assert!(general.try_alloc().is_some());
}

#[test]
fn link_down_does_not_revoke_a_synchronous_tx_admission() {
    let general = allocator::<1>();
    let rx = allocator::<1>();
    let resources = Box::leak(Box::new(OwnedEndpointResources::<NoopRawMutex, 1, 1>::new()));
    let (mut device, radio) = resources.split(NetworkInterfaceId::new(0), [2, 0, 0, 0, 0, 1], rx);
    let link = radio.link_controller();
    link.set_link_up(true);

    assert!(device.can_transmit());
    link.set_link_up(false);
    assert!(device.transmit(frame(general, 0x44)).is_ok());

    // It belongs to the down lifetime and cannot cross the next up edge.
    link.set_link_up(true);
    assert!(radio.try_receive_tx().is_none());
    assert!(general.try_alloc().is_some());
}

#[test]
fn device_constructs_the_owned_embassy_stack() {
    let general = allocator::<8>();
    let rx = allocator::<1>();
    let endpoint = Box::leak(Box::new(OwnedEndpointResources::<NoopRawMutex, 1, 1>::new()));
    let (device, _radio) = endpoint.split(NetworkInterfaceId::new(0), [2, 0, 0, 0, 0, 1], rx);
    let storage = Box::leak(Box::new(owned_embassy_net::StackStorage::new()));

    let (stack, mut runner) = owned_embassy_net::Stack::new(storage, 0x1234, general);
    stack
        .add_iface_borrowed(Box::leak(Box::new(device)))
        .unwrap();
    runner.set_poll_budget(owned_embassy_net::PollBudget::new(4, 7));
}
