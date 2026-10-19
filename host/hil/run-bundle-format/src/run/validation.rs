//! Structural validation of the canonical HIL run documents.

use super::{
    Measurement, MeasurementVerdict, Outcome, RUN_SCHEMA, RunManifest, RunState, SuiteCounts,
    SuiteResult, aggregate_outcome,
};
use crate::Result;
use serde::Deserialize;
use std::{
    collections::BTreeSet,
    fs,
    path::{Component, Path},
};

/// The JSON document at `path`, typed.
///
/// A file that is absent is a message, a bundle missing it; any other failure
/// to read it stays an I/O error, so a reader can tell a broken store from a
/// broken bundle.
pub fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T> {
    let bytes = fs::read(path).map_err(|error| -> Box<dyn std::error::Error + Send + Sync> {
        let message = format!("cannot read HIL evidence `{}`: {error}", path.display());
        if error.kind() == std::io::ErrorKind::NotFound {
            message.into()
        } else {
            Box::new(std::io::Error::new(error.kind(), message))
        }
    })?;
    serde_json::from_slice(&bytes)
        .map_err(|error| format!("cannot parse HIL evidence `{}`: {error}", path.display()).into())
}

/// `read_json`, or `None` when the file does not exist.
pub fn read_optional_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<Option<T>> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
        Ok(_) => read_json(path).map(Some),
    }
}

/// A non-empty relative path of normal components only.
pub fn safe_relative(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

/// A lowercase hexadecimal SHA-256.
pub fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// A scenario id: lowercase words of letters and digits joined by single
/// hyphens, starting with a letter.
pub fn valid_id(value: &str) -> bool {
    value
        .bytes()
        .next()
        .is_some_and(|byte| byte.is_ascii_lowercase())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && !value.ends_with('-')
        && !value.contains("--")
}

/// The measurements of one repetition that ended `outcome`: unique stable
/// names, a threshold and verdict both present or both absent, the verdict
/// the value and threshold give, and no failed verdict in a repetition that
/// did not fail.
pub fn validate_measurements(measurements: &[Measurement], outcome: Outcome) -> Result<()> {
    let mut names = BTreeSet::new();
    for measurement in measurements {
        if measurement.name.is_empty()
            || measurement.name.len() > 128
            || !measurement.name.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'-')
            })
            || !names.insert(&measurement.name)
        {
            return Err(format!(
                "invalid or duplicate HIL measurement name `{}`",
                measurement.name
            )
            .into());
        }
        if measurement.semantics < Measurement::FIRST_SEMANTICS {
            return Err(format!(
                "HIL measurement `{}` has semantics version {}, below the first ({})",
                measurement.name,
                measurement.semantics,
                Measurement::FIRST_SEMANTICS
            )
            .into());
        }
        if !measurement.is_consistent() {
            return Err(format!(
                "HIL measurement `{}` has a verdict its value and threshold contradict, only one of them, or a threshold against its declared direction",
                measurement.name
            )
            .into());
        }
    }
    if measurements
        .iter()
        .any(|measurement| measurement.verdict == Some(MeasurementVerdict::Failed))
        && !matches!(
            outcome,
            Outcome::Failed | Outcome::Broken | Outcome::Interrupted
        )
    {
        return Err("a failed HIL measurement contradicts repetition outcome".into());
    }
    Ok(())
}

pub fn validate_manifest(manifest: &RunManifest, target: &str, directory: &Path) -> Result<()> {
    if manifest.schema != RUN_SCHEMA {
        return Err(format!(
            "HIL manifest `{}` has schema {}, expected {RUN_SCHEMA}",
            directory.display(),
            manifest.schema
        )
        .into());
    }
    if manifest.target != target {
        return Err(format!(
            "HIL manifest `{}` targets `{}`, expected `{target}`",
            directory.display(),
            manifest.target
        )
        .into());
    }
    let timestamps_are_consistent = match manifest.state {
        RunState::Running => {
            manifest.finished_unix_millis.is_none() && manifest.duration_millis.is_none()
        }
        RunState::Completed | RunState::Interrupted => {
            manifest.finished_unix_millis.is_some()
                && manifest.duration_millis.is_some()
                && manifest.finished_unix_millis >= Some(manifest.started_unix_millis)
        }
    };
    if !timestamps_are_consistent {
        return Err(format!(
            "HIL manifest `{}` has timestamps inconsistent with state {:?}",
            directory.display(),
            manifest.state
        )
        .into());
    }
    let directory_id = directory
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("HIL run directory is not UTF-8: {}", directory.display()))?;
    if manifest.run_id != directory_id {
        return Err(format!(
            "HIL manifest run ID `{}` does not match directory `{directory_id}`",
            manifest.run_id
        )
        .into());
    }
    Ok(())
}

