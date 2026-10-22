use super::*;
use core::{cell::Cell, marker::PhantomPinned, task::Waker};
extern crate std;
use std::boxed::Box;

struct Checked<'a> {
    address: &'a Cell<usize>,
    drops: &'a Cell<usize>,
    _pin: PhantomPinned,
}

impl Future for Checked<'_> {
    type Output = ();
    fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<()> {
        let address = (&*self as *const Self) as usize;
        if self.address.get() == 0 {
            self.address.set(address);
            Poll::Pending
        } else {
            assert_eq!(self.address.get(), address);
            Poll::Ready(())
        }
    }
}

impl Drop for Checked<'_> {
    fn drop(&mut self) {
        if self.address.get() != 0 {
            assert_eq!(self.address.get(), (self as *const Self) as usize);
        }
        self.drops.set(self.drops.get() + 1);
    }
}

#[test]
fn future_keeps_its_address_and_drops_short_lived_inputs_once() {
    for polls in 0..=2 {
        let address = Cell::new(0);
        let drops = Cell::new(0);
        let storage = Box::leak(Box::new(StaticPlace::<Checked<'static>>::new()));
        let mut future = storage.future(|| Checked {
            address: &address,
            drops: &drops,
            _pin: PhantomPinned,
        });
        let mut cx = Context::from_waker(Waker::noop());
        for index in 0..polls {
            assert_eq!(Pin::new(&mut future).poll(&mut cx).is_ready(), index == 1);
            // Moving the handle between polls does not move Checked.
            future = core::hint::black_box(future);
        }
        drop(future);
        assert_eq!(drops.get(), 1);
    }
}

#[test]
fn uninitialized_place_preserves_a_short_resource_loan() {
    let value = Cell::new(7);
    let storage = Box::leak(Box::new(StaticPlace::<&'static Cell<u32>>::new()));
    let placed = crate::Placed::init(storage.uninit(), |place| place.write(&value));
    placed.set(12);
    drop(placed);
    assert_eq!(value.get(), 12);
}

#[test]
fn unclaimed_backing_can_move_without_transferring_a_layout_value() {
    fn send<T: Send>() {}
    send::<StaticPlace<Checked<'static>>>();
}
