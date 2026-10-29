//! Wi-Fi HIL acceptance computed from secret-free device, router and air observations.
#![forbid(unsafe_code)]
mod diagnostic;
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
    InitialGtkDiagnostic {},
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
        plan.checks = match self {
            Self::W4 {} => vec!["w4"],
            Self::InitialGtkDiagnostic {} => vec!["initial-gtk-diagnostic"],
        };
        plan
    }
    fn forbidden_keys(&self) -> &'static [oer_hil_protocol::Key] {
        &[]
    }
    fn air_use(&self) -> Vec<AirUse> {
        vec![AirUse::WifiLink]
    }
    fn required_keys(&self) -> &'static [oer_hil_protocol::Key] {
        oer_ieee80211_control::KEYS
    }
}
impl Workload for WifiScenario {
    fn precondition(&self, lab: &oer_hil_session::config::LabConfig) -> Result<()> {
        match &lab.station_fixture {
            oer_hil_session::config::StationFixtureConfig::OpenWrt(config) if !config.read_only => {
            }
            _ => return Err("W4 requires the managed OpenWrt station fixture".into()),
        }
        if matches!(self, Self::InitialGtkDiagnostic {}) {
            return Ok(());
        }
        let (_, proof) = keys::load(lab)?;
        if !proof.complete() {
            return Err("W4 key conformance evidence is incomplete or contradictory".into());
        }
        // Router tool admission runs with the repetition's command recorder,
        // before any configuration change, so even failures retain stderr.
        Ok(())
    }
    fn run(&self, output: &Path, context: &Context<'_>, _: &Fixtures) -> Result<()> {
        match self {
            Self::W4 {} => workload::run(output, context),
            Self::InitialGtkDiagnostic {} => diagnostic::run(output, context),
        }
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
