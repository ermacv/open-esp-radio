//! A position in a fixed table.

/// A position in a table of `N` entries: below `N` by construction, so a read
/// needs no bounds check and the end of the table is the end of [`Self::next`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Ordinal<const N: usize>(u8);

impl<const N: usize> Ordinal<N> {
    /// Every constructor evaluates this: a position fits `u8`, and `next`
    /// cannot wrap into a table of more than 256 entries.
    const SIZE: () = {
        if N == 0 || N > u8::MAX as usize + 1 {
            panic!("an ordinal's table has between 1 and 256 entries");
        }
    };

    /// The first position.
    pub(crate) const FIRST: Self = {
        let () = Self::SIZE;
        Self(0)
    };

    /// The position `index` of the table, for a test that starts mid-way.
    #[cfg(test)]
    pub(crate) fn nth(index: u8) -> Self {
        assert!(usize::from(index) < N);
        Self(index)
    }

    /// The position `index`, when the table has one.
    pub(crate) const fn position(index: u8) -> Option<Self> {
        let () = Self::SIZE;
        if (index as usize) < N {
            Some(Self(index))
        } else {
            None
        }
    }

    /// The position in a table of `W` entries, each covering `N / W` of these.
    pub(crate) const fn coarse<const W: usize>(self) -> Ordinal<W> {
        const {
            if W == 0 || N % W != 0 {
                panic!("a coarse table divides this one evenly");
            }
            let () = Ordinal::<W>::SIZE;
        }
        Ordinal((self.0 as usize / (N / W)) as u8)
    }

    /// The position `index`, or the last one past the table's end.
    pub(crate) const fn clamped(index: u8) -> Self {
        let () = Self::SIZE;
        let last = (N - 1) as u8;
        Self(if index > last { last } else { index })
    }

    /// The position before this one; `None` at the first.
    pub(crate) const fn previous(self) -> Option<Self> {
        match self.0.checked_sub(1) {
            Some(index) => Some(Self(index)),
            None => None,
        }
    }

    /// The position as the hardware sequence and its completions name it.
    pub(crate) const fn get(self) -> u8 {
        self.0
    }

    /// The position after this one; `None` past the last.
    pub(crate) const fn next(self) -> Option<Self> {
        let next = self.0 as usize + 1;
        if next < N {
            Some(Self(next as u8))
        } else {
            None
        }
    }

    /// The entry at this position.
    pub(crate) const fn at<T: Copy>(self, table: &[T; N]) -> T {
        table[self.0 as usize % N]
    }

    /// The entry at this position, to write.
    pub(crate) fn slot<T>(self, table: &mut [T; N]) -> &mut T {
        &mut table[self.0 as usize % N]
    }
}

impl<const N: usize> Default for Ordinal<N> {
    fn default() -> Self {
        Self::FIRST
    }
}

#[cfg(test)]
mod tests {
    use super::Ordinal;

    #[test]
    fn a_walk_visits_every_entry_once_and_ends() {
        let table = [10, 20, 30];
        let mut walked = std::vec::Vec::new();
        let mut position = Some(Ordinal::<3>::FIRST);
        while let Some(at) = position {
            walked.push((at.get(), at.at(&table)));
            position = at.next();
        }
        assert_eq!(walked, [(0, 10), (1, 20), (2, 30)]);
    }

    #[test]
    fn a_position_walks_back_and_clamps_at_the_end() {
        let last = Ordinal::<3>::clamped(9);
        assert_eq!(last.get(), 2);
        assert_eq!(last.previous().map(Ordinal::get), Some(1));
        assert_eq!(Ordinal::<3>::FIRST.previous(), None);
        assert_eq!(Ordinal::<3>::clamped(1).get(), 1);
        assert_eq!(Ordinal::<3>::position(3), None);
    }

    #[test]
    fn a_slot_writes_the_entry_at_its_position() {
        let mut table = [0; 2];
        let second = Ordinal::<2>::FIRST.next().expect("a second entry");
        *second.slot(&mut table) = 7;
        assert_eq!(table, [0, 7]);
    }
}
