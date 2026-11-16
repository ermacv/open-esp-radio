//! Connection epochs: the station's own count of its connection attempts.
//! A frame, a key's installation and a receive configuration carry the
//! epoch they were made in, so their end is told apart from the current
//! connection's. The epoch is a value; only the station's counter issues a
//! new one.

/// One connection attempt of the station.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LinkEpoch {
    sequence: u64,
}

impl LinkEpoch {
    /// The attempt's number, for a reader that tells two apart.
    pub const fn sequence(self) -> u64 {
        self.sequence
    }

    /// The connection `sequence`, for a host test.
    #[cfg(any(test, feature = "test-support"))]
    pub const fn for_test(sequence: u64) -> Self {
        Self { sequence }
    }
}

/// The station's counter of its connection attempts.
#[derive(Debug)]
pub(crate) struct LinkEpochs {
    last: u64,
}

impl LinkEpochs {
    pub(crate) const fn new() -> Self {
        Self { last: 0 }
    }

    /// The next attempt's epoch, wrapping at `u64::MAX`.
    ///
    /// An old epoch equals a new one only after 2^64 attempts while one of
    /// its frames or commands is still out; a frame's end comes within its
    /// command bound, far inside that.
    pub(crate) fn begin(&mut self) -> LinkEpoch {
        self.last = self.last.wrapping_add(1);
        LinkEpoch {
            sequence: self.last,
        }
    }

    #[cfg(test)]
    pub(crate) const fn for_test(last: u64) -> Self {
        Self { last }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adjacent_live_epochs_remain_distinct_across_wrap() {
        let mut epochs = LinkEpochs::for_test(u64::MAX - 1);
        let last = epochs.begin();
        let wrapped = epochs.begin();
        let next = epochs.begin();
        assert_eq!(last.sequence, u64::MAX);
        assert_eq!(wrapped.sequence, 0);
        assert_eq!(next.sequence, 1);
        assert_ne!(last, wrapped);
        assert_ne!(wrapped, next);
    }
}
