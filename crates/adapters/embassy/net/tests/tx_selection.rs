use oer_embassy_net_owned::{NetworkInterfaceId, NoopRawMutex, OwnedEndpointResources};
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

fn packet(pool: PacketBufAllocator, destination: u8, sequence: u8) -> PacketBuf {
    let mut packet = pool.try_alloc().unwrap();
    packet.set_len(15);
    packet.fill(0);
    packet[..6].fill(destination);
    packet[14] = sequence;
    packet
}

#[derive(Default)]
struct Wakes(AtomicUsize);
impl Wake for Wakes {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn sparse_destination_is_directly_available_behind_a_large_bulk_backlog() {
    let pool = allocator::<128>();
    let resources = Box::leak(Box::new(
        OwnedEndpointResources::<NoopRawMutex, 1, 128>::new(),
    ));
    let (mut device, radio) = resources.split(NetworkInterfaceId::new(0), [2; 6], allocator::<1>());
    radio.link_controller().set_link_up(true);
    for n in 0..127 {
        device.transmit(packet(pool, 2, n)).unwrap();
    }
    device.transmit(packet(pool, 4, 200)).unwrap();
    assert!(!device.can_transmit());
    let sparse = radio.try_take_for([4; 6]).unwrap();
    assert_eq!(sparse.ethernet()[14], 200);
    assert_eq!(radio.pending_for([2; 6]), 127);
    assert!(radio.try_take_for([4; 6]).is_none());
    assert!(
        pool.try_alloc().is_none(),
        "selection retains its exact packet owner"
    );
    drop(sparse);
    for n in 0..127 {
        let bulk = radio.try_take_for([2; 6]).unwrap();
        assert_eq!(bulk.ethernet()[14], n);
    }
    assert_eq!(radio.tx_queue_len(), 0);
}

#[test]
fn selected_wait_ignores_other_backlog_and_wakes_for_matching_publication() {
    let pool = allocator::<4>();
    let resources = Box::leak(Box::new(OwnedEndpointResources::<NoopRawMutex, 1, 4>::new()));
    let (mut device, radio) = resources.split(NetworkInterfaceId::new(0), [2; 6], allocator::<1>());
    radio.link_controller().set_link_up(true);
    device.transmit(packet(pool, 2, 1)).unwrap();
    let wakes = Arc::new(Wakes::default());
    let waker = Waker::from(wakes.clone());
    let mut context = Context::from_waker(&waker);
    assert_eq!(radio.poll_ready_for([4; 6], 1, &mut context), Poll::Pending);
    assert_eq!(wakes.0.load(Ordering::Relaxed), 0);
    device.transmit(packet(pool, 2, 3)).unwrap();
    assert_eq!(
        wakes.0.load(Ordering::Relaxed),
        0,
        "another destination cannot satisfy this wait"
    );
    device.transmit(packet(pool, 4, 2)).unwrap();
    assert_eq!(wakes.0.load(Ordering::Relaxed), 1);
    assert_eq!(
        radio.poll_ready_for([4; 6], 1, &mut context),
        Poll::Ready(())
    );
    drop(radio.try_take_for([4; 6]));
    assert_eq!(radio.poll_ready_for([4; 6], 1, &mut context), Poll::Pending);
    assert_eq!(radio.pending_for([2; 6]), 2);
}

#[test]
fn selected_dequeue_cannot_retarget_owners_across_link_epochs() {
    let pool = allocator::<4>();
    let resources = Box::leak(Box::new(OwnedEndpointResources::<NoopRawMutex, 1, 4>::new()));
    let (mut device, radio) = resources.split(NetworkInterfaceId::new(0), [2; 6], allocator::<1>());
    let link = radio.link_controller();
    link.set_link_up(true);
    device.transmit(packet(pool, 2, 1)).unwrap();
    device.transmit(packet(pool, 4, 2)).unwrap();
    let old_head = radio.head_for([4; 6]).unwrap();
    assert_eq!(old_head.ethernet_bytes, 15);
    link.set_link_up(false);
    link.set_link_up(true);
    let mut replacement = packet(pool, 4, 3);
    replacement.set_len(60);
    replacement[15..].fill(0);
    device.transmit(replacement).unwrap();
    let claimed = radio.try_take_for([4; 6]).unwrap();
    assert_eq!(claimed.ethernet()[14], 3);
    assert_eq!(claimed.ethernet().len(), 60);
    assert_ne!(
        claimed.ethernet().len(),
        old_head.ethernet_bytes,
        "a demand snapshot cannot authorize geometry across a link epoch"
    );
    assert!(radio.try_take_for([2; 6]).is_none());
    assert_eq!(radio.tx_queue_len(), 0);
}

#[test]
fn an_any_destination_wait_wakes_for_the_first_publication_to_any_destination() {
    let pool = allocator::<4>();
    let resources = Box::leak(Box::new(OwnedEndpointResources::<NoopRawMutex, 1, 4>::new()));
    let (mut device, radio) = resources.split(NetworkInterfaceId::new(0), [2; 6], allocator::<1>());
    radio.link_controller().set_link_up(true);
    let wakes = Arc::new(Wakes::default());
    let waker = Waker::from(wakes.clone());
    let mut context = Context::from_waker(&waker);
    // Empty: the wait registers.
    assert_eq!(radio.poll_ready_any(&mut context), Poll::Pending);
    device.transmit(packet(pool, 9, 1)).unwrap();
    assert_eq!(wakes.0.load(Ordering::Relaxed), 1);
    assert_eq!(radio.poll_ready_any(&mut context), Poll::Ready(()));
    // A ready check consumes the registration: the next publication does not
    // wake a radio that is not waiting.
    device.transmit(packet(pool, 4, 2)).unwrap();
    assert_eq!(wakes.0.load(Ordering::Relaxed), 1);
    drop(radio.try_take_for([9; 6]));
    drop(radio.try_take_for([4; 6]));
    assert_eq!(radio.poll_ready_any(&mut context), Poll::Pending);
}