pub fn validate_suite(suite: &SuiteResult, manifest: &RunManifest) -> Result<()> {
    if suite.schema != RUN_SCHEMA {
        return Err(format!(
            "HIL suite `{}` has schema {}, expected {RUN_SCHEMA}",
            suite.run_id, suite.schema
        )
        .into());
    }
    if suite.run_id != manifest.run_id
        || suite.target != manifest.target
        || suite.started_unix_millis != manifest.started_unix_millis
        || Some(suite.finished_unix_millis) != manifest.finished_unix_millis
        || Some(suite.duration_millis) != manifest.duration_millis
    {
        return Err(format!(
            "HIL suite `{}` does not match its manifest",
            manifest.run_id
        )
        .into());
    }
    if suite.counts != SuiteCounts::from_results(&suite.scenarios) {
        return Err(format!("HIL suite `{}` has inconsistent counts", suite.run_id).into());
    }
    let expected_outcome = if suite
        .scenarios
        .iter()
        .all(|scenario| scenario.outcome.is_passed())
    {
        Outcome::Passed
    } else {
        Outcome::Failed
    };
    if suite.outcome != expected_outcome {
        return Err(format!(
            "HIL suite `{}` has an outcome inconsistent with its scenarios",
            suite.run_id
        )
        .into());
    }
    let mut seen = BTreeSet::new();
    for scenario in &suite.scenarios {
        if scenario.schema != RUN_SCHEMA {
            return Err(format!(
                "HIL scenario `{}` has schema {}, expected {RUN_SCHEMA}",
                scenario.scenario, scenario.schema
            )
            .into());
        }
        if !valid_id(&scenario.scenario)
            || !(1..=20).contains(&scenario.required_repetitions)
            || !seen.insert(&scenario.scenario)
        {
            return Err(format!(
                "HIL suite `{}` has an invalid or duplicate scenario `{}`",
                suite.run_id, scenario.scenario
            )
            .into());
        }
        if scenario.repetitions.is_empty() {
            if scenario.outcome != Outcome::Blocked || scenario.failure.is_none() {
                return Err(format!(
                    "HIL scenario `{}` has no repetitions without a blocking failure",
                    scenario.scenario
                )
                .into());
            }
            continue;
        }
        if scenario.repetitions.len() != usize::from(scenario.required_repetitions)
            || scenario.outcome
                != aggregate_outcome(scenario.repetitions.iter().map(|entry| entry.outcome))
        {
            return Err(format!(
                "HIL scenario `{}` has inconsistent repetition results",
                scenario.scenario
            )
            .into());
        }
        for (index, repetition) in scenario.repetitions.iter().enumerate() {
            if repetition.schema != RUN_SCHEMA {
                return Err(format!(
                    "HIL scenario `{}` repetition {} has schema {}, expected {RUN_SCHEMA}",
                    scenario.scenario, repetition.repetition, repetition.schema
                )
                .into());
            }
            if usize::from(repetition.repetition) != index + 1 {
                return Err(format!(
                    "HIL scenario `{}` has a non-canonical repetition sequence",
                    scenario.scenario
                )
                .into());
            }
            if (repetition.outcome == Outcome::Passed && repetition.failure.is_some())
                || (matches!(
                    repetition.outcome,
                    Outcome::Failed
                        | Outcome::Broken
                        | Outcome::Blocked
                        | Outcome::Interrupted
                        | Outcome::BoardQuarantined
                ) && repetition.failure.is_none())
            {
                return Err(format!(
                    "HIL scenario `{}` repetition {} has an inconsistent failure record",
                    scenario.scenario, repetition.repetition
                )
                .into());
            }
            validate_measurements(&repetition.measurements, repetition.outcome).map_err(
                |error| {
                    format!(
                        "HIL scenario `{}` repetition {}: {error}",
                        scenario.scenario, repetition.repetition
                    )
                },
            )?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::{Better, Comparison, MeasurementUnit};
    use super::*;

    fn rate(value: u64) -> Measurement {
        Measurement::observed("udp.rate", value, MeasurementUnit::BitsPerSecond)
    }

    #[test]
    fn a_measurement_below_the_first_semantics_version_is_refused() {
        assert!(validate_measurements(&[rate(5)], Outcome::Passed).is_ok());
        let error = validate_measurements(&[rate(5).semantics(0)], Outcome::Passed).unwrap_err();
        assert!(error.to_string().contains("semantics version 0"), "{error}");
    }

    #[test]
    fn a_threshold_against_the_declared_direction_is_refused() {
        let agreeing = rate(5)
            .better(Better::Higher)
            .evaluated(Comparison::AtLeast, 1);
        assert!(validate_measurements(&[agreeing], Outcome::Passed).is_ok());
        let contradicting = rate(5)
            .better(Better::Lower)
            .evaluated(Comparison::AtLeast, 1);
        assert!(!contradicting.is_consistent());
        let error = validate_measurements(&[contradicting], Outcome::Passed).unwrap_err();
        assert!(error.to_string().contains("declared direction"), "{error}");
    }
}
