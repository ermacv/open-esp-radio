#![allow(unsafe_code, reason = "host ranges have no asynchronous DMA actor")]

use super::StableDmaRange;

fn bind<T: ?Sized>(storage: &mut T, start: u32) -> Option<(&mut T, StableDmaRange<'_>)> {
    // SAFETY: the synthetic host address maps to this complete allocation.
    // No hardware accesses it; the returned borrow retains the storage.
    unsafe { StableDmaRange::from_owner(storage, start) }
}

#[test]
fn range_extent_comes_from_the_storage_and_returns_its_unique_borrow() {
    let mut storage = [0_u8; 16];
    let (owner, range) = bind(&mut storage[..], 100).expect("host allocation fits");
    owner[0] = 7;
    assert_eq!(range.start(), 100);
    assert!(range.contains(100, 16));
    assert!(!range.contains(100, 17));
    assert!(!range.contains(99, 1));
    assert_eq!(owner[0], 7);
}

#[test]
fn subranges_cannot_escape_or_widen_their_allocation() {
    let mut storage = [0_u8; 16];
    let (_, range) = bind(&mut storage, 100).expect("host allocation fits");
    let inner = range.subrange(104, 8).expect("interior range fits");
    assert_eq!(inner.start(), 104);
    assert!(inner.contains(104, 8));
    assert!(inner.subrange(104, 9).is_none());
    assert!(inner.subrange(103, 1).is_none());
    assert!(range.subrange(115, 2).is_none());
    assert!(range.subrange(116, 1).is_none());
    assert!(range.subrange(100, 0).is_none());
    assert!(range.subrange(u32::MAX, 2).is_none());
}

#[test]
fn empty_and_wrapping_allocations_have_no_range() {
    assert!(bind(&mut [0_u8; 0], 100).is_none());
    assert!(bind(&mut [0_u8; 16], u32::MAX - 15).is_none());
    assert!(bind(&mut [0_u8; 16], u32::MAX - 16).is_some());
}
