//! The `[phy]` scenario table.

use std::marker::PhantomData;
use std::path::Path;

use oer_tool_data_hil_scenario_catalog::bounded;
use oer_tool_data_hil_schema::image::ImageClass;
use oer_tool_service_hil_lab::config::LabConfig;
use oer_tool_service_hil_scenario::{AirUse, Plan, ScenarioFamily};
use oer_tool_service_hil_workload::context::Context;
use oer_tool_service_hil_workload::{family::Workload, fixture::Fixtures};
use oer_tool_service_phy_calibration_capture::boots::Lifecycle;
use serde::{Deserialize, Serialize};

use crate::Result;
use crate::comparison::Comparison;

/// The vendor firmware projects of `chip`'s PHY calibration cross-check,
/// relative to the repository root: its profile's `[hil]
/// vendor-calibration`.
pub fn vendor_projects(chip: &str) -> Result<std::path::PathBuf> {
    oer_tool_data_chip_profile::Profile::load(&oer_tool_base_process::built_root(), chip)?
        .hil
        .and_then(|hil| hil.vendor_calibration)
        .ok_or_else(|| format!("{chip} has no PHY calibration cross-check").into())
}

/// One PHY workload, compared by the chip's comparison `C`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "kebab-case",
    deny_unknown_fields,
    bound = ""
)]
pub enum PhyScenario<C: Comparison> {
    /// The vendor firmware's and the production image's PHY calibration
    /// and register state at one lifecycle point, alternating boot by boot
    /// on the board under test, compared under the reviewed relation.
    VendorCalibration {
        lifecycle: Lifecycle,
        /// Boots per side.
        boots: u8,
        /// The production image class; the lifecycle point's own by default
        /// (`correctness`, or `diagnostic-i-radio` at an IEEE
        /// 802.15.4 point).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        image: Option<ImageClass>,
        /// The vendor firmware project of the chip's vendor calibration
        /// projects; the lifecycle point's own by default.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        vendor_project: Option<String>,
        /// Further device windows the IEEE 802.15.4 reference firmware reads
        /// with `PEEK`: a vendor-side investigation, never compared.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        vendor_windows: Vec<Window>,
        /// After the reads, transmit one frame on the vendor side and read
        /// its analog image and windows again.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        vendor_transmit: bool,
        /// Boot only the vendor firmware: an investigation without a
        /// comparison.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        vendor_only: bool,
        /// The chip's comparison; not part of the table.
        #[serde(skip)]
        comparison: PhantomData<C>,
    },
}

/// `words` consecutive device words from `address`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Window {
    pub address: u32,
    pub words: u32,
}

impl<C: Comparison> PhyScenario<C> {
    /// The production image class the scenario compares.
    pub fn image(&self) -> ImageClass {
        let Self::VendorCalibration {
            lifecycle, image, ..
        } = self;
        image.unwrap_or(if lifecycle.ieee802154() {
            ImageClass::DiagnosticIeee802154Radio
        } else {
            ImageClass::Correctness
        })
    }
}

impl<C: Comparison> ScenarioFamily for PhyScenario<C> {
    fn validate(&self) -> Result<()> {
        let Self::VendorCalibration {
            lifecycle,
            boots,
            vendor_project,
            vendor_windows,
            vendor_transmit,
            ..
        } = self;
        bounded(*boots, 1, 20, "boots")?;
        if vendor_project.as_deref().is_some_and(str::is_empty) {
            return Err("vendor_project names no project".into());
        }
        if (!vendor_windows.is_empty() || *vendor_transmit) && !lifecycle.ieee802154() {
            return Err("vendor windows and transmission need an IEEE 802.15.4 point".into());
        }
        for window in vendor_windows {
            if window.address % 4 != 0 || window.words == 0 {
                return Err("a vendor window needs a word-aligned address and a word".into());
            }
        }
        Ok(())
    }

    fn plan(&self) -> Plan {
        Plan::target_only(self.image())
    }

