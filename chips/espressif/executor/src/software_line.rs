//! A software interrupt line: one level bit that wakers raise from every
//! context and the line's handler lowers. The line stays raised until its
//! handler lowers it, so the handler lowers it before it takes the work
//! the raises announced ([`serve`]): a raise that lands while the handler
//! takes the work raises the line again, and the handler runs another round.

/// One software interrupt line, shared by every context that raises it: the
/// platform port's contract.
pub(crate) use oer_espressif_platform_port::SoftwareLine;

/// One handler round: lower the line, then take the work (`drain`). A raise
/// during `drain` leaves the line raised for the next round.
#[inline(always)]
pub(crate) fn serve<R>(line: &impl SoftwareLine, drain: impl FnOnce() -> R) -> R {
    line.lower();
    drain()
}

#[cfg(test)]
mod tests;

/// One of the four software interrupt identities used by an executor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum SoftwareInterrupt {
    Zero,
    One,
    Two,
    Three,
}
impl SoftwareInterrupt {
    pub const fn from_index(index: u8) -> Option<Self> {
        match index {
            0 => Some(Self::Zero),
            1 => Some(Self::One),
            2 => Some(Self::Two),
            3 => Some(Self::Three),
            _ => None,
        }
    }
    /// A fixed source, checked during monomorphization.
    ///
    /// ```compile_fail
    /// use oer_espressif_executor::SoftwareInterrupt;
    /// const INVALID: SoftwareInterrupt = SoftwareInterrupt::constant::<4>();
    /// ```
    pub const fn constant<const SWI: u8>() -> Self {
        const {
            match Self::from_index(SWI) {
                Some(swi) => swi,
                None => panic!("software interrupt is out of range"),
            }
        }
    }
    pub(crate) const fn checked(index: u8) -> Self {
        match Self::from_index(index) {
            Some(swi) => swi,
            None => panic!("software interrupt is out of range"),
        }
    }
    pub const fn index(self) -> usize {
        self as usize
    }
}
