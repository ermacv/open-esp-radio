#![allow(
    clippy::unwrap_used,
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros
)]

extern crate std;

use core::cell::Cell;
use core::sync::atomic::Ordering::Acquire;
use core::task::{RawWaker, RawWakerVTable, Waker};
use std::boxed::Box;

use super::{AtomicWaker, REGISTERING, WAITING, WAKING};

/// A waker that counts its uses and may run `on_clone` once, inside the
/// registration that clones it: the place a higher level preempts it.
struct Counted {
    wakes: Cell<u32>,
    clones: Cell<u32>,
    drops: Cell<u32>,
    on_clone: Cell<Option<Box<dyn FnOnce()>>>,
}

impl Counted {
    fn new() -> &'static Self {
        Box::leak(Box::new(Self {
            wakes: Cell::new(0),
            clones: Cell::new(0),
            drops: Cell::new(0),
            on_clone: Cell::new(None),
        }))
    }

    #[allow(unsafe_code, reason = "a test waker over a leaked counter")]
    fn waker(&'static self) -> Waker {
        // SAFETY: the data pointer is a leaked `Counted`, valid for ever, and
        // the vtable's functions treat it so.
        unsafe { Waker::from_raw(RawWaker::new(core::ptr::from_ref(self).cast(), &VTABLE)) }
    }
}

#[allow(unsafe_code, reason = "a test waker over a leaked counter")]
fn counted(data: *const ()) -> &'static Counted {
    // SAFETY: every data pointer here comes from `Counted::waker`.
    unsafe { &*data.cast() }
}

fn clone(data: *const ()) -> RawWaker {
    let counted = counted(data);
    counted.clones.set(counted.clones.get() + 1);
    if let Some(run) = counted.on_clone.take() {
        run();
    }
    RawWaker::new(data, &VTABLE)
}

fn wake_by_ref(data: *const ()) {
    let counted = counted(data);
    counted.wakes.set(counted.wakes.get() + 1);
}

fn wake(data: *const ()) {
    wake_by_ref(data);
    drop_waker(data);
}

fn drop_waker(data: *const ()) {
    let counted = counted(data);
    counted.drops.set(counted.drops.get() + 1);
}

static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, wake, wake_by_ref, drop_waker);

fn cell() -> &'static AtomicWaker {
    Box::leak(Box::new(AtomicWaker::new()))
}

#[test]
fn a_wake_with_nothing_registered_does_nothing() {
    let cell = cell();
    cell.wake();
    assert!(cell.take().is_none());
}

#[test]
fn a_registered_waker_is_woken_once() {
    let (cell, task) = (cell(), Counted::new());
    cell.register(&task.waker());
    cell.wake();
    cell.wake();
    assert_eq!(task.wakes.get(), 1);
}

#[test]
fn the_same_task_keeps_its_stored_waker_and_another_replaces_it() {
    let (cell, first, second) = (cell(), Counted::new(), Counted::new());
    let waker = first.waker();
    cell.register(&waker);
    cell.register(&waker);
    assert_eq!(first.clones.get(), 1);
    cell.register(&second.waker());
    cell.wake();
    assert_eq!((first.wakes.get(), second.wakes.get()), (0, 1));
    drop(waker);
    assert_eq!(first.drops.get(), 2, "the stored clone and the original");
}

#[test]
fn a_wake_during_a_registration_is_carried_by_its_waker() {
    let (cell, task) = (cell(), Counted::new());
    task.on_clone.set(Some(Box::new(|| cell.wake())));
    let waker = task.waker();
    cell.register(&waker);
    assert_eq!(task.wakes.get(), 1, "the registration owed this wake");
    assert_eq!(cell.state.load(Acquire), WAITING);
    assert!(cell.take().is_none(), "that wake emptied the cell");
    assert_eq!(task.drops.get(), 1, "the stored clone");
}

#[test]
fn a_take_during_a_registration_takes_nothing() {
    let (cell, task) = (cell(), Counted::new());
    let taken = Box::leak(Box::new(Cell::new(true)));
    let seen: &'static Cell<bool> = taken;
    task.on_clone
        .set(Some(Box::new(move || seen.set(cell.take().is_some()))));
    cell.register(&task.waker());
    assert!(!taken.get());
    assert_eq!(task.wakes.get(), 1);
}

#[test]
fn a_registration_during_a_wake_wakes_its_task_and_stores_nothing() {
    let (cell, task) = (cell(), Counted::new());
    cell.state
        .store(WAKING, core::sync::atomic::Ordering::Release);
    cell.register(&task.waker());
    assert_eq!(task.wakes.get(), 1);
    assert_eq!(task.clones.get(), 0);
}

#[test]
fn a_second_registration_during_one_does_nothing() {
    let (cell, task) = (cell(), Counted::new());
    cell.state
        .store(REGISTERING, core::sync::atomic::Ordering::Release);
    cell.register(&task.waker());
    assert_eq!((task.wakes.get(), task.clones.get()), (0, 0));
}

#[test]
fn the_cell_holds_two_words_and_its_state() {
    assert_eq!(
        size_of::<AtomicWaker>(),
        size_of::<usize>() + size_of::<Waker>()
    );
}
