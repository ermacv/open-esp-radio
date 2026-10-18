#[path = "../src/mac_interrupt_epoch/storage.rs"]
mod storage;

use std::{cell::Cell, rc::Rc};

#[test]
fn missing_peer_preserves_owner_and_active_route() {
    let owner = Rc::new(());
    let mut first = Some(owner.clone());
    let mut second: Option<Rc<()>> = None;
    let disabled = Cell::new(false);
    assert!(storage::detach_pair(&mut first, &mut second, || disabled.set(true)).is_none());
    assert!(!disabled.get());
    assert!(Rc::ptr_eq(first.as_ref().unwrap(), &owner));
    assert!(storage::detach_pair(&mut second, &mut first, || disabled.set(true)).is_none());
    assert!(!disabled.get());
    assert!(Rc::ptr_eq(first.as_ref().unwrap(), &owner));
}

#[test]
fn successful_detach_keeps_both_capabilities_until_handlers_are_excluded() {
    let first_owner = Rc::new(());
    let second_owner = Rc::new(());
    let mut first = Some(first_owner.clone());
    let mut second = Some(second_owner.clone());
    let disabled = Cell::new(false);
    let (a, b) = storage::detach_pair(&mut first, &mut second, || {
        assert_eq!(Rc::strong_count(&first_owner), 2);
        assert_eq!(Rc::strong_count(&second_owner), 2);
        disabled.set(true);
    })
    .unwrap();
    assert!(disabled.get());
    assert!(first.is_none() && second.is_none());
    assert!(Rc::ptr_eq(&a, &first_owner));
    assert!(Rc::ptr_eq(&b, &second_owner));
    disabled.set(false);
    assert!(storage::detach_pair(&mut first, &mut second, || disabled.set(true)).is_none());
    assert!(!disabled.get());
}
