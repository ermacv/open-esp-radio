//! The executable scenario catalog: the common envelope over exactly one of
//! the radio families this runner composes. This module is the runner's
//! composition: [`Families`] lists the families and fixture providers, and
//! the core reaches every family only through it.

use oer_hil_model::catalog::requirements::Requirements;
use oer_hil_scenario::ScenarioFamily as _;
use oer_hil_scenario::workload::family::{AnyFamily, FixtureProvider, Kind, Registry};

/// The radio families and fixture providers of this runner.
pub(crate) struct Families;

impl Registry for Families {
    const FAMILIES: &'static [Kind] = &[
        oer_hil_family_system::FAMILY,
        oer_hil_family_lower_mac::FAMILY,
        oer_vendor_phy_esp32s31::family::FAMILY,
    ];
    const FIXTURES: &'static [&'static dyn FixtureProvider] = &[];
}

/// The single family table of a scenario document.
pub(crate) type Family = AnyFamily<Families>;
pub(crate) type Scenario = oer_hil_scenario::Scenario<Family>;
pub(crate) type Catalog = oer_hil_scenario::Catalog<Family>;

/// The fixture services a selection needs together.
pub(crate) fn requirements(selected: &[&Scenario]) -> Requirements {
    Requirements::union(
        selected
            .iter()
            .map(|scenario| scenario.family.requirements()),
    )
}
