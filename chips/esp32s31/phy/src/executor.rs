//! Allocation-free async driver for the Rust-owned PHY registration graph.
//!
//! This module deliberately does not know how an ESP executor represents a
//! timer or interrupt future. The board integration owns that policy through
//! the wait vocabulary in [`wait`] and the delay it supplies to the target
//! ports, so the graphs run under Embassy, a custom interrupt executor, or a
//! test harness without importing an RTOS. The graph drivers and their port
//! traits are crate-private.

pub mod wait;

#[cfg(any(target_arch = "riscv32", test))]
pub(crate) mod step;

#[cfg(any(target_arch = "riscv32", test))]
pub(crate) mod rx;

// The graph drivers and their port traits are model-level API, reached only
// through the crate's target ports and registered owners.
mod run;

pub(crate) use run::{
    PhyCalibrationTrackingPort, PhyParamTrackingPort, PhyRegisterPort,
    run_phy_calibration_tracking, run_phy_param_tracking, run_phy_register,
};
pub use run::{PhyCalibrationTrackingRunError, PhyParamTrackingRunError, PhyRegisterRunError};

#[cfg(test)]
mod observation_tests;
#[cfg(test)]
pub(crate) mod test_clock;
#[cfg(test)]
mod tests;
