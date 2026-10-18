//! The comparison port: the chip's half of the cross-check.
//!
//! Both sides' boots are recorded as typed observations
//! (`oer-phy-calibration-capture`); what a chip compares, and which register
//! images both sides read, is the chip's. A chip-specific composition
//! implements [`Comparison`]; this crate names no chip.
use std::path::Path;

use oer_phy_calibration_capture::boots::{Images, Lifecycle, ProductionBoot, VendorBoot};
use oer_phy_calibration_capture::space::Register;

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

/// One chip's comparison of the vendor and production calibrations.
pub trait Comparison: Clone + std::fmt::Debug + Default + Eq + Send + Sync + 'static {
    /// The chip whose boards it compares.
    const CHIP: &'static str;
    /// The calibration object the vendor calibration firmware reports.
    const VENDOR_OBJECT: &'static str;

    /// The register images of the tree at `root`.
    fn images(root: &Path) -> Result<RegisterImages>;

    /// Compare the `vendor` and `production` boots captured at `lifecycle`
    /// with `images` on both sides.
    fn compare(
        lifecycle: Lifecycle,
        identity: &Images,
        vendor: &[VendorBoot],
        production: &[ProductionBoot],
        images: &RegisterImages,
    ) -> Result<Compared>;
}
