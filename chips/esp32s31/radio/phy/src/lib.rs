#![expect(
    clippy::fn_params_excessive_bools,
    clippy::struct_excessive_bools,
    reason = "the PHY's procedures in the one-loop form"
)]
#![expect(
    clippy::cast_possible_truncation,
    reason = "the PHY's procedures as programs"
)]
#![expect(
    clippy::cast_possible_wrap,
    reason = "the PHY's procedures as programs"
)]
#![expect(clippy::cast_sign_loss, reason = "the PHY's procedures as programs")]
#![no_std]
#![forbid(unsafe_code)]
// A forgotten owner never releases what it holds, such as a modem clock lease,
// so radio ownership must end in `Drop`.
#![deny(clippy::mem_forget)]
// The registration and tracking graphs run only through the chip target
// ports, which exist only for `riscv32`. Host builds type-check the graphs and
// their hardware bindings, and tests drive the models, but nothing on the host
// calls every binding. Dead code is enforced by the chip build.
#![cfg_attr(
    not(target_arch = "riscv32"),
    allow(
        dead_code,
        unused_imports,
        reason = "host builds type-check the graphs that only chip target ports drive"
    )
)]
#![cfg_attr(
    test,
    allow(
        clippy::disallowed_methods,
        clippy::disallowed_macros,
        clippy::unreachable,
        clippy::arithmetic_side_effects
    )
)]
#![expect(
    clippy::arithmetic_side_effects,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    reason = "the PHY's procedures as programs"
)]

//! Source-only ESP32-S31 shared RF/PHY ownership frontier.
//!
//! This crate turns a cold radio owner into a registered calibration epoch,
//! tracks it, and closes RF while the registration stays. It deliberately has no dependency on
//! `esp-wifi-sys`, vendor archives, or a radio/Wi-Fi ROM ABI. PAC ownership and
//! semantic register access come from the HAL; protocol roles and executors
//! live above this layer.
//!
//! Start with [`PhyConfig`] and the registration state machine. The PHY has
//! one owner ([`owner`]): it holds the place of the PHY's
//! state, which a registration fills, and the proof of RF open or closed.
//!
//! # Ownership and cancellation
//!
//! Registration, tracking, RF close, and retained wake are hardware
//! transactions. A failure returns its leaf cause and has consumed its proof
//! of RF; the owner can vacate the place, and the PHY's clock modules stay
//! held as the failure left them, so a registration into it ends in its
//! clock's refusal. The lifecycle and
//! tracking futures consume the radio domain and return it with their typed
//! result. Dropping them, including before the first poll, quarantines
//! the owned transaction and records a named fault before reset.
//!
//! Timing types identify raw ticks, microseconds, or absolute deadlines at
//! their owning API. [`HARDWARE_EDGE_LIMIT`] is an observation-attempt limit,
//! not a wall-clock duration. The chip Wi-Fi and Bluetooth drivers are the
//! real target consumers; host tests exercise the pure transitions without
//! claiming RF readiness.

#[cfg(test)]
extern crate std;

/// `$value` placed in the analog field `$field`, checked at build: a value
/// that does not fit the field refuses to compile.
macro_rules! placed {
    ($field:expr, $value:expr) => {
        const {
            match $field.place($value) {
                Some(value) => value,
                None => panic!("the constant fits its analog field"),
            }
        }
    };
}

pub mod executor;
pub mod sar_pilot;
pub mod serial;
#[cfg(target_arch = "riscv32")]
mod target_executor;
#[cfg(target_arch = "riscv32")]
mod target_port;

mod analog;
mod calibration;
mod channel;
mod hardware;
#[cfg(any(target_arch = "riscv32", test))]
mod lifecycle;
mod ordinal;
mod rx;
pub mod state;
pub mod tracking;
mod tx;

mod diagnostic_path;
pub mod owner;

mod size_limits;

/// Vendor RF-calibration version stamped into calibration caches.
pub use analog::rfpll::phy_get_rf_cal_version;
// Value results of registration children that protocol reports and hardware
// evidence carry. The transitions that produce them stay crate-private.
#[cfg(oer_trace)]
pub use calibration::registration::{
    FrequencyCalibrationDiagnostics, ReadyWaitDiagnostics, RfCalibrationDiagnostics,
    RfpllCalibrationPoint,
};
pub use calibration::registration::{
    PhyCalibrationIdentity, PhyCalibrationPath, PhyRegisterBindingError, PhyRegisterFailure,
    PhyRegisterOutcome, PhyRegisterStage,
};
pub use executor::{PhyCalibrationTrackingRunError, PhyParamTrackingRunError, PhyRegisterRunError};
pub use state::{
    CalibrationThreshold, PHY_CALIBRATION_SNAPSHOT_SCHEMA, PhyBluetoothCalibration,
    PhyCalibrationCache, PhyCalibrationSnapshot, PhyCommonCalibration, PhyConfig, PhyState,
    PhyWifiCalibration,
};
pub use tx::power::{PhyTxTargetPowerPair, PhyTxTargetPowerProfile};
pub use {
    analog::{
        dcode::PhyDcodeOutcome, pbus::PhyPbusClearOutcome, temperature::PhyTemperatureOutcome,
    },
    channel::PhyChipChannelFailure,
    rx::{
        gain::PhyRxGainInitOutcome,
        gain_calibration::{FINE_CODES as PHY_RX_FINE_DC_CODES, PhyRxGainDcQuality},
    },
    tracking::calibration::{PhyCalibrationTrackingFailure, PhyCalibrationTrackingTransitionError},
    tx::dc_power_detector::PhyTxDcPwdetOutcome,
};
/// Shared finite observation/attempt bound used by target executors and host
/// checks of typed timeout paths. This is not a microsecond duration: direct
/// readiness sampling and timer-backed bus retries have different costs.
pub const HARDWARE_EDGE_LIMIT: u16 = 10_000;
#[cfg(target_arch = "riscv32")]
pub use executor::wait::PhyShortDelay;
#[cfg(target_arch = "riscv32")]
pub use target_executor::PhyTargetPortError;
#[cfg(target_arch = "riscv32")]
pub use target_executor::RomShortDelay;
#[cfg(target_arch = "riscv32")]
pub use target_port::{
    ChannelFailure, CloseFailure, Registered, RegistrationCause, RegistrationFailure,
    TrackingFailure, WakeFailure, close, disable_wifi_rx, enable_wifi_rx, register, select_channel,
    switch_channel, track, wake,
};
#[cfg(target_arch = "riscv32")]
pub use target_port::{
    NoopPhyTargetObserver, PhyGrantProtectPort, PhyParamTrackingPortError, PhyRegisterConfig,
    PhyRegisterFailed, PhyRfBoundary, PhyTargetObserver, PhyTargetPortCounters,
    TargetPhyParamTrackingError, TargetPhyRegisterError,
};

#[cfg(test)]
mod test_clocks;
