use core::ptr;

use super::*;
use crate::zeroed::ZeroedStatic;

#[test]
fn a_cell_is_empty_until_installed() {
    static CELL: HandlerCell<u32> = zeroed();
    static VALUE: ZeroedStatic<u32> = ZeroedStatic::new();
    assert_eq!(CELL.with(|value| *value), None);
    let value = VALUE.take();
    *value = 7;
    let installed = CELL.install(value).expect("an empty cell installs");
    assert!(ptr::eq(installed.cell(), &CELL));
    assert_eq!(CELL.with(|value| *value), Some(7));
}

#[test]
fn the_installed_value_and_its_changes_are_visible_to_the_handler() {
    static CELL: HandlerCell<[u32; 2]> = HandlerCell::new();
    static VALUE: ZeroedStatic<[u32; 2]> = ZeroedStatic::new();
    let value = VALUE.take();
    *value = [1, 2];
    let _installed = CELL.install(value).expect("an empty cell installs");
    assert_eq!(CELL.with(|value| *value), Some([1, 2]));
    assert_eq!(CELL.with(|value| value[1] = 5), Some(()));
    assert_eq!(CELL.with(|value| *value), Some([1, 5]));
}

#[test]
fn a_nested_call_is_refused_without_waiting() {
    static CELL: HandlerCell<u32> = HandlerCell::new();
    static VALUE: ZeroedStatic<u32> = ZeroedStatic::new();
    let _installed = CELL.install(VALUE.take()).expect("an empty cell installs");
    let nested = CELL.with(|outer| {
        *outer = 3;
        CELL.with(|inner| *inner)
    });
    assert_eq!(nested, Some(None));
    // The outer call returned the value: the next call gets it again.
    assert_eq!(CELL.with(|value| *value), Some(3));
}

#[test]
fn a_second_install_returns_its_value_and_keeps_the_first() {
    static CELL: HandlerCell<u32> = HandlerCell::new();
    static FIRST: ZeroedStatic<u32> = ZeroedStatic::new();
    static SECOND: ZeroedStatic<u32> = ZeroedStatic::new();
    let first = FIRST.take();
    *first = 1;
    let _installed = CELL.install(first).expect("an empty cell installs");
    let second = SECOND.take();
    *second = 2;
    let second_address: *const u32 = second;
    let Err(returned) = CELL.install(second) else {
        panic!("an installed cell refuses a second value");
    };
    assert!(ptr::eq(returned, second_address));
    assert_eq!(*returned, 2);
    assert_eq!(CELL.with(|value| *value), Some(1));
}

#[test]
fn the_empty_cell_is_all_zero_bytes() {
    let cell: HandlerCell<u32> = HandlerCell::new();
    assert_eq!(cell.state.load(Ordering::Relaxed), EMPTY);
    assert!(cell.value.load(Ordering::Relaxed).is_null());
    let zeroed: HandlerCell<u32> = zeroed();
    assert_eq!(zeroed.state.load(Ordering::Relaxed), EMPTY);
    assert!(zeroed.value.load(Ordering::Relaxed).is_null());
}
