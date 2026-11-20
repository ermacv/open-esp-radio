use core::{cell::Cell, mem::MaybeUninit};

use super::Occupied;

/// Counts its drops.
struct Counted<'c>(&'c Cell<u32>, u32);

impl Drop for Counted<'_> {
    fn drop(&mut self) {
        self.0.set(self.0.get() + 1);
    }
}

#[test]
fn a_filled_place_gives_its_value_by_reference_and_back_on_vacating() {
    let drops = Cell::new(0);
    let mut place = MaybeUninit::uninit();
    let mut occupied = Occupied::fill(&mut place, Counted(&drops, 1));
    assert_eq!(occupied.1, 1);
    occupied.1 = 2;
    let (value, place) = occupied.vacate();
    assert_eq!(value.1, 2);
    assert_eq!(
        drops.get(),
        0,
        "vacating moves the value out, nothing dropped"
    );
    drop(value);
    assert_eq!(drops.get(), 1);
    let again = Occupied::fill(place, Counted(&drops, 3));
    assert_eq!(again.1, 3, "the vacated place is filled again");
    let (value, _) = again.vacate();
    drop(value);
    assert_eq!(drops.get(), 2);
}

#[test]
fn a_handle_let_go_leaves_its_value_in_the_place() {
    let drops = Cell::new(0);
    let mut place = MaybeUninit::uninit();
    drop(Occupied::fill(&mut place, Counted(&drops, 5)));
    assert_eq!(drops.get(), 0, "no drop of the value: it stays where it is");
    // SAFETY: the test reads back what the dropped handle left.
    #[allow(unsafe_code, reason = "the test checks the place's bytes")]
    let left = unsafe { place.assume_init_read() };
    assert_eq!(left.1, 5);
}
