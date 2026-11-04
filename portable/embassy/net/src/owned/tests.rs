use super::*;
use crate::sink::Sink;
use oer_portable_memory::HostContext;
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

/// An endpoint with its link up over a device of `room` frames.
fn endpoint(
    room: usize,
) -> (
    OwnedNetworkDevice<'static, HostContext, 1, Sink>,
    OwnedNetworkRunner<'static, HostContext, 1>,
    Sink,
) {
    let resources = Box::leak(Box::new(OwnedEndpointResources::<HostContext, 1>::new()));
    let sink = Sink::with_room(room);
    let (device, mut radio) = resources
        .split(
            NetworkInterfaceId::new(0),
            [2; 6],
            allocator::<1>(),
            sink.clone(),
        )
        .expect("endpoint split");
    radio.link_controller().set_link_up(true);
    (device, radio, sink)
}

#[test]
fn a_transmitted_packet_goes_straight_to_the_device_and_returns_to_its_origin() {
    let general = allocator::<1>();
    let (mut device, _radio, sink) = endpoint(1);
    device.transmit(frame(general, 0x51)).unwrap();
    assert!(general.try_alloc().is_none());
    let taken = sink.0.borrow_mut().taken.pop_front().unwrap();
    assert_eq!(taken.ethernet(), &[0x51; ETHERNET_HEADER_LEN]);
    drop(taken);
    assert!(general.try_alloc().is_some());
}

#[test]
fn a_full_device_hands_the_packet_back_unchanged() {
    let general = allocator::<1>();
    let (mut device, _radio, _sink) = endpoint(0);
    let packet = device.transmit(frame(general, 0x52)).unwrap_err();
    assert_eq!(&*packet, &[0x52; ETHERNET_HEADER_LEN]);
}

#[test]
fn a_full_device_wakes_the_stack_once_it_has_room() {
    let (mut device, _radio, sink) = endpoint(0);
    let count = Arc::new(WakeCount::default());
    device.register_waker(&Waker::from(count.clone()));
    assert!(sink.0.borrow().waker.is_some());
    sink.0.borrow_mut().waker = None;
    // A later refusal arms again, without another stack poll.
    assert!(!device.can_transmit());
    sink.free(1);
    assert_eq!(count.0.load(StdOrdering::Relaxed), 1);
    assert!(device.can_transmit());
}

#[test]
fn a_device_with_room_registers_nothing_at_each_frame() {
    let (mut device, _radio, sink) = endpoint(1);
    assert!(device.can_transmit());
    assert!(sink.0.borrow().waker.is_none());
}

#[test]
fn a_replaced_stack_task_is_the_one_a_full_device_wakes() {
    let (mut device, _radio, sink) = endpoint(0);
    let first = Arc::new(WakeCount::default());
    let second = Arc::new(WakeCount::default());
    device.register_waker(&Waker::from(first.clone()));
    device.register_waker(&Waker::from(second.clone()));
    sink.0.borrow_mut().waker = None;
    assert!(!device.can_transmit());
    sink.free(1);
    assert_eq!(first.0.load(StdOrdering::Relaxed), 0);
    assert_eq!(second.0.load(StdOrdering::Relaxed), 1);
}

