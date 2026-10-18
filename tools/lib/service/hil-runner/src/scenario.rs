//! The executable scenario catalog: the common envelope over exactly one of
//! the radio families this runner composes. This module is the runner's
//! composition: [`Families`] lists the families and fixture providers, and
//! the core reaches every family only through it.

use oer_hil_scenario::ScenarioFamily as _;
use oer_hil_scenario_catalog::requirements::Requirements;
use oer_hil_workload::family::{AnyFamily, FixtureProvider, Kind, Registry};

/// The radio families and fixture providers of this runner.
pub(crate) struct Families;

impl Registry for Families {
    const FAMILIES: &'static [Kind] = &[oer_hil_family_system::FAMILY];
    const FIXTURES: &'static [&'static dyn FixtureProvider] =
        &[&oer_hil_family_ieee80211_fixture::provider::PROVIDER];
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

#[cfg(test)]
mod tests;
