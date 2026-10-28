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
