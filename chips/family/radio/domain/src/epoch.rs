//! Registration epoch policy; the chip retains the affine registration grant.
use core::num::NonZeroU32;

/// One packed word travels through every route of the same root.
#[derive(Debug)]
pub struct Registration(u32);
impl Registration {
    const CURRENT: u32 = 1 << 31;
    const COUNTER: u32 = Self::CURRENT - 1;
    pub const fn new() -> Self {
        Self(0)
    }
    const fn issued(&self) -> u32 {
        self.0 & Self::COUNTER
    }
    pub fn begin(&mut self) -> NonZeroU32 {
        // Preserve the existing total 31-bit counter: a full calibration per
        // registration bounds practical boot lifetime; wrap skips zero.
        let next = self.issued().wrapping_add(1) & Self::COUNTER;
        let issued = NonZeroU32::new(next).unwrap_or(NonZeroU32::MIN);
        self.0 = issued.get() | Self::CURRENT;
        issued
    }
    pub const fn current(&self) -> Option<NonZeroU32> {
        if self.0 & Self::CURRENT == 0 {
            None
        } else {
            NonZeroU32::new(self.issued())
        }
    }
    pub const fn retired(self) -> Self {
        Self(self.issued())
    }
}
impl Default for Registration {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retirement_preserves_the_counter_and_wrap_skips_zero() {
        let mut registration = Registration::new();
        assert_eq!(registration.current(), None);
        assert_eq!(registration.begin().get(), 1);
        assert_eq!(registration.current().map(NonZeroU32::get), Some(1));
        let mut retired = registration.retired();
        assert_eq!(retired.current(), None);
        assert_eq!(retired.begin().get(), 2);
        let mut wrapping = Registration(Registration::CURRENT | Registration::COUNTER);
        assert_eq!(wrapping.begin().get(), 1);
        assert_eq!(wrapping.current().map(NonZeroU32::get), Some(1));
    }
}
