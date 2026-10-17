//! Periodic calibration and power-tracking state machines.

#[cfg(feature = "validation-probes")]
pub mod calibration;
#[cfg(not(feature = "validation-probes"))]
pub(crate) mod calibration;
#[cfg(feature = "validation-probes")]
pub mod i2c;
#[cfg(not(feature = "validation-probes"))]
pub(crate) mod i2c;
pub mod inspection;
pub mod observation;
#[cfg(feature = "validation-probes")]
pub mod parameters;
#[cfg(not(feature = "validation-probes"))]
pub(crate) mod parameters;
#[cfg(feature = "validation-probes")]
pub mod power;
#[cfg(not(feature = "validation-probes"))]
pub(crate) mod power;
#[cfg(feature = "validation-probes")]
pub mod rfpll;
#[cfg(not(feature = "validation-probes"))]
pub(crate) mod rfpll;
pub mod schedule;

#[cfg(feature = "validation-probes")]
pub mod temperature;
#[cfg(not(feature = "validation-probes"))]
pub(crate) mod temperature;

/// Terminal shared-PHY policy and its required hardware postcondition.
pub mod fail_stop;

// Value results of the tracking graph that protocol owners report. The
// graph's transitions and bindings stay crate-private.
pub use parameters::{CalibrationProgress, PhyParamTrackRequest, PhyParamTrackingOutcome};
pub use rfpll::Observation as RfpllObservation;
