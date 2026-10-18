use super::*;
use core::{
    pin::pin,
    task::{Context, Poll, Waker},
};
use trouble_host::{Address, LongTermKey};

fn ready<F: Future>(future: F) -> F::Output {
    match pin!(future).poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("RAM operation must complete without waiting"),
    }
}

fn bond(peer: u8, key: u128) -> BondInformation {
    BondInformation::new(
        Identity::from(Address::random([peer, 2, 3, 4, 5, 0xc6])),
        LongTermKey::new(key),
        SecurityLevel::EncryptedAuthenticated,
        true,
    )
}

#[test]
fn full_store_and_duplicate_never_replace_existing_trust() {
    let mut store = RamBondStore::<1>::new();
    let first = bond(1, 123);
    ready(store.insert(first.clone())).unwrap();
    assert_eq!(ready(store.insert(bond(2, 456))), Err(StoreError::Full));
    assert_eq!(
        ready(store.insert(bond(1, 456))),
        Err(StoreError::AlreadyStored)
    );
    assert!(ready(store.load(0)).unwrap().as_ref() == Some(&first));
}

#[test]
fn enumeration_preserves_holes_and_requires_explicit_removal() {
    let mut store = RamBondStore::<2>::new();
    ready(store.insert(bond(1, 123))).unwrap();
    ready(store.insert(bond(2, 456))).unwrap();
    assert!(ready(store.remove(bond(1, 123).identity)).unwrap());
    assert!(ready(store.load(0)).unwrap().is_none());
    assert!(ready(store.load(1)).unwrap().as_ref() == Some(&bond(2, 456)));
    assert!(!ready(store.remove(bond(1, 123).identity)).unwrap());
    ready(store.insert(bond(1, 789))).unwrap();
    assert!(ready(store.load(0)).unwrap().as_ref() == Some(&bond(1, 789)));
}

#[test]
fn unauthenticated_and_nonbonded_records_are_rejected() {
    let mut store = RamBondStore::<1>::new();
    for level in [SecurityLevel::NoEncryption, SecurityLevel::Encrypted] {
        let mut record = bond(1, 123);
        record.security_level = level;
        assert_eq!(
            ready(store.insert(record)),
            Err(StoreError::InsufficientSecurity)
        );
    }
    let mut record = bond(1, 123);
    record.is_bonded = false;
    assert_eq!(
        ready(store.insert(record)),
        Err(StoreError::InsufficientSecurity)
    );
    assert!(ready(store.load(0)).unwrap().is_none());
}

#[test]
fn invalid_slots_and_zero_capacity_fail_explicitly() {
    let mut store = RamBondStore::<0>::new();
    assert_eq!(store.capacity(), 0);
    assert!(matches!(ready(store.load(0)), Err(StoreError::InvalidSlot)));
    assert_eq!(ready(store.insert(bond(1, 123))), Err(StoreError::Full));
}

#[test]
fn independent_working_copy_does_not_own_application_record() {
    let mut store = RamBondStore::<1>::new();
    ready(store.insert(bond(1, 123))).unwrap();
    {
        let working_copy = ready(store.load(0)).unwrap().unwrap();
        assert!(working_copy == bond(1, 123));
    }
    assert!(ready(store.load(0)).unwrap().as_ref() == Some(&bond(1, 123)));
    assert!(ready(RamBondStore::<1>::new().load(0)).unwrap().is_none());
}
