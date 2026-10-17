//! Admission follows the software owner, including time retained by the radio.

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use oer_embassy_net_owned::{NetworkInterfaceId, OwnedEndpointResources};
use oer_ieee80211_datapath::DestinationTxQueues;
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    task::{Wake, Waker},
};
use xarxa_driver::{PacketBuf, PacketBufAllocator, PacketPool, PacketPoolStorage};

fn allocator<const N: usize>() -> PacketBufAllocator {
    let storage = Box::leak(Box::new(PacketPoolStorage::<N>::new()));
    Box::leak(Box::new(PacketPool::new(storage))).allocator()
}

fn packet(pool: PacketBufAllocator, destination: u8) -> PacketBuf {
    let mut packet = pool.try_alloc().unwrap();
    packet.set_len(14);
    packet.fill(destination);
    packet
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
fn retained_frames_share_admission_with_queued_frames() {
    let pool = allocator::<4>();
    // Resources deliberately remain borrowed, rather than requiring static placement.
    let mut resources = OwnedEndpointResources::<CriticalSectionRawMutex, 1, 2>::new();
    let (mut device, radio) = resources.split(NetworkInterfaceId::new(0), [2; 6], allocator::<1>());
    radio.link_controller().set_link_up(true);
    let wakes = Arc::new(Wakes::default());
    device.register_waker(&Waker::from(wakes.clone()));
    device.transmit(packet(pool, 2)).unwrap();
    device.transmit(packet(pool, 4)).unwrap();
    let retained = radio.try_take_for([2; 6]).unwrap();
    assert_eq!(radio.tx_queue_len(), 1);
    assert!(
        !device.can_transmit(),
        "dequeue must not admit a second owner for the same credit"
    );
    assert_eq!(wakes.0.load(Ordering::Relaxed), 0);
    let rejected = packet(pool, 6);
    let address = rejected.as_ptr();
    let rejected = device
        .transmit(rejected)
        .expect_err("radio retention consumes admission");
    assert_eq!(rejected.as_ptr(), address);
    drop(retained);
    assert!(device.can_transmit());
    assert_eq!(wakes.0.load(Ordering::Relaxed), 1);
    device.transmit(rejected).unwrap();
    assert!(!device.can_transmit());
    drop(radio.try_receive_tx());
    drop(radio.try_receive_tx());
    assert!(device.can_transmit());
}

#[test]
fn empty_ingress_remains_full_until_radio_owner_is_released_on_another_thread() {
    let pool = allocator::<2>();
    let mut resources = OwnedEndpointResources::<CriticalSectionRawMutex, 1, 1>::new();
    let (mut device, radio) = resources.split(NetworkInterfaceId::new(0), [2; 6], allocator::<1>());
    radio.link_controller().set_link_up(true);
    device.transmit(packet(pool, 2)).unwrap();
    let retained = radio.try_receive_tx().unwrap();
    assert_eq!(radio.tx_queue_len(), 0);
    assert!(!device.can_transmit());
    let wakes = Arc::new(Wakes::default());
    device.register_waker(&Waker::from(wakes.clone()));
    std::thread::scope(|scope| {
        scope.spawn(move || drop(retained)).join().unwrap();
    });
    assert_eq!(wakes.0.load(Ordering::Relaxed), 1);
    assert!(device.can_transmit());
}

#[test]
fn stale_queue_disposal_returns_credit_without_revoking_retained_owners() {
    let pool = allocator::<3>();
    let mut resources = OwnedEndpointResources::<CriticalSectionRawMutex, 1, 2>::new();
    let (mut device, radio) = resources.split(NetworkInterfaceId::new(0), [2; 6], allocator::<1>());
    let link = radio.link_controller();
    link.set_link_up(true);
    device.transmit(packet(pool, 2)).unwrap();
    device.transmit(packet(pool, 4)).unwrap();
    let retained = radio.try_take_for([2; 6]).unwrap();
    link.set_link_up(false);
    link.set_link_up(true);
    assert!(radio.try_receive_tx().is_none());
    device.transmit(packet(pool, 6)).unwrap();
    assert!(
        !device.can_transmit(),
        "new epoch must still account for the old retained owner"
    );
    drop(retained);
    assert!(device.can_transmit());
    drop(radio.try_receive_tx());
}

#[test]
fn admission_notification_observes_the_already_returned_packet_pool_slot() {
    struct PoolWake(PacketBufAllocator, AtomicUsize);
    impl Wake for PoolWake {
        fn wake(self: Arc<Self>) {
            self.wake_by_ref();
        }
        fn wake_by_ref(self: &Arc<Self>) {
            assert!(
                self.0.try_alloc().is_some(),
                "packet storage must be returned before admission wake"
            );
            self.1.fetch_add(1, Ordering::Relaxed);
        }
    }
    let pool = allocator::<1>();
    let mut resources = OwnedEndpointResources::<CriticalSectionRawMutex, 1, 1>::new();
    let (mut device, radio) = resources.split(NetworkInterfaceId::new(0), [2; 6], allocator::<1>());
    radio.link_controller().set_link_up(true);
    device.transmit(packet(pool, 2)).unwrap();
    let wake = Arc::new(PoolWake(pool, AtomicUsize::new(0)));
    device.register_waker(&Waker::from(wake.clone()));
    let retained = radio.try_receive_tx().unwrap();
    assert_eq!(wake.1.load(Ordering::Relaxed), 0);
    assert!(pool.try_alloc().is_none());
    drop(retained);
    assert_eq!(wake.1.load(Ordering::Relaxed), 1);
}
