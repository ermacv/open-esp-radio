//! The typed results of one repetition: the evidence its workload observed,
//! step by step, and the claim it makes. [`Results::write`] is the one
//! writer of a repetition's `observations.json`
//! ([`oer_hil_run_bundle_format::run::Observations`]); workloads never write report
//! files of their own.

use std::{
    path::Path,
    sync::{Arc, Mutex},
};

use oer_hil_run_bundle_format::run::Claim;
use oer_hil_run_bundle_format::run::OBSERVATIONS_FILE;
use oer_hil_run_bundle_format::run::OBSERVATIONS_SCHEMA;
use oer_hil_run_bundle_format::run::Observation;
use oer_hil_run_bundle_format::run::Observations;
use serde::Serialize;

use crate::Result;

/// A repetition's typed observations, shared by its workload's threads.
#[derive(Clone, Default)]
pub struct Results(Arc<Mutex<Observations>>);

impl Results {
    /// Record `value` as the observation `name`. A value that does not
    /// serialize is recorded as its error, never dropped.
    pub fn observe(&self, name: impl Into<String>, value: &impl Serialize) {
        let value = serde_json::to_value(value)
            .unwrap_or_else(|error| serde_json::json!({"unserializable": error.to_string()}));
        self.lock().observations.push(Observation {
            name: name.into(),
            value,
        });
    }

    /// Record `value` as the observation `name`, replacing the one recorded
    /// under that name before: a workload's progressively completed
    /// evidence, kept current at each step.
    pub fn replace(&self, name: impl Into<String>, value: &impl Serialize) {
        let name = name.into();
        let value = serde_json::to_value(value)
            .unwrap_or_else(|error| serde_json::json!({"unserializable": error.to_string()}));
        let mut observations = self.lock();
        match observations
            .observations
            .iter_mut()
            .find(|observation| observation.name == name)
        {
            Some(observation) => observation.value = value,
            None => observations.observations.push(Observation { name, value }),
        }
    }

    /// What a passing repetition of this workload establishes, and what it
    /// does not.
    pub fn claim(&self, result: &str, not_proven: &[&str]) {
        self.lock().claim = Some(Claim {
            result: result.to_owned(),
            not_proven: not_proven.iter().map(|&name| name.to_owned()).collect(),
        });
    }

    /// The observations so far.
    pub fn snapshot(&self) -> Observations {
        let mut observations = self.lock().clone();
        observations.schema = OBSERVATIONS_SCHEMA;
        observations
    }

    /// Write `observations.json` into the repetition's `output`, atomically;
    /// nothing when the workload observed nothing.
    pub fn write(&self, output: &Path) -> Result<()> {
        let observations = self.snapshot();
        if observations.claim.is_none() && observations.observations.is_empty() {
            return Ok(());
        }
        oer_durable::atomic_json(&output.join(OBSERVATIONS_FILE), &observations)?;
        Ok(())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Observations> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observations_are_written_once_in_order_with_their_claim() {
        let directory = std::env::temp_dir().join(format!(
            "oer-results-{}-{}",
            std::process::id(),
            oer_durable::unix_millis()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let results = Results::default();
        results.write(&directory).unwrap();
        assert!(!directory.join(OBSERVATIONS_FILE).exists());
        results.observe("boot-001", &serde_json::json!({"rssi": -40}));
        results.observe("boot-002", &[1, 2]);
        results.replace("boot-002", &[3]);
        results.replace("progress", &1);
        results.claim("exchange", &["calibrated-rssi"]);
        results.write(&directory).unwrap();
        let written: Observations =
            serde_json::from_slice(&std::fs::read(directory.join(OBSERVATIONS_FILE)).unwrap())
                .unwrap();
        assert_eq!(written.schema, OBSERVATIONS_SCHEMA);
        assert_eq!(
            written
                .observations
                .iter()
                .map(|observation| observation.name.as_str())
                .collect::<Vec<_>>(),
            ["boot-001", "boot-002", "progress"]
        );
        assert_eq!(written.observations[1].value, serde_json::json!([3]));
        assert_eq!(written.observations[0].value["rssi"], -40);
        assert_eq!(written.claim.unwrap().not_proven, ["calibrated-rssi"]);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
