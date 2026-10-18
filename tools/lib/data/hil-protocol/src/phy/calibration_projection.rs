//! The production side of the vendor calibration cross-check, as data: the
//! words of the image's retained calibration under the reviewed `phy_param`
//! relation's projection (calibration roots, parent words, committed state
//! words), computed by the firmware from the calibration it just
//! published, so the host compares them without linking the PHY.

use postcard_schema::Schema;
use serde::{Deserialize, Serialize};

/// Most projection words one reply carries.
pub const PHY_CALIBRATION_PROJECTION_WORDS: usize = 32;

/// Read `count` words of the projection from index `first`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct PhyCalibrationProjectionRequest {
    pub first: u16,
    pub count: u8,
}

/// A window of the projection.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct PhyCalibrationProjectionWords {
    pub first: u16,
    /// Words of the whole projection.
    pub length: u16,
    pub values: heapless::Vec<u16, PHY_CALIBRATION_PROJECTION_WORDS>,
}
