//! Recursive discovery for the versioned scenario catalog.
//!
//! Scenario identity is independent of its domain folder. Entries are regular
//! TOML files or README.md; symlinks and other file kinds fail closed.

use std::path::Path;

use oer_hil_scenario_catalog::documents;

use super::{Scenario, ScenarioFamily};
use crate::Result;

#[derive(Debug)]
pub struct Catalog<F> {
    scenarios: Vec<Scenario<F>>,
}

impl<F: ScenarioFamily> Catalog<F> {
    /// The catalog below `directory`, each document parsed and validated
    /// with its family's types.
    pub fn load(directory: &Path) -> Result<Self> {
        let scenarios = documents(directory)?
            .into_iter()
            .map(|document| {
                Scenario::<F>::from_value(document.value)
                    .and_then(|mut scenario| {
                        scenario.source = document.path.clone();
                        scenario.validate()?;
                        Ok(scenario)
                    })
                    .map_err(|error| format!("{}: {error}", document.path.display()).into())
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self::new(scenarios))
    }

    /// Build a catalog from validated scenarios.
    pub fn new(scenarios: Vec<Scenario<F>>) -> Self {
        Self { scenarios }
    }

    pub fn all(&self) -> &[Scenario<F>] {
        &self.scenarios
    }

    pub fn get(&self, id: &str) -> Result<&Scenario<F>> {
        self.scenarios
            .iter()
            .find(|scenario| scenario.id() == id)
            .ok_or_else(|| format!("unknown HIL scenario `{id}`").into())
    }
}

#[cfg(test)]
mod tests;
