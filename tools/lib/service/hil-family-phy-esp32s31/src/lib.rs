//! The ESP32-S31 implementation of the PHY cross-check's comparison port
//! (`oer-hil-family-phy`): the chip's vendor calibration comparison
//! (`oer-esp32s31-phy-vendor-calibration`). The family itself names no chip.
#![forbid(unsafe_code)]

use std::path::Path;

use oer_esp32s31_phy_vendor_calibration::{compare, registers};
use oer_hil_family_phy::comparison::{Compared, Comparison, RegisterImages};
use oer_phy_calibration_capture::boots::{Images, Lifecycle, ProductionBoot, VendorBoot};

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
            analog: registers::analog(root, registers::ANALOG_DOMAIN)?,
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
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..");
        let images = Esp32s31::images(&root).unwrap();
        assert!(!images.registers.is_empty() && !images.analog.is_empty());
    }
}
