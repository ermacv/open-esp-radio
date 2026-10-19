extern crate std;

use super::*;
use core::cell::Cell;
use std::{sync::mpsc, thread};

struct Name(usize);
struct Domain;

#[allow(
    unsafe_code,
    reason = "private test names are constructed once per cell"
)]
// SAFETY: tests issue each name once and reuse it only after take returns it.
// Each test has one private storage and keeps its names in that test. Names
// never leave this module's sealed construction/return protocol.
unsafe impl UniqueQueueIndex<2> for Name {
    fn queue_index(&self) -> usize {
        self.0
    }
}

#[allow(
    unsafe_code,
    reason = "the test binding has one private store per invocation"
)]
// SAFETY: Domain is constructed only alongside that test's store; its two
// private names are never routed to another invocation's store. They contain
// no pointer, borrow or destructor that could access the retained values.
unsafe impl CustodyDomain<2> for Domain {
    type Name = Name;
    type Stopped = Stopped;
}

/// The test binding's hardware-stopped proof.
struct Stopped;

struct Owner<'a>(&'a Cell<usize>, u8);
impl Drop for Owner<'_> {
    fn drop(&mut self) {
        self.0.set(self.0.get() + 1);
    }
}
std::thread_local! {
    /// Secrets erased by quarantine, summed; a test reads it after the store.
    static ERASED: Cell<usize> = const { Cell::new(0) };
}
/// A borrowed context holds no secret and no hardware-reachable owner.
#[allow(unsafe_code, reason = "test owners never release in quarantine")]
// SAFETY: the method does nothing, so it releases and moves nothing.
unsafe impl<T> Quarantine for &Cell<T> {
    fn quarantine(&mut self) {}
}
#[allow(unsafe_code, reason = "test owners never release in quarantine")]
// SAFETY: it only rewrites its own byte in place and releases nothing.
unsafe impl Quarantine for Owner<'_> {
    fn quarantine(&mut self) {
        // Erase the secret in place; the owner itself stays.
        ERASED.with(|erased| erased.set(erased.get() + usize::from(self.1)));
        self.1 = 0;
    }
}

#[test]
fn whole_owners_return_out_of_order_and_reuse_their_exact_place() {
    let drops = Cell::new(0);
    let store = IndexedCustody::new(Domain);
    let first = store.insert(Name(0), Owner(&drops, 11));
    let mut second = store.insert(Name(1), Owner(&drops, 22));
    store.with_mut(&mut second, |owner| owner.1 += 1);
    let (name, owner) = store.take(second);
    assert_eq!(owner.1, 23);
    let second = store.insert(name, owner);
    let (_, owner) = store.take(first);
    assert_eq!(owner.1, 11);
    drop(owner);
    let (_, owner) = store.take(second);
    drop(owner);
    drop(store);
    assert_eq!(drops.get(), 2);
}

#[test]
fn quarantine_keeps_values_whose_names_are_out_and_erases_them_in_place() {
    // Destroying a non-empty store is the quarantine path: a forgotten name
    // and an outstanding one both leave their owner in place, never dropped,
    // while each owner's secret is erased.
    let drops = Cell::new(0);
    ERASED.with(|erased| erased.set(0));
    {
        let store = IndexedCustody::new(Domain);
        let _lost_name = store.insert(Name(0), Owner(&drops, 1));
        let _in_flight = store.insert(Name(1), Owner(&drops, 2));
    }
    assert_eq!(drops.get(), 0);
    assert_eq!(ERASED.with(Cell::get), 3);
}

#[test]
fn finish_after_stop_returns_every_retained_value_across_cycles() {
    // The ordinary end: N installation cycles each leave a value with a lost
    // name and one with an outstanding name; finish returns both every time.
    const CYCLES: usize = 16;
    let drops = Cell::new(0);
    for cycle in 0..CYCLES {
        let store = IndexedCustody::new(Domain);
        let _lost_name = store.insert(Name(0), Owner(&drops, 1));
        let _in_flight = store.insert(Name(1), Owner(&drops, 2));
        let values: std::vec::Vec<u8> = store.finish(&Stopped).map(|owner| owner.1).collect();
        assert_eq!(values, [1, 2]);
        assert_eq!(drops.get(), 2 * (cycle + 1));
    }
}

#[test]
fn finish_drops_values_its_caller_did_not_take() {
    let drops = Cell::new(0);
    let store = IndexedCustody::new(Domain);
    let _first = store.insert(Name(0), Owner(&drops, 1));
    let _second = store.insert(Name(1), Owner(&drops, 2));
    let mut drained = store.finish(&Stopped);
    let first = drained.next().map(|owner| owner.1);
    assert_eq!(first, Some(1));
    drop(drained);
    assert_eq!(drops.get(), 2);
}

#[test]
fn a_name_crosses_threads_without_moving_or_sending_borrowed_context() {
    let context = Cell::new(7);
    let store = IndexedCustody::new(Domain);
    let name = store.insert(Name(0), &context);
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        assert_eq!(name.queue_index(), 0);
        tx.send(name).unwrap();
    })
    .join()
    .unwrap();
    let (_, returned) = store.take(rx.recv().unwrap());
    assert!(core::ptr::eq(returned, &context));
    returned.set(9);
    assert_eq!(context.get(), 9);
}
