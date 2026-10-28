#![no_std]
#![forbid(unsafe_code)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros,
    clippy::disallowed_methods
)]
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic,
        clippy::unreachable,
        clippy::arithmetic_side_effects,
        clippy::disallowed_macros,
        clippy::disallowed_methods
    )
)]
//! Diagnostic calibration capture and opaque calibration-cache exchange. No endpoint writes arbitrary registers.

mod cache;
pub use cache::*;

mod capture;
pub use capture::*;

mod hold;
pub use hold::*;

oer_hil_protocol::messages! {
    endpoint GetCalibrationCacheIdentity = "phy/cache/identity/get" => CalibrationCacheIdentity;
    topic CalibrationCacheIdentity = "phy/cache/identity";
    endpoint StartCalibration = "phy/calibration/start" => CalibrationStarted;
    topic CalibrationStarted = "phy/calibration/started";
    endpoint WriteCalibrationCachePage = "phy/cache/write" => CalibrationCachePageWritten;
    topic CalibrationCachePageWritten = "phy/cache/written";
    endpoint CompleteCalibrationCache = "phy/cache/complete" => CalibrationCacheCompleted;
    topic CalibrationCacheCompleted = "phy/cache/completed";
    endpoint GetCalibrationStartStatus = "phy/calibration/status/get" => CalibrationStartStatus;
    topic CalibrationStartStatus = "phy/calibration/status";
    endpoint ReadCalibrationSnapshot = "phy/cache/read" => CalibrationSnapshot;
    topic CalibrationSnapshot = "phy/cache/snapshot";
    topic CalibrationCacheRequested = "phy/cache/requested";
    property HostCalibrationCache = "phy/cache/host";
    endpoint StartTrackingHold = "phy/hold/start" => HoldStarted;
    topic HoldStarted = "phy/hold/started";
    endpoint GetTrackingHold = "phy/hold/get" => TrackingHold;
    topic TrackingHold = "phy/hold/samples";
    endpoint StopTrackingHold = "phy/hold/stop" => HoldStopped;
    topic HoldStopped = "phy/hold/stopped";
    endpoint RestartCaptureRadio = "phy/capture/restart" => CaptureRestarted;
    topic CaptureRestarted = "phy/capture/restarted";
    endpoint StopCapture = "phy/capture/stop" => CaptureStopped;
    topic CaptureStopped = "phy/capture/stopped";
    endpoint ReadCalibrationProjection = "phy/capture/calibration/read" => CalibrationProjection;
    topic CalibrationProjection = "phy/capture/calibration";
    endpoint ReadRegisterImage = "phy/capture/registers/read" => RegisterImage;
    topic RegisterImage = "phy/capture/registers";
    endpoint ReadAnalogImage = "phy/capture/analog/read" => AnalogImage;
    topic AnalogImage = "phy/capture/analog";
}

/// Reviewed paths, schemas (through keys) and message kinds.
#[cfg(feature = "registry")]
pub const MESSAGES_LOCK: &str = include_str!("../messages.lock");