#[test]
fn rx_owner_survives_handoff_and_returns_to_rx_pool() {
    let rx = allocator::<1>();
    let resources = Box::leak(Box::new(OwnedEndpointResources::<HostContext, 1>::new()));
    let (mut device, mut radio) = resources
        .split(
            NetworkInterfaceId::new(0),
            [2, 0, 0, 0, 0, 1],
            rx,
            Sink::with_room(1),
        )
        .expect("endpoint split");
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
fn copied_rx_is_independent_of_source_storage_until_the_packet_drops() {
    let rx = allocator::<1>();
    let mut resources = OwnedEndpointResources::<HostContext, 1>::new();
    let (mut device, mut radio) = resources
        .split(NetworkInterfaceId::new(0), [2; 6], rx, Sink::with_room(1))
        .expect("endpoint split");
    radio.link_controller().set_link_up(true);
    let publisher = radio.rx_publisher();
    let mut source = [0x75; 60];

    publisher.try_send(&source).unwrap();
    source.fill(0x76);
    let packet = device.receive().unwrap();
    assert_eq!(&packet[..], &[0x75; 60]);
    assert_eq!(
        publisher.try_send(&source),
        Err(RxEnqueueError::PoolExhausted)
    );

    drop(packet);
    publisher.try_send(&source).unwrap();
    assert_eq!(&device.receive().unwrap()[..], &source);
}

#[test]
fn rx_readiness_waits_for_both_link_and_pool_capacity() {
    let rx = allocator::<1>();
    let held = rx.try_alloc().unwrap();
    let resources = Box::leak(Box::new(OwnedEndpointResources::<HostContext, 1>::new()));
    let (_device, mut radio) = resources
        .split(
            NetworkInterfaceId::new(0),
            [2, 0, 0, 0, 0, 1],
            rx,
            Sink::with_room(1),
        )
        .expect("endpoint split");
    let wake_count = Arc::new(WakeCount::default());
    let waker = Waker::from(wake_count.clone());
    let mut context = Context::from_waker(&waker);

    assert_eq!(radio.rx_publisher().poll_ready(&mut context), Poll::Pending);
    radio.link_controller().set_link_up(true);
    let publisher = radio.rx_publisher();
    let after_link = wake_count.0.load(StdOrdering::Relaxed);
    assert_ne!(after_link, 0);
    assert_eq!(publisher.poll_ready(&mut context), Poll::Pending);

    drop(held);
    assert!(wake_count.0.load(StdOrdering::Relaxed) > after_link);
    assert_eq!(publisher.poll_ready(&mut context), Poll::Ready(()));
}

#[test]
fn no_transmission_is_admitted_over_a_down_link() {
    let (mut device, mut radio, sink) = endpoint(1);
    radio.link_controller().set_link_up(false);
    assert!(!device.can_transmit());
    assert!(sink.0.borrow().waker.is_none());
}

#[test]
fn link_down_does_not_revoke_a_synchronous_tx_admission() {
    let general = allocator::<1>();
    let (mut device, mut radio, sink) = endpoint(1);
    assert!(device.can_transmit());
    radio.link_controller().set_link_up(false);
    sink.0.borrow_mut().refusal = Some(TransmitError::LinkDown);
    assert!(device.transmit(frame(general, 0x44)).is_ok());
    assert!(sink.0.borrow().taken.is_empty());
    assert!(general.try_alloc().is_some());
}

#[test]
fn a_frame_the_device_refuses_for_its_length_ends_here() {
    let general = allocator::<1>();
    let (mut device, _radio, sink) = endpoint(1);
    sink.0.borrow_mut().refusal = Some(TransmitError::InvalidLength(FrameLengthError::TooLong));
    assert!(device.transmit(frame(general, 0x45)).is_ok());
    assert!(general.try_alloc().is_some());
}

#[test]
fn device_constructs_the_owned_embassy_stack() {
    let general = allocator::<8>();
    let rx = allocator::<1>();
    let endpoint = Box::leak(Box::new(OwnedEndpointResources::<HostContext, 1>::new()));
    let (device, _radio) = endpoint
        .split(
            NetworkInterfaceId::new(0),
            [2, 0, 0, 0, 0, 1],
            rx,
            Sink::with_room(1),
        )
        .expect("endpoint split");
    let storage = Box::leak(Box::new(owned_embassy_net::StackStorage::new()));

    let (stack, mut runner) = owned_embassy_net::Stack::new(storage, 0x1234, general);
    stack
        .add_iface_borrowed(Box::leak(Box::new(device)))
        .unwrap();
    runner.set_poll_budget(owned_embassy_net::PollBudget::new(4, 7));
}

#[test]
fn a_second_split_returns_a_named_refusal() {
    let rx = allocator::<1>();
    let mut resources = OwnedEndpointResources::<HostContext, 1>::new();
    {
        let _owners = resources
            .split(NetworkInterfaceId::new(0), [2; 6], rx, Sink::with_room(1))
            .unwrap();
    }
    assert!(matches!(
        resources.split(NetworkInterfaceId::new(0), [2; 6], rx, Sink::with_room(1)),
        Err(EndpointSplitError::AlreadySplit)
    ));
}

#[test]
fn a_claimed_rx_waiter_refuses_split_without_consuming_the_resources() {
    let rx = allocator::<1>();
    let held = rx.try_claim_waiter().unwrap();
    let mut resources = OwnedEndpointResources::<HostContext, 1>::new();
    assert!(matches!(
        resources.split(NetworkInterfaceId::new(0), [2; 6], rx, Sink::with_room(1)),
        Err(EndpointSplitError::RxWaiterUnavailable)
    ));
    drop(held);
    let (mut device, mut radio) = resources
        .split(NetworkInterfaceId::new(0), [2; 6], rx, Sink::with_room(1))
        .unwrap();
    radio.link_controller().set_link_up(true);
    radio.try_send_rx(&[0x66; 14]).unwrap();
    assert_eq!(&*device.receive().unwrap(), &[0x66; 14]);
}

#[test]
fn rx_copy_checks_lengths_and_preserves_all_bytes_at_pool_capacity() {
    let rx = allocator::<1>();
    let capacity = rx.try_alloc().unwrap().capacity();
    let mut resources = OwnedEndpointResources::<HostContext, 1>::new();
    let (mut device, mut radio) = resources
        .split(NetworkInterfaceId::new(0), [2; 6], rx, Sink::with_room(1))
        .unwrap();
    radio.link_controller().set_link_up(true);
    let publisher = radio.rx_publisher();
    assert_eq!(
        publisher.try_send(&[0; 13]),
        Err(RxEnqueueError::InvalidLength(FrameLengthError::TooShort))
    );
    let mut bytes = std::vec![0x67; capacity + 1];
    assert_eq!(
        publisher.try_send(&bytes),
        Err(RxEnqueueError::InvalidLength(FrameLengthError::TooLong))
    );
    assert_eq!(
        publisher.try_send_parts([1; 6], [2; 6], 0x0806, &bytes),
        Err(RxEnqueueError::InvalidLength(FrameLengthError::TooLong))
    );
    bytes.pop();
    publisher.try_send(&bytes).unwrap();
    assert_eq!(&*device.receive().unwrap(), &bytes);
    let payload = &bytes[ETHERNET_HEADER_LEN..];
    publisher
        .try_send_parts([1; 6], [2; 6], 0x0806, payload)
        .unwrap();
    let packet = device.receive().unwrap();
    assert_eq!(&packet[..6], &[1; 6]);
    assert_eq!(&packet[6..12], &[2; 6]);
    assert_eq!(&packet[12..14], &[8, 6]);
    assert_eq!(&packet[14..], payload);
}
