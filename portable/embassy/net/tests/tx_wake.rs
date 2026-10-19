//! Force a producer handoff between radio queue operations without timing races.

use embassy_sync::blocking_mutex::raw::{NoopRawMutex, RawMutex};
use oer_embassy_net::{NetworkInterfaceId, OwnedEndpointResources};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    task::{Wake, Waker},
};
use xarxa_driver::{PacketBuf, PacketBufAllocator, PacketPool, PacketPoolStorage};

std::thread_local! {
    static AFTER_UNLOCK: RefCell<Option<Box<dyn FnOnce()>>> = RefCell::new(None);
}

struct HandoffMutex(NoopRawMutex);

// SAFETY: the inner NoopRawMutex keeps this type non-Sync. All access and the
// simulated producer handoff run on one thread, after the protected call ends.
unsafe impl RawMutex for HandoffMutex {
    const INIT: Self = Self(NoopRawMutex::INIT);

    fn lock<R>(&self, action: impl FnOnce() -> R) -> R {
        let result = self.0.lock(action);
        let handoff = AFTER_UNLOCK.with(|slot| slot.borrow_mut().take());
        if let Some(handoff) = handoff {
            handoff();
        }
        result
    }
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

fn allocator() -> PacketBufAllocator {
    let storage = Box::leak(Box::new(PacketPoolStorage::<4>::new()));
    Box::leak(Box::new(PacketPool::new(storage))).allocator()
}

fn packet(allocator: PacketBufAllocator) -> PacketBuf {
    let mut packet = allocator.try_alloc().unwrap();
    packet.set_len(14);
    packet.fill(0);
    packet
}

#[test]
fn producer_filling_budget_during_radio_handoff_waits_for_owner_release() {
    let general = allocator();
    let resources = Box::leak(Box::new(OwnedEndpointResources::<HandoffMutex, 1, 2>::new()));
    let (mut device, radio) = resources.split(NetworkInterfaceId::new(0), [2; 6], allocator());
    radio.link_controller().set_link_up(true);
    let wakes = Arc::new(Wakes::default());
    device.register_waker(&Waker::from(Arc::clone(&wakes)));
    device.transmit(packet(general)).unwrap();
    let device = Rc::new(RefCell::new(device));
    let waiting = Rc::new(Cell::new(false));
    let producer = Rc::clone(&device);
    let producer_waiting = Rc::clone(&waiting);
    AFTER_UNLOCK.with(|slot| {
        *slot.borrow_mut() = Some(Box::new(move || {
            let mut device = producer.borrow_mut();
            device.transmit(packet(general)).unwrap();
            producer_waiting.set(!device.can_transmit());
        }));
    });

    let claimed = radio
        .try_receive_tx()
        .expect("initial packet remains claimable");
    assert!(waiting.get());
    assert!(!device.borrow().can_transmit());
    assert_eq!(wakes.0.load(Ordering::Relaxed), 0);
    drop(claimed);
    assert!(device.borrow().can_transmit());
    assert_eq!(wakes.0.load(Ordering::Relaxed), 1);
}

#[test]
fn capacity_wake_is_one_full_to_available_edge_and_tracks_the_current_task() {
    let general = allocator();
    let resources = Box::leak(Box::new(OwnedEndpointResources::<NoopRawMutex, 1, 2>::new()));
    let (mut device, radio) = resources.split(NetworkInterfaceId::new(0), [2; 6], allocator());
    radio.link_controller().set_link_up(true);
    let previous = Arc::new(Wakes::default());
    let current = Arc::new(Wakes::default());
    device.register_waker(&Waker::from(Arc::clone(&previous)));
    device.register_waker(&Waker::from(Arc::clone(&current)));
    // Embassy may notify the displaced task during registration. Only the
    // current task must receive subsequent capacity notifications.
    let previous_wakes = previous.0.load(Ordering::Relaxed);
    device.transmit(packet(general)).unwrap();
    device.transmit(packet(general)).unwrap();
    assert!(!device.can_transmit());
    assert_eq!(current.0.load(Ordering::Relaxed), 0);

    drop(radio.try_receive_tx().unwrap());
    assert_eq!(current.0.load(Ordering::Relaxed), 1);
    drop(radio.try_receive_tx().unwrap());
    assert!(radio.try_receive_tx().is_none());
    assert_eq!(current.0.load(Ordering::Relaxed), 1);
    assert_eq!(previous.0.load(Ordering::Relaxed), previous_wakes);

    // The next stack poll registers again before producing a new burst.
    device.register_waker(&Waker::from(Arc::clone(&current)));
    device.transmit(packet(general)).unwrap();
    device.transmit(packet(general)).unwrap();
    assert!(!device.can_transmit());
    drop(radio.try_receive_tx().unwrap());
    assert_eq!(current.0.load(Ordering::Relaxed), 2);
}
