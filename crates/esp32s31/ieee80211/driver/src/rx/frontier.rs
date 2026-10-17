//! Finite ESP32-S31 RX descriptor-frontier ownership.
//!
//! This module owns prepared/live/halted DMA lifecycle and finite descriptor
//! service. Scan, monitor, station and access-point policy wrap this owner but
//! do not duplicate its hardware state machine.

#![forbid(unsafe_code)]

mod lifecycle;
mod state;

pub use state::{
    ReceiveFrontier, RxFrontierContinuation, RxFrontierDirective, RxFrontierError,
    RxFrontierIntoLiveFailure, RxFrontierPhase, RxFrontierProgress, RxFrontierSchedulerSnapshot,
    RxFrontierServiceProgress,
};

#[cfg(test)]
mod tests;
