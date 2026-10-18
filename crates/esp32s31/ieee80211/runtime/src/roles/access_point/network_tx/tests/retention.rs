use super::super::{AP_ACTIVE_FRAME_CAPACITY, ApGroupFrameQueue};
use super::*;

#[cfg(feature = "owned-network")]
#[test]
fn power_save_tickets_preserve_owned_admission_through_rollback_and_completion() {
    use embassy_sync::blocking_mutex::raw::NoopRawMutex;
    use oer_embassy_net_owned::{NetworkInterfaceId, OwnedEndpointResources};
    use std::boxed::Box;
    use xarxa_driver::{PacketPool, PacketPoolStorage};

    let storage = Box::leak(Box::new(PacketPoolStorage::<3>::new()));
    let pool = Box::leak(Box::new(PacketPool::new(storage))).allocator();
    let mut resources = OwnedEndpointResources::<NoopRawMutex, 1, 2>::new();
    let (mut device, radio) = resources.split(NetworkInterfaceId::new(0), [2; 6], pool);
    radio.link_controller().set_link_up(true);
    for destination in [2, 255] {
        let mut packet = pool.try_alloc().unwrap();
        packet.set_len(14);
        packet.fill(destination);
        device.transmit(packet).unwrap();
    }
    let mut arena = ApFrameLeaseArena::new();
    let mut unicast = ApPowerSaveFrameQueue::new();
    let mut group = ApGroupFrameQueue::new();
    let identity = FLOW_A.association().unwrap();
    let unicast_index = unicast
        .push(identity, radio.try_receive_tx().unwrap(), &mut arena)
        .unwrap_or_else(|_| panic!("unicast admission"));
    let group_index = group
        .push(radio.try_receive_tx().unwrap(), &mut arena)
        .unwrap_or_else(|_| panic!("group admission"));
    assert_eq!(radio.tx_queue_len(), 0);
    assert!(!device.can_transmit());
    let release = unicast.take_at(unicast_index).unwrap();
    assert!(
        !device.can_transmit(),
        "preparing release retains admission"
    );
    unicast.restore(release);
    assert!(!device.can_transmit(), "rollback retains admission");
    let release = group.take_at(group_index).unwrap();
    group.restore(release);
    assert!(!device.can_transmit());
    let release = unicast.take_at(unicast_index).unwrap();
    release.complete(&mut arena);
    assert!(device.can_transmit());
    let mut packet = pool.try_alloc().unwrap();
    packet.set_len(14);
    packet.fill(4);
    device.transmit(packet).unwrap();
    assert!(
        !device.can_transmit(),
        "group retention and new ingress share the limit"
    );
    let release = group.take_at(group_index).unwrap();
    release.complete(&mut arena);
    assert!(device.can_transmit());
    drop(radio.try_receive_tx());
}

#[test]
fn power_save_rollback_keeps_fifo_and_releases_each_owner_once() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    struct Owner(usize, Arc<AtomicUsize>);
    impl Drop for Owner {
        fn drop(&mut self) {
            self.1.fetch_add(1, Ordering::Relaxed);
        }
    }
    let drops = Arc::new(AtomicUsize::new(0));
    let mut arena = ApFrameLeaseArena::new();
    let mut queue = ApPowerSaveFrameQueue::new();
    let identity = FLOW_A.association().unwrap();
    for n in 0..2 {
        queue
            .push(identity, Owner(n, drops.clone()), &mut arena)
            .unwrap_or_else(|_| panic!("test admission"));
    }
    let index = queue.oldest_index_for(identity).unwrap();
    let release = queue.take_at(index).unwrap();
    queue
        .push(identity, Owner(2, drops.clone()), &mut arena)
        .unwrap_or_else(|_| panic!("concurrent admission"));
    assert_eq!(drops.load(Ordering::Relaxed), 0);
    queue.restore(release);
    for n in 0..3 {
        let index = queue.oldest_index_for(identity).unwrap();
        let release = queue.take_at(index).unwrap();
        assert_eq!(release.frame(&arena).0, n);
        release.complete(&mut arena);
        assert_eq!(drops.load(Ordering::Relaxed), n + 1);
    }
    drop(arena);
    assert_eq!(drops.load(Ordering::Relaxed), 3);
}

#[test]
fn unicast_release_keeps_its_storage_credit_until_terminal_completion() {
    let mut arena = ApFrameLeaseArena::new();
    let mut queue = ApPowerSaveFrameQueue::new();
    let identity = FLOW_A.association().unwrap();
    let first = queue.push(identity, 10, &mut arena).unwrap();
    for value in 1..AP_ACTIVE_FRAME_CAPACITY {
        arena.insert(value).unwrap();
    }
    let release = queue.take_at(first).unwrap();
    assert_eq!(
        arena.remaining_capacity(),
        0,
        "in-flight release must keep rollback storage"
    );
    assert_eq!(arena.insert(999), Err(999));
    queue.restore(release);
    let first = queue.oldest_index_for(identity).unwrap();
    let release = queue.take_at(first).unwrap();
    assert_eq!(*release.frame(&arena), 10);
    release.complete(&mut arena);
    assert_eq!(arena.remaining_capacity(), 1);
    assert!(arena.insert(999).is_ok());
}

#[test]
fn group_release_keeps_its_storage_credit_until_terminal_completion() {
    let mut arena = ApFrameLeaseArena::new();
    let mut queue = ApGroupFrameQueue::new();
    let first = queue.push(10, &mut arena).unwrap();
    for value in 1..AP_ACTIVE_FRAME_CAPACITY {
        arena.insert(value).unwrap();
    }
    let release = queue.take_at(first).unwrap();
    assert_eq!(
        arena.remaining_capacity(),
        0,
        "DTIM release must keep rollback storage"
    );
    assert_eq!(arena.insert(999), Err(999));
    queue.restore(release);
    let first = queue.oldest_index().unwrap();
    let release = queue.take_at(first).unwrap();
    assert_eq!(*release.frame(&arena), 10);
    release.complete(&mut arena);
    assert_eq!(arena.remaining_capacity(), 1);
    assert!(arena.insert(999).is_ok());
}
