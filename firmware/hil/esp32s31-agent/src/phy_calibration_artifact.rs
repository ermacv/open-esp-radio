//! The HIL wire representation of retained PHY calibration, stored in the
//! startup artifact buffer.

pub use oer_esp32s31_hil_calibration_artifact::{decode, encode};

pub const MAX_ENCODED_LEN: usize = crate::console::STARTUP_ARTIFACT_CAPACITY;
