//! Checked frame geometry at the storage boundary.

/// A byte length which fits the named allocation capacity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrameLen<const CAP: usize>(usize);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrameLenError {
    pub length: usize,
    pub capacity: usize,
}

impl<const CAP: usize> FrameLen<CAP> {
    pub const fn new(length: usize) -> Result<Self, FrameLenError> {
        if length <= CAP {
            Ok(Self(length))
        } else {
            Err(FrameLenError {
                length,
                capacity: CAP,
            })
        }
    }

    /// A fixed length, validated during monomorphization.
    pub const fn constant<const LEN: usize>() -> Self {
        const {
            if LEN > CAP {
                panic!("frame length exceeds capacity");
            }
        }
        Self(LEN)
    }

    pub const fn get(self) -> usize {
        self.0
    }
}

/// A refused view; the initialized extent and requested geometry are retained.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrameRangeError {
    pub offset: usize,
    pub length: usize,
    pub initialized_offset: usize,
    pub initialized_length: usize,
}

impl FrameRangeError {
    pub(crate) fn check(
        offset: usize,
        length: usize,
        initialized_offset: usize,
        initialized_length: usize,
    ) -> Result<(), Self> {
        let valid = offset
            .checked_sub(initialized_offset)
            .and_then(|relative| relative.checked_add(length))
            .is_some_and(|end| end <= initialized_length);
        if valid {
            Ok(())
        } else {
            Err(Self {
                offset,
                length,
                initialized_offset,
                initialized_length,
            })
        }
    }
}

/// Refusal to lend a pool slot; no storage ownership transfers on failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PoolError {
    Index {
        index: u8,
        slots: usize,
    },
    State {
        index: u8,
        expected: u8,
        observed: u8,
    },
    Length(FrameLenError),
    Full,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lengths_accept_zero_and_capacity_and_retain_refused_input() {
        assert_eq!(FrameLen::<8>::new(0), Ok(FrameLen(0)));
        assert_eq!(FrameLen::<8>::new(8), Ok(FrameLen::constant::<8>()));
        assert_eq!(
            FrameLen::<8>::new(9),
            Err(FrameLenError {
                length: 9,
                capacity: 8
            })
        );
        assert_eq!(FrameLen::<0>::new(0), Ok(FrameLen::constant::<0>()));
    }

    #[test]
    fn ranges_reject_overflow_and_bytes_outside_the_published_view() {
        assert!(FrameRangeError::check(4, 8, 4, 8).is_ok());
        assert!(FrameRangeError::check(12, 0, 4, 8).is_ok());
        assert!(FrameRangeError::check(3, 1, 4, 8).is_err());
        assert!(FrameRangeError::check(5, 8, 4, 8).is_err());
        assert!(FrameRangeError::check(usize::MAX, 2, 4, 8).is_err());
    }
}
