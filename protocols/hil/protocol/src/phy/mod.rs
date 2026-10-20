//! Diagnostic calibration capture. No endpoint writes arbitrary registers.

#[cfg(feature = "phy")]
mod capture;
#[cfg(feature = "phy")]
pub use capture::*;

#[cfg(feature = "phy")]
crate::messages! {
    endpoint PrepareCapture = "phy/capture/prepare" => CaptureReady;
    topic CaptureReady = "phy/capture/ready";
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

#[cfg(all(feature = "registry", not(feature = "phy")))]
pub const MESSAGES: &[crate::MessageInfo] = &[];
