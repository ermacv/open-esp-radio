//! Periodic calibration and power-tracking state machines.

#[cfg(oer_probe)]
pub mod calibration;
#[cfg(not(oer_probe))]
pub(crate) mod calibration;
#[cfg(oer_probe)]
pub mod i2c;
#[cfg(not(oer_probe))]
pub(crate) mod i2c;
pub mod inspection;
pub mod observation;
#[cfg(oer_probe)]
pub mod parameters;
#[cfg(not(oer_probe))]
pub(crate) mod parameters;
pub(crate) mod power;
#[cfg(oer_probe)]
pub mod rfpll;
#[cfg(not(oer_probe))]
pub(crate) mod rfpll;
pub mod schedule;

/// The temperature band Wi-Fi I2C tracking last applied, which
/// [`crate::PhyState::wifi_i2c_tracking_parameters`] reports.
pub use i2c::PhyWifiI2cTrackingBand;

#[cfg(oer_probe)]
pub mod temperature;
#[cfg(not(oer_probe))]
pub(crate) mod temperature;

/// Terminal shared-PHY policy and its required hardware postcondition.
pub mod fail_stop;

// Value results of the tracking graph that protocol owners report. The
// graph's transitions and bindings stay crate-private.
pub use parameters::{CalibrationProgress, PhyParamTrackRequest, PhyParamTrackingOutcome};
pub use rfpll::Observation as RfpllObservation;
pub use temperature::{
    Acquisition as TemperatureAcquisition, Observation as TemperatureObservation,
};