    /// Cold calibration measures TX and RX DC and IQ that another
    /// transmission on the band would bias.
    fn air_use(&self) -> Vec<AirUse> {
        vec![AirUse::Band2G4]
    }
}

impl<C: Comparison> Workload for PhyScenario<C> {
    fn precondition(&self, lab: &LabConfig) -> Result<()> {
        if lab.chip() != C::CHIP {
            return Err(format!(
                "the PHY calibration cross-check compares {} boards, not {}",
                C::CHIP,
                lab.chip()
            )
            .into());
        }
        vendor_projects(lab.chip()).map(|_| ())
    }

    fn run(&self, output: &Path, context: &Context<'_>, _fixtures: &Fixtures) -> Result<()> {
        let Self::VendorCalibration {
            lifecycle,
            boots,
            vendor_project,
            vendor_windows,
            vendor_transmit,
            vendor_only,
            ..
        } = self;
        crate::workload::calibration::run::<C>(
            &crate::workload::calibration::Calibration {
                lifecycle: *lifecycle,
                boots: *boots,
                image: self.image(),
                vendor_project: vendor_project
                    .as_deref()
                    .unwrap_or(lifecycle.vendor_project()),
                windows: vendor_windows,
                transmit: *vendor_transmit,
                vendor_only: *vendor_only,
            },
            output,
            context,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::comparison::{Compared, RegisterImages};

    /// A comparison of no chip, for the table's own rules.
    #[derive(Clone, Debug, Default, Eq, PartialEq)]
    struct Fake;

    impl Comparison for Fake {
        const CHIP: &'static str = "chip-x";
        const VENDOR_OBJECT: &'static str = "phy_param";

        fn images(_: &Path) -> Result<RegisterImages> {
            Err("no images".into())
        }

        fn compare(
            _: Lifecycle,
            _: &oer_tool_service_phy_calibration_capture::boots::Images,
            _: &[oer_tool_service_phy_calibration_capture::boots::VendorBoot],
            _: &[oer_tool_service_phy_calibration_capture::boots::ProductionBoot],
            _: &RegisterImages,
        ) -> Result<Compared> {
            Err("no comparison".into())
        }
    }

    fn table(text: &str) -> PhyScenario<Fake> {
        toml::from_str(text).unwrap()
    }

    #[test]
    fn the_lifecycle_point_implies_the_image_and_vendor_project() {
        let cold = table("kind = \"vendor-calibration\"\nlifecycle = \"cold\"\nboots = 10\n");
        cold.validate().unwrap();
        assert_eq!(cold.plan().image, ImageClass::Correctness);
        let radio = table(
            "kind = \"vendor-calibration\"\nlifecycle = \"ieee802154-restart\"\nboots = 3\n\
             vendor_windows = [{ address = 0x2010fc00, words = 2 }]\nvendor_transmit = true\n",
        );
        radio.validate().unwrap();
        assert_eq!(radio.plan().image, ImageClass::DiagnosticIeee802154Radio);
        assert_eq!(
            Lifecycle::Ieee802154Restart.vendor_project(),
            "ieee802154-reference"
        );
    }

    #[test]
    fn invalid_tables_are_rejected() {
        let windows_at_cold = table(
            "kind = \"vendor-calibration\"\nlifecycle = \"cold\"\nboots = 1\n\
             vendor_windows = [{ address = 0x2010fc00, words = 2 }]\n",
        );
        assert!(windows_at_cold.validate().is_err());
        let unaligned = table(
            "kind = \"vendor-calibration\"\nlifecycle = \"ieee802154\"\nboots = 1\n\
             vendor_windows = [{ address = 0x2010fc02, words = 2 }]\n",
        );
        assert!(unaligned.validate().is_err());
        assert!(
            table("kind = \"vendor-calibration\"\nlifecycle = \"cold\"\nboots = 0\n")
                .validate()
                .is_err()
        );
        assert!(
            toml::from_str::<PhyScenario<Fake>>(
                "kind = \"vendor-calibration\"\nlifecycle = \"cold\"\nboots = 1\ncolor = 1\n"
            )
            .is_err()
        );
    }
}
