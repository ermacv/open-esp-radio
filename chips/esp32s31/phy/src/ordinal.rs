//! A position in a fixed table.

/// A position in a table of `N` entries: below `N` by construction, so a read
/// needs no bounds check and the end of the table is the end of [`Self::next`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Ordinal<const N: usize>(u8);

impl<const N: usize> Ordinal<N> {
    /// The first position.
    pub(crate) const FIRST: Self = {
        if N == 0 || N > u8::MAX as usize + 1 {
            panic!("an ordinal's table has between 1 and 256 entries");
        }
        Self(0)
    };

    /// The position `index` of the table, for a test that starts mid-way.
    #[cfg(test)]
    pub(crate) fn nth(index: u8) -> Self {
        assert!(usize::from(index) < N);
        Self(index)
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
    fn a_slot_writes_the_entry_at_its_position() {
        let mut table = [0; 2];
        let second = Ordinal::<2>::FIRST.next().expect("a second entry");
        *second.slot(&mut table) = 7;
        assert_eq!(table, [0, 7]);
    }
}
