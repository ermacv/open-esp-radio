//! A place in which a value stays, and the handle that says it is there.
#![allow(
    unsafe_code,
    reason = "the handle's borrow of its place is the proof that the place holds a value"
)]

use core::{
    mem::MaybeUninit,
    ops::{Deref, DerefMut},
};

/// A place that holds a value: made by filling the place, given up by
/// [`Occupied::vacate`], which hands the value and the empty place back.
///
/// Who holds which says the state: the handle is the value in its place,
/// the bare place is empty. The handle has no `Drop` of its own: a handle
/// let go leaves its value in the place, unread and undropped.
///
/// A vacated handle is gone (`tests/ui/occupied_vacated_is_gone.rs`), and
/// an occupied place is not filled again
/// (`tests/ui/occupied_place_fills_once.rs`).
pub struct Occupied<'p, T> {
    place: &'p mut MaybeUninit<T>,
}

impl<'p, T> Occupied<'p, T> {
    /// Write `value` into `place`; the handle is the place, now holding it.
    pub fn fill(place: &'p mut MaybeUninit<T>, value: T) -> Self {
        place.write(value);
        Self { place }
    }

    /// The value out, and its place back, empty.
    pub fn vacate(self) -> (T, &'p mut MaybeUninit<T>) {
        let Self { place } = self;
        // SAFETY: a handle exists only after `fill` wrote its place, and it
        // holds the place's one unique borrow for `'p`, so nothing else has
        // emptied or written it since. The handle is consumed here, and the
        // place goes back as `MaybeUninit`, whose type says it holds nothing:
        // the value read out is not read again.
        let value = unsafe { place.assume_init_read() };
        (value, place)
    }
}

impl<T> Deref for Occupied<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        // SAFETY: `fill` wrote the place, and the handle's unique borrow
        // keeps it so until `vacate` consumes the handle.
        unsafe { self.place.assume_init_ref() }
    }
}

impl<T> DerefMut for Occupied<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: as `deref`, with the handle borrowed uniquely.
        unsafe { self.place.assume_init_mut() }
    }
}

#[cfg(test)]
mod tests;
