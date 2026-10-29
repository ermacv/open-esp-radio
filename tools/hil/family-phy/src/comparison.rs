//! The comparison port: the chip's half of the cross-check.
//!
//! Both sides' boots are recorded as typed observations
//! (`oer_hil_family_phy::calibration_capture`); what a chip compares, and which register
//! images both sides read, is the chip's. A chip-specific composition
//! implements [`Comparison`]; this crate names no chip.
use std::path::Path;

use crate::calibration_capture::boots::{Images, Lifecycle, ProductionBoot, VendorBoot};
use crate::calibration_capture::space::Register;

use crate::Result;

/// The register images both sides read: the radio-PHY image and the analog
/// image, each in production's index order.
pub struct RegisterImages {
    pub registers: Vec<Register>,
    pub analog: Vec<Register>,
}

/// What a comparison found.
pub struct Compared {
    /// The typed result the repetition records as its `comparison`
    /// observation.
    pub summary: serde_json::Value,
    /// Why the cross-check did not MATCH; `None` when it did.
    pub mismatch: Option<String>,
}

/// A validated recorded reference; the chip owns its document and rule.
pub struct RecordedReference {
    pub vendor_application_sha256: String,
    pub vendor_idf_revision: String,
    pub channel: u8,
    pub readable: Vec<usize>,
    pub document: serde_json::Value,
}

/// One chip's comparison of the vendor and production calibrations.
pub trait Comparison: Clone + std::fmt::Debug + Default + Eq + Send + Sync + 'static {
    /// The chip whose boards it compares.
    const CHIP: &'static str;
    /// The calibration object the vendor calibration firmware reports.
    const VENDOR_OBJECT: &'static str;

    /// The register images of the tree at `root`.
    fn images(root: &Path) -> Result<RegisterImages>;

    /// Only the non-destructive Wi-Fi counter and NF words used by the native MAC.
    /// Geometry is chip-owned; the protocol accepts no addresses from the host.
    fn health_registers(root: &Path) -> Result<Vec<HealthRegister>>;

    fn recorded(root: &Path, board: &str, lifecycle: Lifecycle) -> Result<RecordedReference>;

    fn compare_recorded(
        root: &Path,
        board: &str,
        reference: &RecordedReference,
        identity: &Images,
        production: &[ProductionBoot],
        images: &RegisterImages,
    ) -> Result<Compared>;

    /// Compare the `vendor` and `production` boots captured at `lifecycle`
    /// with `images` on both sides.
    fn compare(
        root: &Path,
        lifecycle: Lifecycle,
        identity: &Images,
        vendor: &[VendorBoot],
        production: &[ProductionBoot],
        images: &RegisterImages,
    ) -> Result<Compared>;
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct HealthRegister {
    pub name: String,
    pub address: u32,
    pub fields: Vec<HealthField>,
}
#[derive(Clone, Debug, serde::Serialize)]
pub struct HealthField {
    pub name: String,
    pub offset: u8,
    pub width: u8,
}
impl HealthField {
    pub fn value(&self, word: u32) -> Option<u32> {
        if self.width == 0 || self.width > 32 || u16::from(self.offset) + u16::from(self.width) > 32
        {
            return None;
        }
        Some((word >> self.offset) & (u32::MAX >> (32 - self.width)))
    }
}
