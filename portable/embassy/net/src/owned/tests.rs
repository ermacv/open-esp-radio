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
    let (mut device, radio) = resources
        .split(NetworkInterfaceId::new(3), [2, 0, 0, 0, 0, 3], rx)
        .expect("endpoint split");
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
    let (mut device, radio) = resources
        .split(NetworkInterfaceId::new(0), [2, 0, 0, 0, 0, 1], rx)
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
    let mut resources = OwnedEndpointResources::<NoopRawMutex, 1, 1>::new();
    let (mut device, radio) = resources
        .split(NetworkInterfaceId::new(0), [2; 6], rx)
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
    let resources = Box::leak(Box::new(OwnedEndpointResources::<NoopRawMutex, 1, 1>::new()));
    let (_device, radio) = resources
        .split(NetworkInterfaceId::new(0), [2, 0, 0, 0, 0, 1], rx)
        .expect("endpoint split");
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
    let (mut device, radio) = resources
        .split(NetworkInterfaceId::new(0), [2, 0, 0, 0, 0, 1], rx)
        .expect("endpoint split");
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
    let (mut device, radio) = resources
        .split(NetworkInterfaceId::new(0), [2, 0, 0, 0, 0, 1], rx)
        .expect("endpoint split");
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
    let (device, _radio) = endpoint
        .split(NetworkInterfaceId::new(0), [2, 0, 0, 0, 0, 1], rx)
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
    let mut resources = OwnedEndpointResources::<NoopRawMutex, 1, 1>::new();
    {
        let _owners = resources
            .split(NetworkInterfaceId::new(0), [2; 6], rx)
            .unwrap();
    }
    assert!(matches!(
        resources.split(NetworkInterfaceId::new(0), [2; 6], rx),
        Err(EndpointSplitError::AlreadySplit)
    ));
}

#[test]
fn a_claimed_rx_waiter_refuses_split_without_consuming_the_resources() {
    let rx = allocator::<1>();
    let held = rx.try_claim_waiter().unwrap();
    let mut resources = OwnedEndpointResources::<NoopRawMutex, 1, 1>::new();
    assert!(matches!(
        resources.split(NetworkInterfaceId::new(0), [2; 6], rx),
        Err(EndpointSplitError::RxWaiterUnavailable)
    ));
    drop(held);
    let (mut device, radio) = resources
        .split(NetworkInterfaceId::new(0), [2; 6], rx)
        .unwrap();
    radio.link_controller().set_link_up(true);
    radio.try_send_rx(&[0x66; 14]).unwrap();
    assert_eq!(&*device.receive().unwrap(), &[0x66; 14]);
}

#[test]
fn rx_copy_checks_lengths_and_preserves_all_bytes_at_pool_capacity() {
    let rx = allocator::<1>();
    let capacity = rx.try_alloc().unwrap().capacity();
    let mut resources = OwnedEndpointResources::<NoopRawMutex, 1, 1>::new();
    let (mut device, radio) = resources
        .split(NetworkInterfaceId::new(0), [2; 6], rx)
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

type ReentrantQueue =
    tx_queue::TxQueue<embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex, 1>;

struct DrainOnWake {
    queue: Arc<ReentrantQueue>,
    calls: AtomicUsize,
}

impl Wake for DrainOnWake {
    fn wake(self: Arc<Self>) {
        // Publication and the selected destination must be visible before the
        // callback, which may synchronously drain and return the affine credit.
        assert_eq!(self.queue.len(), 1);
        assert!(self.queue.is_full());
        assert_eq!(self.queue.head_for([0x68; 6]).unwrap().pending_frames, 1);
        let (queued, credit) = self.queue.pop(None).unwrap();
        assert_eq!(&*queued.packet, &[0x68; 14]);
        drop(queued);
        drop(credit);
        assert!(!self.queue.is_full());
        self.calls.fetch_add(1, StdOrdering::Relaxed);
    }
}

#[test]
fn tx_publication_wakes_after_restoring_owners_and_metadata() {
    let pool = allocator::<1>();
    let queue = Arc::new(ReentrantQueue::new());
    let callback = Arc::new(DrainOnWake {
        queue: queue.clone(),
        calls: AtomicUsize::new(0),
    });
    let waker = Waker::from(callback.clone());
    assert_eq!(
        queue.poll_ready_any(&mut Context::from_waker(&waker)),
        Poll::Pending
    );
    assert!(
        queue
            .push(QueuedPacket {
                epoch: 0,
                packet: frame(pool, 0x68)
            })
            .is_ok()
    );
    assert_eq!(callback.calls.load(StdOrdering::Relaxed), 1);
    assert!(queue.is_empty());
    assert!(pool.try_alloc().is_some());
}

struct RefillOnWake {
    queue: Arc<ReentrantQueue>,
    pool: PacketBufAllocator,
    calls: AtomicUsize,
}

impl Wake for RefillOnWake {
    fn wake(self: Arc<Self>) {
        assert!(!self.queue.is_full());
        // Packet destruction must precede the credit wake, so even a one-slot
        // pool is available for immediate reuse in this callback.
        assert!(
            self.queue
                .push(QueuedPacket {
                    epoch: 0,
                    packet: frame(self.pool, 0x69),
                })
                .is_ok()
        );
        self.calls.fetch_add(1, StdOrdering::Relaxed);
    }
}

#[test]
fn returning_tx_credit_can_synchronously_refill_the_queue() {
    let pool = allocator::<1>();
    let queue = Arc::new(ReentrantQueue::new());
    let callback = Arc::new(RefillOnWake {
        queue: queue.clone(),
        pool,
        calls: AtomicUsize::new(0),
    });
    queue.register_sender(&Waker::from(callback.clone()));
    assert!(
        queue
            .push(QueuedPacket {
                epoch: 0,
                packet: frame(pool, 0x68)
            })
            .is_ok()
    );
    let (packet, credit) = queue.pop(None).unwrap();
    drop(packet);
    drop(credit);
    assert_eq!(callback.calls.load(StdOrdering::Relaxed), 1);
    let (packet, credit) = queue.pop(None).unwrap();
    assert_eq!(&*packet.packet, &[0x69; 14]);
    drop(packet);
    drop(credit);
    assert!(!queue.is_full());
    assert!(pool.try_alloc().is_some());
}

#[test]
fn replacing_a_tx_waiter_restores_state_before_waking_the_previous_task() {
    let pool = allocator::<1>();
    let queue = Arc::new(ReentrantQueue::new());
    assert!(
        queue
            .push(QueuedPacket {
                epoch: 0,
                packet: frame(pool, 0x68)
            })
            .is_ok()
    );
    let callback = Arc::new(DrainOnWake {
        queue: queue.clone(),
        calls: AtomicUsize::new(0),
    });
    let previous = Waker::from(callback.clone());
    let next = Waker::from(Arc::new(WakeCount::default()));
    assert_eq!(
        queue.poll_ready_for([0x70; 6], 1, &mut Context::from_waker(&previous)),
        Poll::Pending
    );
    assert_eq!(
        queue.poll_ready_for([0x70; 6], 1, &mut Context::from_waker(&next)),
        Poll::Pending
    );
    assert_eq!(callback.calls.load(StdOrdering::Relaxed), 1);
    assert!(queue.is_empty());
    assert!(!queue.is_full());
}
