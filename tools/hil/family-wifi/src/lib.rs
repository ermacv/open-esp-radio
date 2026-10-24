//! Wi-Fi HIL acceptance computed from secret-free device, router and air observations.
#![forbid(unsafe_code)]
pub mod evidence;
mod keys;
mod workload;
use oer_hil_model::{catalog::link::PhyExpectation, schema::image::ImageClass};
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

pub const FAMILY: Kind = Kind::of::<WifiScenario>("wifi");
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum WifiScenario {
    W4 {},
}
impl ScenarioFamily for WifiScenario {
    fn validate(&self) -> Result<()> {
        Ok(())
    }
    fn plan(&self) -> Plan {
        let mut plan = Plan::target_only(ImageClass::WifiStation);
        plan.requirements.station_network = true;
        plan.requirements.laptop_air_monitor = true;
        plan.wifi.link = Some(PhyExpectation::Ht20);
        plan.checks = vec!["w4"];
        plan
    }
    fn air_use(&self) -> Vec<AirUse> {
        vec![AirUse::WifiLink]
    }
    fn served_by(&self, keys: &oer_hil_protocol::DeviceImageKeys) -> bool {
        keys.has::<oer_hil_protocol::wifi::StartStation>()
            && keys.has::<oer_hil_protocol::wifi::ProbeStation>()
            && keys.has::<oer_hil_protocol::wifi::StopStation>()
    }
}
impl Workload for WifiScenario {
    fn precondition(&self, lab: &oer_hil_lab::config::LabConfig) -> Result<()> {
        match &lab.station_fixture {
            oer_hil_lab::config::StationFixtureConfig::OpenWrt(config) if !config.read_only => {}
            _ => return Err("W4 requires the managed OpenWrt station fixture".into()),
        }
        let (_, proof) = keys::load(lab)?;
        if !proof.complete() {
            return Err("W4 key conformance evidence is incomplete or contradictory".into());
        }
        if let oer_hil_lab::config::StationFixtureConfig::OpenWrt(config) = &lab.station_fixture {
            oer_lab_router::w4::verify_observation_tools(&mut oer_lab_router::ssh::Ssh::new(
                &config.ssh_target,
            )?)?;
        }
        Ok(())
    }
    fn run(&self, output: &Path, context: &Context<'_>, _: &Fixtures) -> Result<()> {
        workload::run(output, context)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn w4_has_a_runtime_station_image_and_claims_the_ap_and_passive_monitor() {
        let scenario: WifiScenario = toml::from_str("kind='w4'").expect("scenario");
        assert_eq!(scenario.plan().image, ImageClass::WifiStation);
        assert!(scenario.plan().requirements.station_network);
        assert!(scenario.plan().requirements.laptop_air_monitor);
        assert!(toml::from_str::<WifiScenario>("kind='w4'\npassword='secret-passphrase'").is_err());
    }
}
