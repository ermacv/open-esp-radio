//! Model-versus-hardware conformance of the IEEE 802.11 lower-MAC port.
#![forbid(unsafe_code)]
mod workload;
use oer_hil_model::schema::image::ImageClass;
use oer_hil_scenario::{
    AirUse, Plan, Result, ScenarioFamily,
    workload::{
        context::Context,
        family::{Kind, Workload},
        fixture::Fixtures,
    },
};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const FAMILY: Kind = Kind::of::<LowerMacScenario>("lower-mac");
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum LowerMacScenario {
    Conformance {},
}
impl ScenarioFamily for LowerMacScenario {
    fn validate(&self) -> Result<()> {
        Ok(())
    }
    fn plan(&self) -> Plan {
        let mut plan = Plan::target_only(ImageClass::DiagnosticLowerMac);
        plan.requirements.station_network = true;
        plan.checks = vec!["lower-mac-conformance"];
        plan
    }
    fn served_by(&self, keys: &oer_hil_protocol::DeviceImageKeys) -> bool {
        keys.has::<oer_hil_protocol::lower_mac::Prepare>()
            && keys.has::<oer_hil_protocol::lower_mac::RunCase>()
    }
    fn air_use(&self) -> Vec<AirUse> {
        vec![AirUse::Band2G4]
    }
}
impl Workload for LowerMacScenario {
    fn precondition(&self, lab: &oer_hil_lab::config::LabConfig) -> Result<()> {
        match &lab.station_fixture {
            oer_hil_lab::config::StationFixtureConfig::OpenWrt(config) if !config.read_only => {
                Ok(())
            }
            _ => Err("lower-MAC conformance requires the managed laboratory OpenWrt AP".into()),
        }
    }
    fn run(&self, output: &Path, context: &Context<'_>, _: &Fixtures) -> Result<()> {
        workload::run(output, context)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn conformance_has_its_own_diagnostic_image_and_claims_the_ap() {
        let scenario: LowerMacScenario = toml::from_str("kind='conformance'").unwrap();
        scenario.validate().unwrap();
        assert_eq!(scenario.plan().image, ImageClass::DiagnosticLowerMac);
        assert!(scenario.plan().requirements.station_network);
        assert!(
            toml::from_str::<LowerMacScenario>("kind='conformance'\nimage='boot-smoke'").is_err()
        );
    }
}
