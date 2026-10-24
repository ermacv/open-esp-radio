//! Which executor's context runs on a core: the executor that entered it
//! and the interrupt level it entered at.
//!
//! An executor runs its context at one level: a thread-mode executor at the
//! core's thread level, an interrupt-mode executor at its software
//! interrupt's. A hardware interrupt that preempts it runs at a higher level
//! (the interrupt controller admits only a higher one), so it is never the
//! executor's context, though the executor stays entered beneath it. Only
//! code at the level the executor entered at runs as the executor.

use core::sync::atomic::{AtomicU16, Ordering};

/// No executor's context runs on the core.
pub(crate) const NO_EXECUTOR: u8 = u8::MAX;

/// One core's running executor: its software interrupt and the level it
/// entered at, in one word, so a preempting context reads both at once.
pub(crate) struct Running(AtomicU16);

impl Running {
    pub(crate) const fn new() -> Self {
        Self(AtomicU16::new(u16::from_le_bytes([NO_EXECUTOR, 0])))
    }

    /// Run `poll` as executor `swi`'s context, which the core runs at
    /// interrupt level `level`, and restore the context it preempted.
    pub(crate) fn enter(&self, swi: u8, level: u8, poll: impl FnOnce()) {
        // Relaxed: only this core reads it, and a context that preempts
        // this one restores it before this one runs again.
        let preempted = self
            .0
            .swap(u16::from_le_bytes([swi, level]), Ordering::Relaxed);
        poll();
        self.0.store(preempted, Ordering::Relaxed);
    }

    /// The executor whose context code at interrupt level `level` runs in:
    /// the one that entered at that level, or [`NO_EXECUTOR`] for code that
    /// preempts it at a higher level, or that runs where none entered.
    pub(crate) fn at(&self, level: u8) -> u8 {
        let [swi, entered] = self.0.load(Ordering::Relaxed).to_le_bytes();
        if entered == level { swi } else { NO_EXECUTOR }
    }
}

#[cfg(test)]
mod tests;
