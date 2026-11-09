//! The place of a slot in its pool, bounded by the pool's size in its type.
//!
//! A slot of the layout [`Slots<Kind, N>`](super::Slots) names that layout
//! as its kind, so its [`place`](super::Slot::place) is a [`Place<N>`]:
//! below `N` by the pool's type, not by a check. A [`Places<T, N>`] keeps
//! one item per place of such a pool; a place of another pool's size does
//! not index it:
//!
//! ```compile_fail,E0308
//! use oer_protocols_radio_port::{Place, Places};
//! fn other(places: &mut Places<u8, 7>, place: Place<8>) {
//!     let _ = places.take(place);
//! }
//! ```
//!
//! ```
//! use oer_protocols_radio_port::{Mint, Places, Slots};
//! enum Keys {}
//! fn keep<'id>(mint: Mint<'id, Slots<Keys, 2>>) -> Places<u8, 2> {
//!     let [first, second] = mint.into_pools();
//!     let mut places = Places::new();
//!     assert!(places.put(second.place(), 2).is_ok());
//!     assert!(places.put(first.place(), 1).is_ok());
//!     places
//! }
//! ```
//!
//! No caller makes a place:
//!
//! ```compile_fail,E0451
//! use oer_protocols_radio_port::Place;
//! fn forge() -> Place<4> {
//!     Place { index: 9 }
//! }
//! ```
//!
//! A bounded place does not prove that its slot is the only one of its
//! index the caller put in, nor that the place is free: a put into a taken
//! place returns its item whole.

use super::{PoolPath, Slot, Slots};

/// The place of a slot in a pool of `N`: its index, below `N`.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Place<const N: usize> {
    index: u8,
}

impl<const N: usize> Place<N> {
    /// The place's index, below `N`.
    pub const fn index(self) -> u8 {
        self.index
    }

    /// Every place of a pool of `N`, in order.
    pub fn all() -> impl Iterator<Item = Self> {
        (0..N).map(|index| Self {
            index: super::index_of(index),
        })
    }
}

impl<K, const N: usize, Path: PoolPath> Slot<'_, Slots<K, N>, Path> {
    /// The slot's place in its pool of `N`.
    pub const fn place(&self) -> Place<N> {
        Place { index: self.index }
    }
}

/// One item per place of a pool of `N`.
pub struct Places<T, const N: usize> {
    items: [Option<T>; N],
}

impl<T, const N: usize> Default for Places<T, N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T, const N: usize> Places<T, N> {
    /// No place holds an item.
    pub const fn new() -> Self {
        Self {
            items: [const { None }; N],
        }
    }

    /// The item at `place`'s place, for writing. The index is below `N` by
    /// the place's type, so the place is always there.
    fn at(&mut self, place: Place<N>) -> Option<&mut Option<T>> {
        self.items.get_mut(usize::from(place.index))
    }

    /// Put `item` at `place`.
    ///
    /// # Errors
    ///
    /// The place holds an item: `item`, whole; nothing changed.
    pub fn put(&mut self, place: Place<N>, item: T) -> Result<(), T> {
        match self.at(place) {
            Some(vacant @ None) => {
                *vacant = Some(item);
                Ok(())
            }
            // A taken place; a place outside the array is excluded by the
            // place's type and refused the same way.
            _ => Err(item),
        }
    }

    /// Take the item at `place`, if it holds one.
    pub fn take(&mut self, place: Place<N>) -> Option<T> {
        self.at(place).and_then(Option::take)
    }

    /// The item at `place`, if it holds one.
    pub fn get(&self, place: Place<N>) -> Option<&T> {
        self.items
            .get(usize::from(place.index))
            .and_then(Option::as_ref)
    }

    /// The first held item that `matches`, for writing: the place stays
    /// borrowed, so a refused item goes back to the place it left.
    pub fn find_mut(&mut self, mut matches: impl FnMut(&T) -> bool) -> Option<&mut Option<T>> {
        self.items
            .iter_mut()
            .find(|item| item.as_ref().is_some_and(&mut matches))
    }

    /// Every held item, for writing.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut T> {
        self.items.iter_mut().flatten()
    }

    /// Every held item.
    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.items.iter().flatten()
    }

    /// Whether no place holds an item.
    pub fn is_empty(&self) -> bool {
        self.items.iter().all(Option::is_none)
    }
}

#[cfg(test)]
mod tests {
    use super::{Place, Places};
    use crate::pool::{Root, Slot, Slots};
    use core::marker::PhantomData;

    enum Ticket {}
    type Path = Root<crate::LocalId<'static>>;

    fn minted<const N: usize>() -> [Slot<'static, Slots<Ticket, N>, Path>; N] {
        core::array::from_fn(|index| Slot {
            index: super::super::index_of(index),
            brand: PhantomData,
            kind: PhantomData,
        })
    }

    #[test]
    fn an_item_goes_to_its_slots_place_and_a_taken_place_returns_it_whole() {
        let [first, second, third] = minted::<3>();
        let mut places = Places::<&str, 3>::new();
        assert!(places.is_empty());
        assert_eq!(places.put(second.place(), "second"), Ok(()));
        assert_eq!(places.put(second.place(), "again"), Err("again"));
        assert_eq!(places.get(second.place()), Some(&"second"));
        assert_eq!(places.get(first.place()), None);
        assert_eq!(places.put(third.place(), "third"), Ok(()));
        assert_eq!(places.take(second.place()), Some("second"));
        assert_eq!(places.take(second.place()), None);
        assert!(places.iter().eq([&"third"]));
        let held = places.find_mut(|item| *item == "third").expect("held");
        assert_eq!(held.take(), Some("third"));
        assert!(places.is_empty());
    }

    #[test]
    fn every_place_of_a_pool_is_below_its_size() {
        assert!(Place::<4>::all().map(Place::index).eq([0, 1, 2, 3]));
        let last = minted::<256>().map(|slot| slot.place().index());
        assert_eq!(last.last(), Some(&255));
    }
}
