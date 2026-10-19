//! The registry dispatches to the family a document names.

use std::{path::Path, sync::Mutex};

use oer_hil_scenario::{AirUse, Plan, Scenario, ScenarioFamily};
use oer_hil_schema::image::ImageClass;
use serde::{Deserialize, Serialize};

use super::*;

static RAN: Mutex<Vec<String>> = Mutex::new(Vec::new());

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct Quiet {
    boots: u8,
}

impl ScenarioFamily for Quiet {
    fn validate(&self) -> crate::Result<()> {
        oer_hil_scenario_catalog::bounded(self.boots, 1, 3, "boots")
    }
    fn plan(&self) -> Plan {
        Plan::target_only(ImageClass::SystemWatchdog)
    }
    fn air_use(&self) -> Vec<AirUse> {
        Vec::new()
    }
}

impl Workload for Quiet {
    fn run(&self, _: &Path, _: &Context<'_>, _: &Fixtures) -> crate::Result<()> {
        RAN.lock().unwrap().push(format!("quiet {}", self.boots));
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct Radio {
    channel: u8,
}

impl ScenarioFamily for Radio {
    fn validate(&self) -> crate::Result<()> {
        Ok(())
    }
    fn plan(&self) -> Plan {
        Plan::target_only(ImageClass::SystemWatchdog)
    }
    fn peer_image(&self) -> Option<PeerImage> {
        Some(PeerImage {
            name: "radio-peer",
            reflash: "flash it",
        })
    }
    fn air_use(&self) -> Vec<AirUse> {
        vec![AirUse::Ieee802154Channel(self.channel)]
    }
}

impl Workload for Radio {
    fn precondition(&self, _: &LabConfig) -> crate::Result<()> {
        Err("no radio peer".into())
    }
    fn run(&self, _: &Path, _: &Context<'_>, _: &Fixtures) -> crate::Result<()> {
        RAN.lock().unwrap().push(format!("radio {}", self.channel));
        Ok(())
    }
}

struct Families;

impl Registry for Families {
    const FAMILIES: &'static [Kind] = &[Kind::of::<Quiet>("quiet"), Kind::of::<Radio>("radio")];
    const FIXTURES: &'static [&'static dyn FixtureProvider] = &[];
}

fn scenario(family: &str) -> crate::Result<Scenario<AnyFamily<Families>>> {
    Scenario::from_toml(
        &format!(
            "schema = 5\nid = \"test\"\ndescription = \"a test\"\nrole = \"investigation\"\n{family}"
        ),
        Path::new("test.toml"),
    )
}

#[test]
fn a_document_dispatches_to_the_family_it_names() {
    let quiet = scenario("[quiet]\nboots = 2\n").unwrap();
    let radio = scenario("[radio]\nchannel = 15\n").unwrap();
    assert_eq!(quiet.family.key(), "quiet");
    assert_eq!(quiet.family.air_use(), Vec::new());
    assert_eq!(quiet.family.peer_image(), None);
    assert_eq!(radio.family.air_use(), vec![AirUse::Ieee802154Channel(15)]);
    assert_eq!(radio.family.peer_image().unwrap().name, "radio-peer");
    let lab = oer_hil_lab::config::LabConfig::for_test();
    assert!(quiet.family.precondition(&lab).is_ok());
    assert_eq!(
        radio.family.precondition(&lab).unwrap_err().to_string(),
        "no radio peer"
    );
    let output = std::env::temp_dir();
    let context = Context::new(&lab, &output);
    let fixtures = Fixtures::default();
    quiet.family.run(&output, &context, &fixtures).unwrap();
    radio.family.run(&output, &context, &fixtures).unwrap();
    let ran = RAN.lock().unwrap().clone();
    assert!(ran.contains(&"quiet 2".to_owned()) && ran.contains(&"radio 15".to_owned()));
    assert_eq!(
        table_of::<Radio>(&radio.family).unwrap(),
        Radio { channel: 15 }
    );
}

#[test]
fn a_document_round_trips_and_compares_by_its_table() {
    let radio = scenario("[radio]\nchannel = 15\n").unwrap();
    let json = serde_json::to_vec(&radio).unwrap();
    let read = Scenario::<AnyFamily<Families>>::from_json(&json).unwrap();
    assert_eq!(read.family, radio.family);
    assert_ne!(
        read.family,
        scenario("[radio]\nchannel = 16\n").unwrap().family
    );
    assert_eq!(
        serde_json::to_value(&radio).unwrap()["radio"]["channel"],
        15
    );
}

#[test]
fn an_unknown_missing_or_second_family_is_refused() {
    assert!(scenario("[wifi]\nkind = \"x\"\n").is_err());
    assert!(scenario("").is_err());
    assert!(scenario("[quiet]\nboots = 1\n[radio]\nchannel = 11\n").is_err());
    assert!(
        scenario("[quiet]\nboots = 9\n").is_err(),
        "the family validates"
    );
    assert!(scenario("[quiet]\nboots = 1\nextra = 2\n").is_err());
}
