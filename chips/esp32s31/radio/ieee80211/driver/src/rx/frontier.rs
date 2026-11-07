//! Finite ESP32-S31 RX descriptor-frontier ownership.
//!
//! This module owns the prepared/live/halted DMA lifecycle. Descriptor
//! service is `rx::transaction::service` over the live ring. Scan, monitor,
//! station and access-point policy wrap this owner but do not duplicate its
//! hardware state machine.

#![forbid(unsafe_code)]

mod lifecycle;
mod state;

pub use state::{
    ReceiveFrontier, RxFrontierContinuation, RxFrontierError, RxFrontierIntoLiveFailure,
    RxFrontierPhase, RxFrontierSchedulerSnapshot,
};

#[cfg(test)]
mod tests;
