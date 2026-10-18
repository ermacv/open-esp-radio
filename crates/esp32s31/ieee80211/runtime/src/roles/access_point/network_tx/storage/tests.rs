use super::*;
use core::cell::Cell;

#[derive(Debug)]
struct Packet<'a>(&'a Cell<usize>);
impl Drop for Packet<'_> {
    fn drop(&mut self) {
        self.0.set(self.0.get() + 1);
    }
}

#[test]
fn epoch_drop_returns_all_retained_owners_and_reuses_every_slot() {
    let drops = Cell::new(0);
    let mut storage = AccessPointTxStorage::new();
    for epoch in 0..3 {
        let mut arena = storage.borrow();
        for _ in 0..super::super::AP_SOFTWARE_TX_CAPACITY {
            arena.insert(Packet(&drops)).unwrap();
        }
        assert_eq!(arena.remaining_capacity(), 0);
        assert_eq!(drops.get(), epoch * super::super::AP_SOFTWARE_TX_CAPACITY);
        drop(arena);
        assert_eq!(
            drops.get(),
            (epoch + 1) * super::super::AP_SOFTWARE_TX_CAPACITY
        );
    }
    drop(storage);
    assert_eq!(drops.get(), 3 * super::super::AP_SOFTWARE_TX_CAPACITY);
}

#[test]
fn frame_transfer_outlives_epoch_without_double_release() {
    let drops = Cell::new(0);
    let mut storage = AccessPointTxStorage::new();
    let mut arena = storage.borrow();
    let index = arena.insert(Packet(&drops)).unwrap();
    arena.insert(Packet(&drops)).unwrap();
    let transferred = arena.take(index);
    drop(arena);
    assert_eq!(drops.get(), 1);
    assert_eq!(
        storage.borrow().remaining_capacity(),
        super::super::AP_SOFTWARE_TX_CAPACITY
    );
    drop(transferred);
    assert_eq!(drops.get(), 2);
}
