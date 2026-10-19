//! The ESP32-C5 radio root, acquired only through the platform handoff.

pub use oer_esp32c5_pac::RadioHandoff;
use oer_esp32c5_pac::RadioPartitions;

/// Unique radio root following the platform's ownership handoff.
#[must_use = "dropping the root loses its radio register authority"]
pub struct RadioHardware {
    partitions: RadioPartitions,
}

impl RadioHardware {
    pub fn from_handoff(handoff: RadioHandoff) -> Self {
        Self {
            partitions: RadioPartitions::from_handoff(handoff),
        }
    }

    /// Transfer every partition to its protocol or shared-radio owner.
    pub fn into_partitions(self) -> RadioPartitions {
        self.partitions
    }
}
