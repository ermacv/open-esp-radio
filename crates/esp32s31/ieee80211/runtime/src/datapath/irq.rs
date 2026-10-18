//! Embassy wake adapter for the executor-neutral S31 interrupt state.
//!
//! Hardware acknowledgement and interrupt classification remain in the chip
//! MAC crate. This facade exposes only executor wake runtimes and the finite
//! platform-route epoch owner.

mod epoch;
mod mac_runtime;
mod power_runtime;

pub use epoch::{
    InterruptEpoch, MacInterruptEpochActivateError, MacInterruptEpochDrain,
    MacInterruptEpochQuiesceError, MacInterruptEpochStateError,
};

pub use mac_runtime::{EmbassyMacIrqDrain, EmbassyMacIrqRuntime};

pub use power_runtime::EmbassyPowerIrqRuntime;

#[cfg(test)]
mod tests;
