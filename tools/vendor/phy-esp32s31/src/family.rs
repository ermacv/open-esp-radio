//! The ESP32-S31 implementation of the PHY cross-check's comparison port
//! (`oer-hil-family-phy`): the chip's vendor calibration comparison
//! (`oer-vendor-phy-esp32s31`). The family itself names no chip.
#![forbid(unsafe_code)]

use std::path::Path;

use crate::{compare, registers};
use oer_vendor_phy::calibration_capture::boots::{Images, Lifecycle, ProductionBoot, VendorBoot};
use oer_vendor_phy::comparison::{Compared, Comparison, RegisterImages};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// The ESP32-S31's comparison: the reviewed `phy_param` relation and
/// tolerances over the published radio-PHY partition and analog-I2C images.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Esp32s31;

impl Comparison for Esp32s31 {
    const CHIP: &'static str = "esp32s31";
    const VENDOR_OBJECT: &'static str = compare::VENDOR_OBJECT;

    fn images(root: &Path) -> Result<RegisterImages> {
        Ok(RegisterImages {
            registers: registers::partition(root, registers::PARTITION)?,
            analog: registers::analog(root)?,
        })
    }

    fn health_registers(root: &Path) -> Result<Vec<oer_vendor_phy::comparison::HealthRegister>> {
        registers::health(root)
    }

    fn recorded(
        root: &Path,
        board: &str,
        lifecycle: Lifecycle,
    ) -> Result<oer_vendor_phy::comparison::RecordedReference> {
        if lifecycle != Lifecycle::Cold {
            return Err("no recorded reference at this lifecycle".into());
        }
        let signature = crate::baseline::Signature::current(root)?;
        let reference = signature.read_reference(root)?;
        let images = Self::images(root)?;
        reference.validate(&signature, &images, board, 0)?;
        Ok(oer_vendor_phy::comparison::RecordedReference {
            vendor_application_sha256: reference.provenance.vendor_application_sha256.clone(),
            vendor_idf_revision: reference.provenance.idf_revision.clone(),
            channel: reference.channel,
            readable: reference.readable(board, &images)?,
            document: serde_json::to_value(reference)?,
        })
    }

    fn compare_recorded(
        root: &Path,
        board: &str,
        reference: &oer_vendor_phy::comparison::RecordedReference,
        identity: &Images,
        production: &[ProductionBoot],
        images: &RegisterImages,
    ) -> Result<Compared> {
        let reference: crate::baseline::Reference =
            serde_json::from_value(reference.document.clone())?;
        let checked = reference.check(
            &crate::baseline::Signature::current(root)?,
            images,
            board,
            0,
            identity,
            production,
        );
        Ok(Compared {
            mismatch: (checked.verdict != compare::Verdict::Match)
                .then(|| format!("{:?}", checked.verdict)),
            summary: serde_json::to_value(checked)?,
        })
    }

    fn compare(
        lifecycle: Lifecycle,
        identity: &Images,
        vendor: &[VendorBoot],
        production: &[ProductionBoot],
        images: &RegisterImages,
    ) -> Result<Compared> {
        let summary = compare::compare(
            lifecycle,
            identity,
            vendor,
            production,
            compare::ReviewFile::reviewed()?,
            (&images.registers, &images.analog),
        )?;
        let mismatch = (summary.verdict != compare::Verdict::Match).then(|| {
            format!(
                "{:?}: {} of {} registers and {} of {} analog registers matched",
                summary.verdict,
                summary.registers.matched,
                summary.registers.compared,
                summary.analog.matched,
                summary.analog.compared,
            )
        });
        Ok(Compared {
            summary: serde_json::to_value(&summary)?,
            mismatch,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_composition_reads_the_published_images_of_its_chip() {
        let root = crate::registers::repository();
        let images = Esp32s31::images(&root).unwrap();
        assert!(!images.registers.is_empty() && !images.analog.is_empty());
    }
}

/// The executable S31 comparison family; the generic workload names no chip.
pub const FAMILY: oer_hil_scenario::workload::family::Kind =
    oer_hil_scenario::workload::family::Kind::of::<oer_vendor_phy::scenario::PhyScenario<Esp32s31>>(
        "phy",
    );
