//! Checked positions in one const-sized descriptor ring.

/// A position strictly below `COUNT`. A zero-sized ring has no such value.
/// Arithmetic retains the bound and cannot overflow, even for a full turn
/// or an arbitrarily large advance. This value grants no DMA ownership.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RingIndex<const COUNT: usize>(usize);

impl<const COUNT: usize> RingIndex<COUNT> {
    pub const fn checked(index: usize) -> Option<Self> {
        if index < COUNT {
            Some(Self(index))
        } else {
            None
        }
    }

    pub const fn get(self) -> usize {
        self.0
    }

    pub const fn advance(self, amount: usize) -> Self {
        // An inhabited RingIndex proves COUNT > 0. Subtract before adding
        // so a large representable ring does not overflow at its wrap.
        let amount = if amount <= COUNT {
            amount
        } else {
            amount % COUNT
        };
        let remaining = COUNT - self.0;
        Self(if amount >= remaining {
            amount - remaining
        } else {
            self.0 + amount
        })
    }

    pub const fn previous(self) -> Self {
        Self(if self.0 == 0 { COUNT - 1 } else { self.0 - 1 })
    }

    pub const fn distance_to(self, end: Self) -> usize {
        if end.0 >= self.0 {
            end.0 - self.0
        } else {
            COUNT - self.0 + end.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::RingIndex;

    #[test]
    fn ring_positions_reject_empty_or_foreign_bounds_and_preserve_circular_order() {
        assert_eq!(RingIndex::<0>::checked(0), None);
        assert_eq!(RingIndex::<96>::checked(96), None);
        for start in 0..96 {
            let index = RingIndex::<96>::checked(start).unwrap();
            assert_eq!(index.previous().advance(1), index);
            for distance in 0..=96 {
                let end = index.advance(distance);
                assert_eq!(end.get(), (start + distance) % 96);
                assert_eq!(index.distance_to(end), distance % 96);
            }
        }
        let last = RingIndex::<{ usize::MAX }>::checked(usize::MAX - 1).unwrap();
        assert_eq!(last.advance(2).get(), 1);
        assert_eq!(last.advance(usize::MAX), last);
    }
}
