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
}
