//! The manually selected `[phy]` calibration investigation.
use crate::{Result, calibration_capture::boots::Lifecycle, comparison::Comparison};
use oer_hil_lab::config::LabConfig;
use oer_hil_model::schema::image::ImageClass;
use oer_hil_scenario::{
    AirUse, Plan, ScenarioFamily,
    workload::{context::Context, family::Workload, fixture::Fixtures},
};
use serde::{Deserialize, Serialize};
use std::{marker::PhantomData, path::Path};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Window {
    pub address: u32,
    pub words: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "kebab-case",
    deny_unknown_fields,
    bound = ""
)]
pub enum PhyScenario<C: Comparison> {
    TrackingHold {
        ticks: u8,
    },
    VendorCalibration {
        lifecycle: Lifecycle,
        boots: u16,
        #[serde(default)]
        vendor_only: bool,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        vendor_windows: Vec<Window>,
        #[serde(default)]
        vendor_transmit: bool,
        #[serde(skip)]
        comparison: PhantomData<C>,
    },
}
impl<C: Comparison> ScenarioFamily for PhyScenario<C> {
    fn validate(&self) -> Result<()> {
        let (lifecycle, boots, vendor_windows, vendor_transmit) = match self {
            Self::TrackingHold { ticks } => {
                return if (1..=oer_hil_protocol::phy::HOLD_TICKS as u8).contains(ticks) {
                    Ok(())
                } else {
                    Err("hold ticks must be in 1..=16".into())
                };
            }
            Self::VendorCalibration {
                lifecycle,
                boots,
                vendor_windows,
                vendor_transmit,
                ..
            } => (lifecycle, boots, vendor_windows, vendor_transmit),
        };
        if *boots == 0 || *boots > 255 {
            return Err("PHY boots must be in 1..=255".into());
        }
        if (!vendor_windows.is_empty() || *vendor_transmit) && !lifecycle.ieee802154() {
            return Err("vendor windows and transmission need an IEEE 802.15.4 point".into());
        }
        for window in vendor_windows {
            if window.address % 4 != 0
                || window.words == 0
                || window.words > 1024
                || window.address.checked_add((window.words - 1) * 4).is_none()
            {
                return Err("a vendor window must be bounded, aligned and nonempty".into());
            }
        }
        Ok(())
    }
    fn plan(&self) -> Plan {
        Plan::target_only(ImageClass::DiagnosticPhy)
    }
    fn served_by(&self, keys: &oer_hil_protocol::DeviceImageKeys) -> bool {
        if matches!(self, Self::TrackingHold { .. }) {
            return keys.has::<oer_hil_protocol::phy::StartTrackingHold>()
                && keys.has::<oer_hil_protocol::phy::GetTrackingHold>()
                && keys.has::<oer_hil_protocol::phy::StopTrackingHold>();
        }
        keys.has::<oer_hil_protocol::phy::PrepareCapture>()
            && keys.has::<oer_hil_protocol::phy::ReadAnalogImage>()
    }
    fn air_use(&self) -> Vec<AirUse> {
        vec![AirUse::Band2G4]
    }
}
impl<C: Comparison> Workload for PhyScenario<C> {
    fn precondition(&self, lab: &LabConfig) -> Result<()> {
        if lab.chip() != C::CHIP {
            return Err(format!("PHY capture targets {}, not {}", C::CHIP, lab.chip()).into());
        }
        self.validate()
    }
    fn run(&self, output: &Path, context: &Context<'_>, _: &Fixtures) -> Result<()> {
        let (lifecycle, boots, vendor_only, vendor_windows, vendor_transmit) = match self {
            Self::TrackingHold { ticks } => {
                return crate::workload::hold::run(*ticks, output, context);
            }
            Self::VendorCalibration {
                lifecycle,
                boots,
                vendor_only,
                vendor_windows,
                vendor_transmit,
                ..
            } => (
                lifecycle,
                boots,
                vendor_only,
                vendor_windows,
                vendor_transmit,
            ),
        };
        crate::workload::calibration::run::<C>(
            *lifecycle,
            *boots,
            *vendor_only,
            vendor_windows,
            *vendor_transmit,
            output,
            context,
        )
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::calibration_capture::boots::{Images, ProductionBoot, VendorBoot};
    use crate::comparison::{Compared, RegisterImages};
    #[derive(Clone, Debug, Default, Eq, PartialEq)]
    struct Fake;
    impl Comparison for Fake {
        const CHIP: &'static str = "chip-x";
        const VENDOR_OBJECT: &'static str = "phy_param";
        fn images(_: &Path) -> Result<RegisterImages> {
            Err("no device".into())
        }
        fn compare(
            _: Lifecycle,
            _: &Images,
            _: &[VendorBoot],
            _: &[ProductionBoot],
            _: &RegisterImages,
        ) -> Result<Compared> {
            Err("no device".into())
        }
    }
    fn parse(text: &str) -> Result<PhyScenario<Fake>> {
        Ok(toml::from_str(text)?)
    }
    #[test]
    fn hold_counts_are_bounded_and_wifi_workloads_are_not_exposed() {
        parse("kind='tracking-hold'\nticks=5")
            .unwrap()
            .validate()
            .unwrap();
        for count in [0, 17, 255] {
            assert!(
                parse(&format!("kind='tracking-hold'\nticks={count}"))
                    .unwrap()
                    .validate()
                    .is_err()
            );
        }
        assert!(parse("kind='tracking-hold'\nticks=5\nwidth='ht40'").is_err());
    }
    #[test]
    fn investigations_use_the_diagnostic_image_and_validate_counts() {
        let scenario = parse("kind='vendor-calibration'\nlifecycle='restart'\nboots=10").unwrap();
        scenario.validate().unwrap();
        assert_eq!(scenario.plan().image, ImageClass::DiagnosticPhy);
        for lifecycle in ["ieee802154", "ieee802154-restart"] {
            parse(&format!("kind='vendor-calibration'\nlifecycle='{lifecycle}'\nboots=5\nvendor_transmit=true\nvendor_windows=[{{address=4096,words=2}}]")).unwrap().validate().unwrap();
        }
        assert!(parse("kind='vendor-calibration'\nboots=10").is_err());
        for text in [
            "kind='vendor-calibration'\nboots=0\nlifecycle='cold'",
            "kind='vendor-calibration'\nboots=1\nlifecycle='cold'\nvendor_transmit=true",
        ] {
            assert!(parse(text).unwrap().validate().is_err());
        }
        assert!(
            parse("kind='vendor-calibration'\nboots=10\nlifecycle='cold'\nimage='correctness'")
                .is_err()
        );
    }
}
