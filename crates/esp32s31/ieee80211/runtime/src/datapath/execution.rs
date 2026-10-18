//! Event-driven stop request for one connected execution epoch.
//!
//! [`super::DatapathRunner::run_controlled`] drains active TX before acting
//! on the stop; the stop retains ordinary prepared-TX cancellation and link
//! shutdown semantics.
//!
//! The S31 connected child task uses this interface. A stop result is a
//! scheduler boundary, not MAC/DMA/IRQ or RF quiescence.

use core::sync::atomic::{AtomicBool, Ordering};
use embassy_sync::{blocking_mutex::raw::RawMutex, signal::Signal};

/// One connection's stop request. The stop remains latched until this owner
/// is discarded. A new connection uses a new control owner rather than
/// clearing a live request channel.
pub struct Control<M: RawMutex> {
    stop: AtomicBool,
    wake: Signal<M, ()>,
}

impl<M: RawMutex> Control<M> {
    pub const fn new() -> Self {
        Self {
            stop: AtomicBool::new(false),
            wake: Signal::new(),
        }
    }

    pub fn request_stop(&self) {
        if !self.stop.swap(true, Ordering::AcqRel) {
            self.wake.signal(());
        }
    }

    pub fn stop_requested(&self) -> bool {
        self.stop.load(Ordering::Acquire)
    }

    pub(super) async fn wait_stop(&self) {
        while !self.stop_requested() {
            self.wake.wait().await;
        }
    }
}

impl<M: RawMutex> Default for Control<M> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
