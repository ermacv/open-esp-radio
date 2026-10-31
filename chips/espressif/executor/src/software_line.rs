//! A software interrupt line: one level bit that wakers raise from every
//! context and the line's handler lowers. The line stays raised until its
//! handler lowers it, so the handler lowers it before it takes the work
//! the raises announced ([`serve`]): a raise that lands while the handler
//! takes the work raises the line again, and the handler runs another round.
//!
//! The line is the announcement of facts in memory, which the raiser may
//! publish on another hart than the handler's. A device write is ordered
//! against memory accesses only by a fence with the I/O bits, so both sides
//! fence: the raiser publishes its facts, then `fence w, o`, then raises
//! ([`raise`]); the handler lowers, then `fence o, rw`, then takes the facts
//! ([`serve`]). Across harts the order is conditional on two hardware
//! preconditions the repository does not establish: the core honors the I/O
//! bits of FENCE and both harts' writes of one `HP_CPU_INT_FROM_CPU_n` word
//! are serialized (H1), and a level last written 1 eventually pends on the
//! receiving hart (H2). On one hart the line already orders behind program
//! order.

/// One software interrupt line, shared by every context that raises it: the
/// platform port's contract.
pub(crate) use oer_espressif_platform_port::SoftwareLine;

/// Raise `line` after the caller's published facts: `fence w, o`, then the
/// raise.
#[cfg_attr(
    not(feature = "chip"),
    allow(dead_code, reason = "the chip's executor and doorbell raise")
)]
#[inline(always)]
pub(crate) fn raise(line: &impl SoftwareLine) {
    raise_with(line, oer_riscv::fence_w_o);
}

/// [`raise`] with its fence as a parameter, so a test records the order.
#[inline(always)]
pub(crate) fn raise_with(line: &impl SoftwareLine, fence: impl FnOnce()) {
    fence();
    line.raise();
}

/// One handler round: lower the line, `fence o, rw`, then take the work
/// (`drain`). A raise during `drain` leaves the line raised for the next
/// round.
#[inline(always)]
pub(crate) fn serve<R>(line: &impl SoftwareLine, drain: impl FnOnce() -> R) -> R {
    serve_with(line, oer_riscv::fence_o_rw, drain)
}

/// [`serve`] with its fence as a parameter, so a test records the order.
#[inline(always)]
pub(crate) fn serve_with<R>(
    line: &impl SoftwareLine,
    fence: impl FnOnce(),
    drain: impl FnOnce() -> R,
) -> R {
    line.lower();
    fence();
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
    /// Select from the four source slots without a runtime bounds path.
    #[cfg(feature = "chip")]
    pub(crate) const fn at<T>(self, entries: &[T; 4]) -> &T {
        let [zero, one, two, three] = entries;
        match self {
            Self::Zero => zero,
            Self::One => one,
            Self::Two => two,
            Self::Three => three,
        }
    }
    pub const fn index(self) -> usize {
        self as usize
    }
}
