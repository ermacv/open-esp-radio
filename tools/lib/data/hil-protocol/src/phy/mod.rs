//! The PHY module: fault injection, tracking control, register images and
//! the startup calibration artifact, with the properties PHY images
//! advertise.

use postcard_schema::Schema;
use serde::{Deserialize, Serialize};

mod artifact;
pub use artifact::*;
mod calibration_projection;
pub use calibration_projection::*;
mod fault;
pub use fault::*;
mod register_image;
pub use register_image::*;
mod tracking_hold;
pub use tracking_hold::*;

crate::messages! {
    /// Destructive checkpoints in actual PHY maintenance; diagnostic only.
    property FaultInjection = "phy/fault-injection";
    /// Windows of the radio-PHY register image and of the analog image.
    property RegisterImage = "phy/register-image";
    /// One opaque, host-owned startup artifact, returned after
    /// initialization.
    property StartupArtifact = "phy/startup-artifact";
    /// The direct source-owned RX-gain transaction executes from internal
    /// SRAM.
    property RxHotSram = "phy/rx-hot-sram";
    /// One forced tracking tick can be timed on request
    /// (`phy/tracking/hold/measure`); diagnostic only.
    property TrackingHoldProbe = "phy/tracking-hold";
    endpoint ControlFault = "phy/fault/control" => crate::phy::FaultState;
    topic FaultState = "phy/fault/state";
    endpoint ControlTracking = "phy/tracking/control" => crate::phy::TrackingState;
    topic TrackingState = "phy/tracking/state";
    endpoint MeasureTrackingHold = "phy/tracking/hold/measure" => crate::phy::TrackingHold;
    topic TrackingHold = "phy/tracking/hold";
    endpoint ReadRegisterImage = "phy/register-image/read" => crate::phy::RegisterImageWords;
    topic RegisterImageWords = "phy/register-image/words";
    endpoint ReadAnalogImage = "phy/analog-image/read" => crate::phy::AnalogImageBytes;
    topic AnalogImageBytes = "phy/analog-image/bytes";
    endpoint ReadCalibrationProjection = "phy/calibration-projection/read" => crate::phy::CalibrationProjectionWords;
    topic CalibrationProjectionWords = "phy/calibration-projection/words";
    endpoint UploadStartupArtifact = "phy/startup-artifact/upload" => crate::base::Accepted;
    topic StartupArtifactReady = "phy/startup-artifact/ready";
    topic StartupArtifactPart = "phy/startup-artifact/part";
}

/// `phy/fault/control`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct ControlFault(pub crate::phy::PhyFaultCommand);

/// `phy/fault/state`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct FaultState(pub crate::phy::PhyFaultEvidence);

/// Suspend, resume or report the shared PHY's periodic tracking timer.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct ControlTracking(pub crate::phy::PhyTrackingCommand);

/// `phy/tracking/state`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct TrackingState(pub crate::phy::PhyTrackingEvidence);

/// Time one forced tracking tick while the periodic timer is suspended.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct MeasureTrackingHold(pub crate::phy::PhyTrackingHoldRequest);

/// `phy/tracking/hold`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct TrackingHold(pub crate::phy::PhyTrackingHoldEvidence);

/// Read a window of the radio-PHY register image without effect.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct ReadRegisterImage(pub crate::phy::PhyRegisterImageRequest);

/// Correlated response to its request.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct RegisterImageWords(pub crate::phy::PhyRegisterImageWords);

/// Read a window of the analog image: one analog-I2C read per register.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct ReadAnalogImage(pub crate::phy::PhyRegisterImageRequest);

/// Correlated response to its request.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct AnalogImageBytes(pub crate::phy::PhyAnalogImageBytes);

/// Read a window of the projection of the calibration the image published
/// as its startup artifact.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct ReadCalibrationProjection(pub crate::phy::PhyCalibrationProjectionRequest);

/// Correlated response to its request.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct CalibrationProjectionWords(pub crate::phy::PhyCalibrationProjectionWords);

/// `phy/startup-artifact/upload`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct UploadStartupArtifact(pub crate::phy::StartupArtifactChunk);

/// `phy/startup-artifact/ready`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct StartupArtifactReady(pub crate::phy::StartupArtifactStatus);

/// `phy/startup-artifact/part`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct StartupArtifactPart(pub crate::phy::StartupArtifactChunk);
