//! The offline plan `cargo hil plan` prints: the selected scenarios, their
//! procedure digests, requirements and the named checks they supply. It
//! binds scenario semantics, not a firmware build, and confers no
//! qualification.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::{Catalog, Result, Scenario, ScenarioFamily};
use oer_hil_scenario_catalog::requirements::Requirements;
use oer_hil_schema::image::ImageClass;

const CAMPAIGN_SCHEMA: u16 = 7;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
enum Reason {
    Requested,
    ProvidesChecks { checks: Vec<String> },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    scenario: String,
    scenario_sha256: String,
    image: ImageClass,
    repetitions: u8,
    requirements: Requirements,
    reasons: Vec<Reason>,
    supported_checks: Vec<String>,
}

/// Serializable execution inputs; unlike run provenance this is not evidence.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    schema: u16,
    network: String,
    requested: Vec<String>,
    requested_checks: Vec<String>,
    requirements: Requirements,
    scenarios: Vec<Entry>,
}

// Descriptive metadata is preserved in run provenance, but is not executed.
fn procedure<F: ScenarioFamily>(scenario: &Scenario<F>) -> Result<serde_json::Value> {
    Ok(crate::identity::normalize(&serde_json::to_value(scenario)?))
}

impl Plan {
    #[cfg(any(test, feature = "test-support"))]
    pub fn create<F: ScenarioFamily>(
        catalog: &Catalog<F>,
        requested: &[&Scenario<F>],
        network: &str,
    ) -> Result<Self> {
        Self::create_for_checks(catalog, requested, network, &[])
    }

    pub fn create_for_checks<F: ScenarioFamily>(
        catalog: &Catalog<F>,
        candidates: &[&Scenario<F>],
        network: &str,
        checks: &[String],
    ) -> Result<Self> {
        let unique = checks.iter().collect::<BTreeSet<_>>();
        if unique.len() != checks.len() {
            return Err("campaign repeats a requested check".into());
        }
        let checks = unique.into_iter().cloned().collect::<Vec<_>>();
        let requested = candidates
            .iter()
            .copied()
            .filter(|scenario| {
                let supported = scenario.plan().checks;
                checks
                    .iter()
                    .all(|check| supported.contains(&check.as_str()))
            })
            .collect::<Vec<_>>();
        if requested.is_empty() {
            return Err("no selected scenario supplies every requested check".into());
        }
        let ids = requested
            .iter()
            .map(|s| s.id().to_owned())
            .collect::<BTreeSet<_>>();
        if ids.len() != requested.len() {
            return Err("campaign repeats a requested scenario".into());
        }
        // Selection never expands: product prerequisites do not belong to a
        // campaign and never trigger a baseline suite here.
        let ordered = ids
            .iter()
            .map(|id| catalog.get(id))
            .collect::<Result<Vec<_>>>()?;
        // Match the executor's single-flash-per-image grouping.
        let ordered = ImageClass::ALL
            .into_iter()
            .flat_map(|image| ordered.iter().copied().filter(move |s| s.image() == image))
            .collect::<Vec<_>>();
        let scenarios = ordered
            .iter()
            .map(|scenario| {
                let mut reasons = vec![Reason::Requested];
                let plan = scenario.plan();
                if !checks.is_empty() {
                    reasons.push(Reason::ProvidesChecks {
                        checks: checks.clone(),
                    });
                }
                Ok(Entry {
                    scenario: scenario.id().to_owned(),
                    scenario_sha256: oer_durable::sha256_bytes(&serde_json::to_vec(&procedure(
                        scenario,
                    )?)?),
                    image: plan.image,
                    repetitions: scenario.repetitions(),
                    requirements: plan.requirements,
                    reasons,
                    supported_checks: plan.checks.into_iter().map(str::to_owned).collect(),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            schema: CAMPAIGN_SCHEMA,
            network: network.to_owned(),
            requested: ids.into_iter().collect(),
            requested_checks: checks,
            requirements: Requirements::union(ordered.iter().map(|s| s.requirements())),
            scenarios,
        })
    }
}

#[cfg(test)]
mod tests;
