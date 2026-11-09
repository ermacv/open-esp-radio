//! The home of a pool's own slots.

use super::{Brand, PoolPath, Root, Slot};
use core::marker::PhantomData;

/// The places of a pool's own slots, one per slot index the [`Mint`]
/// issued: a slot returns to the place its index names, which is its own.
///
/// Returning is infallible and has no branch: the index of a slot of
/// this pool is unique (the mint issues each once, and a slot is neither
/// `Clone` nor `Copy`), so its place is vacant while the slot is out, and
/// the places cover every `u8` index, so no slot lies outside them. A slot
/// carries no value beyond its index: its place keeps that index alone,
/// and a [`take`](Self::take) gives the same slot back. A bank only ever
/// gives out slots that were put in, so it adds no capacity.
///
/// [`Mint`]: super::Mint
pub struct SlotBank<'id, Kind, Path: PoolPath = Root<crate::LocalId<'id>>> {
    /// The indices put in at construction: the pool's slots this bank keeps.
    issued: [u64; 4],
    /// The indices whose slot is at home.
    home: [u64; 4],
    brand: Brand<'id>,
    kind: PhantomData<fn() -> (Kind, Path)>,
}

/// The word of `index` and its bit in it. The four words cover every `u8`.
const fn place(index: u8) -> (usize, u64) {
    let word = match index >> 6 {
        0 => 0,
        1 => 1,
        2 => 2,
        _ => 3,
    };
    (word, 1u64.wrapping_shl((index & 63) as u32))
}

/// One of the four words.
const fn word_of(words: &[u64; 4], word: usize) -> u64 {
    match word {
        0 => words[0],
        1 => words[1],
        2 => words[2],
        _ => words[3],
    }
}

fn word_mut(words: &mut [u64; 4], word: usize) -> &mut u64 {
    let [first, second, third, fourth] = words;
    match word {
        0 => first,
        1 => second,
        2 => third,
        _ => fourth,
    }
}

impl<'id, Kind, Path: PoolPath> SlotBank<'id, Kind, Path> {
    /// A bank of no slots, which gives none out.
    pub const fn empty() -> Self {
        Self {
            issued: [0; 4],
            home: [0; 4],
            brand: PhantomData,
            kind: PhantomData,
        }
    }

    /// A bank holding `slots` at their own places: the pool's slots it
    /// keeps from here on.
    pub fn new<const N: usize>(slots: [Slot<'id, Kind, Path>; N]) -> Self {
        let mut bank = Self::empty();
        for slot in slots {
            let (word, bit) = place(slot.index);
            *word_mut(&mut bank.issued, word) |= bit;
            bank.put(slot);
        }
        bank
    }

    /// Return `slot` to its own place.
    pub fn put(&mut self, slot: Slot<'id, Kind, Path>) {
        let (word, bit) = place(slot.index);
        *word_mut(&mut self.home, word) |= bit;
    }

    /// The slot of `index`, when it is at home.
    pub fn take(&mut self, index: u8) -> Option<Slot<'id, Kind, Path>> {
        let (word, bit) = place(index);
        let kept = word_of(&self.issued, word) & word_of(&self.home, word) & bit != 0;
        if !kept {
            return None;
        }
        *word_mut(&mut self.home, word) &= !bit;
        Some(Slot {
            index,
            brand: self.brand,
            kind: PhantomData,
        })
    }

    /// The slot of the lowest index at home.
    pub fn take_first(&mut self) -> Option<Slot<'id, Kind, Path>> {
        let index = (0..=u8::MAX).find(|&index| self.is_home(index))?;
        self.take(index)
    }

    /// Whether the slot of `index` is at home.
    pub const fn is_home(&self, index: u8) -> bool {
        let (word, bit) = place(index);
        word_of(&self.issued, word) & word_of(&self.home, word) & bit != 0
    }

    /// How many of the bank's slots are at home.
    pub fn at_home(&self) -> u32 {
        self.issued
            .iter()
            .zip(&self.home)
            .map(|(issued, home)| (issued & home).count_ones())
            .fold(0, u32::saturating_add)
    }

    /// Whether every slot the bank was made with is at home.
    pub fn all_home(&self) -> bool {
        self.issued
            .iter()
            .zip(&self.home)
            .all(|(issued, home)| home & issued == *issued)
    }
}

#[cfg(test)]
mod tests {
    use super::SlotBank;
    use crate::pool::{Root, Slot};
    use core::marker::PhantomData;

    enum Ticket {}
    type Path = Root<crate::LocalId<'static>>;

    /// The slots a mint issues for a pool of `N`.
    fn minted<const N: usize>() -> [Slot<'static, Ticket, Path>; N] {
        core::array::from_fn(|index| Slot {
            index: super::super::index_of(index),
            brand: PhantomData,
            kind: PhantomData,
        })
    }

    #[test]
    fn a_slot_goes_back_to_its_own_place_and_comes_out_once() {
        let mut bank = SlotBank::new(minted::<3>());
        assert!(bank.all_home());
        assert_eq!(bank.at_home(), 3);
        let second = bank.take(1).expect("at home");
        assert_eq!(second.index(), 1);
        assert!(bank.take(1).is_none(), "one slot of an index exists");
        assert!(!bank.all_home());
        let first = bank.take_first().expect("index 0 at home");
        assert_eq!(first.index(), 0);
        bank.put(second);
        assert!(bank.is_home(1));
        assert!(!bank.is_home(0));
        bank.put(first);
        assert!(bank.all_home());
    }

    #[test]
    fn a_bank_gives_out_only_the_slots_it_was_made_with() {
        let [kept, other] = minted::<2>();
        let mut bank = SlotBank::new([kept]);
        assert!(bank.take(1).is_none(), "index 1 was never put in");
        assert!(bank.take(200).is_none());
        let kept = bank.take(0).expect("at home");
        assert!(bank.take_first().is_none());
        bank.put(kept);
        bank.put(other);
        assert_eq!(
            bank.at_home(),
            1,
            "a slot it was not made with adds no capacity"
        );
        assert!(bank.all_home());
    }

    #[test]
    fn the_places_cover_every_index() {
        let mut bank = SlotBank::new(minted::<256>());
        assert_eq!(bank.at_home(), 256);
        let last = bank.take(255).expect("at home");
        assert!(!bank.is_home(255));
        assert!(bank.is_home(254));
        bank.put(last);
        assert!(bank.all_home());
    }
}
