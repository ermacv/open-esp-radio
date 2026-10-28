//! Admitted packet prefixes, retaining each owner's existing length storage.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(transparent)]
pub(crate) struct Length<T, const CAPACITY: usize>(T);

macro_rules! length {
    ($storage:ty) => {
        impl<const CAPACITY: usize> Length<$storage, CAPACITY> {
            pub(crate) const fn with_header<const HEADER: usize, const BODY: usize>() -> Self {
                const {
                    if !(HEADER <= CAPACITY
                        && BODY <= CAPACITY - HEADER
                        && CAPACITY <= <$storage>::MAX as usize)
                    {
                        panic!("HCI packet exceeds its fixed capacity");
                    }
                    Self((HEADER + BODY) as $storage)
                }
            }
        }
    };
}

length!(u8);
length!(usize);

impl<T: Copy + Into<usize>, const CAPACITY: usize> Length<T, CAPACITY> {
    pub(crate) fn new(value: T) -> Option<Self> {
        if value.into() <= CAPACITY {
            Some(Self(value))
        } else {
            None
        }
    }

    pub(crate) const fn get(self) -> T {
        self.0
    }

    /// The constructor proves that trimming the unused tail preserves exactly
    /// the admitted prefix. No second bounds check or fallback is needed.
    pub(crate) fn borrow(self, bytes: &[u8; CAPACITY]) -> &[u8] {
        let mut prefix = bytes.iter();
        let mut unused = self.0.into()..CAPACITY;
        if unused.next().is_some() {
            prefix.nth_back(unused.len());
        }
        prefix.as_slice()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefixes_reject_excess_and_preserve_empty_partial_and_full_packets() {
        let bytes = [1, 2, 3];
        for length in 0..=3_u8 {
            assert_eq!(
                Length::<u8, 3>::new(length).unwrap().borrow(&bytes),
                &bytes[..usize::from(length)]
            );
        }
        assert_eq!(Length::<u8, 3>::new(4), None);
        assert_eq!(Length::<u16, 3>::new(u16::MAX), None);
        assert_eq!(Length::<usize, 3>::new(usize::MAX), None);
    }
}
