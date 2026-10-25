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

//! Source-only ESP32-S31 shared RF/PHY ownership frontier.
//!
//! This crate turns a cold radio owner into a registered calibration epoch,
//! admits protocol clients, performs client-specific maintenance, and closes
//! or retains the shared RF domain. It deliberately has no dependency on
//! `esp-wifi-sys`, vendor archives, or a radio/Wi-Fi ROM ABI. PAC ownership and
//! semantic register access come from the HAL; protocol roles and executors
//! live above this layer.
//!
//! Start with [`PhyConfig`] and the registration state machine. Every
//! protocol shares one PHY domain through the radio domain:
//! [`register_concurrent_phy`] registers it for the first client, and
//! [`wifi_client`], [`bluetooth_client`] and [`ieee802154_client`] join and
//! leave it. Releasing a non-final client keeps the domain powered; only the
//! final-client close path closes RF.
//!
//! # Ownership and cancellation
//!
//! Registration, tracking, RF close, and retained wake are hardware
//! transactions. Preparation errors that expose an owner are retryable only
//! through that returned owner. Failures after a physical transition retain a
//! poisoned/fail-stop owner instead of claiming recovery. Once an async close,
//! wake, or maintenance future has been polled across its hardware edge, the
//! caller must drive it to a terminal result; dropping it does not reconstruct
//! the preceding typestate.
//!
//! Timing types identify raw ticks, microseconds, or absolute deadlines at
//! their owning API. [`HARDWARE_EDGE_LIMIT`] is an observation-attempt limit,
//! not a wall-clock duration. The chip Wi-Fi and Bluetooth drivers are the
//! real target consumers; host tests exercise the pure transitions without
//! claiming RF readiness.

#[cfg(test)]
extern crate std;

pub mod executor;
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
mod rx;
pub mod state;
pub mod tracking;
mod tx;

pub mod bluetooth_client;
pub mod concurrent;
mod diagnostic_path;
pub mod domain;
pub mod ieee802154_client;
mod size_limits;
pub mod wifi_client;

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
    PhyRegisterOutcome, PhyRegisterStage, RegisteredPhyState,
};
pub use executor::{PhyCalibrationTrackingRunError, PhyParamTrackingRunError, PhyRegisterRunError};
pub use state::{
    PHY_CALIBRATION_SNAPSHOT_SCHEMA, PhyBluetoothCalibration, PhyCalibrationCache,
    PhyCalibrationSnapshot, PhyCommonCalibration, PhyConfig, PhyState, PhyWifiCalibration,
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
pub use oer_esp32s31_hal::phy::delay::RomShortDelay;
#[cfg(target_arch = "riscv32")]
pub use target_executor::PhyTargetPortError;
#[cfg(target_arch = "riscv32")]
pub use target_port::{
    ConcurrentPhyRegisterFailure, ConcurrentPhyRegistration, ConcurrentPhyTrackingError,
    ConcurrentRfError, ConcurrentTrackingTick, ConcurrentWifiChannelError, close_concurrent_rf,
    maintain_concurrent_phy, register_concurrent_phy, select_concurrent_wifi_channel,
    switch_concurrent_wifi_channel, track_concurrent_phy, wake_concurrent_rf,
};
#[cfg(target_arch = "riscv32")]
pub use target_port::{
    NoopPhyTargetObserver, PhyDomainRegisterFailure, PhyDomainRegistered, PhyGrantProtectPort,
    PhyParamTrackingPortError, PhyRegisterConfig, PhyRfBoundary, PhyTargetObserver,
    PhyTargetPortCounters, TargetPhyParamTrackingError, TargetPhyRegisterError,
};

#[cfg(test)]
mod test_clocks;
