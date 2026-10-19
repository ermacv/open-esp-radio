//! Serialized vocabulary for one HIL invocation and its derived views.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use oer_hil_schema::image::ImageClass;

/// Format of a run bundle and its `manifest.json`. 7: a run's runtime ELF
/// lies deflate-compressed at `firmware/<image>/runtime.elf.deflate`; what
/// the manifest and build provenance record of it (path, size, digest) is
/// the uncompressed file's ([`crate::archived`]).
pub const RUN_SCHEMA: u16 = 10;

pub use oer_hil_schema::run::{
    Better, Comparison, FailureKind, MeasurementUnit, MeasurementVerdict, Outcome, RunEventKind,
    RunState, Threshold,
};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Failure {
    pub kind: FailureKind,
    pub message: String,
}

impl Failure {
    pub fn new(kind: FailureKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Attachment {
    pub path: PathBuf,
    pub media_type: String,
    pub size_bytes: u64,
    pub sha256: String,
}

/// One measured value of a repetition.
///
/// Its identity as a metric ([`MetricId`]) is its scenario, `name`, `unit`,
/// `semantics` and `better`: values are comparable across repetitions,
/// runs and revisions only when all of these agree. A producer bumps
/// `semantics` whenever what the value means changes under the same name
/// and unit (another window, another counting rule, another endpoint).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Measurement {
    pub name: String,
    pub value: u64,
    pub unit: MeasurementUnit,
    /// The version of what the value means; starts at
    /// [`Measurement::FIRST_SEMANTICS`].
    pub semantics: u16,
    /// Which way the quantity improves; `None` when the producer declares
    /// no direction (a counter, a diagnostic figure). An `at-least` or
    /// `at-most` threshold fixes it.
    pub better: Option<Better>,
    pub threshold: Option<Threshold>,
    pub verdict: Option<MeasurementVerdict>,
}

impl Measurement {
    /// The semantics version of a measurement whose meaning never changed.
    pub const FIRST_SEMANTICS: u16 = 1;

    /// A value of the first semantics version without a declared
    /// direction, threshold or verdict.
    pub fn observed(name: impl Into<String>, value: u64, unit: MeasurementUnit) -> Self {
        Self {
            name: name.into(),
            value,
            unit,
            semantics: Self::FIRST_SEMANTICS,
            better: None,
            threshold: None,
            verdict: None,
        }
    }

    /// The same value under semantics version `version`.
    pub fn semantics(mut self, version: u16) -> Self {
        self.semantics = version;
        self
    }

    /// Declare which way the quantity improves.
    pub fn better(mut self, better: Better) -> Self {
        self.better = Some(better);
        self
    }

    /// Judge the value against `threshold`; an `at-least` or `at-most`
    /// threshold also declares the direction (higher or lower is better)
    /// unless one is declared already, in which case they must agree
    /// ([`Self::is_consistent`]).
    pub fn evaluated(mut self, comparison: Comparison, threshold: u64) -> Self {
        if self.better.is_none() {
            self.better = comparison.better();
        }
        self.threshold = Some(Threshold {
            comparison,
            value: threshold,
        });
        self.verdict = Some(if measurement_passes(self.value, comparison, threshold) {
            MeasurementVerdict::Passed
        } else {
            MeasurementVerdict::Failed
        });
        self
    }

    /// Whether its threshold and verdict are both absent, or both present
    /// with the verdict its value and threshold give, and a directional
    /// threshold agrees with the declared direction.
    pub fn is_consistent(&self) -> bool {
        match (self.threshold, self.verdict) {
            (None, None) => true,
            (Some(threshold), Some(verdict)) => {
                let expected =
                    if measurement_passes(self.value, threshold.comparison, threshold.value) {
                        MeasurementVerdict::Passed
                    } else {
                        MeasurementVerdict::Failed
                    };
                let direction = threshold
                    .comparison
                    .better()
                    .is_none_or(|better| self.better == Some(better));
                verdict == expected && direction
            }
            _ => false,
        }
    }

    /// Its identity as a metric of `scenario`.
    pub fn metric(&self, scenario: &str) -> MetricId {
        MetricId {
            scenario: scenario.to_owned(),
            name: self.name.clone(),
            unit: self.unit,
            semantics: self.semantics,
            better: self.better,
        }
    }
}

/// What makes two measured values the same quantity: the scenario that
/// measured it, its name, unit, semantics version and improvement
/// direction. Analyses combine and compare only values of one `MetricId`
/// and refuse values that share a scenario and name but differ in the rest.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MetricId {
    pub scenario: String,
    pub name: String,
    pub unit: MeasurementUnit,
    pub semantics: u16,
    pub better: Option<Better>,
}

impl MetricId {
    /// Why `other`, of the same scenario and name, is not this metric;
    /// `None` when it is.
    pub fn incompatibility(&self, other: &Self) -> Option<String> {
        let mut differences = Vec::new();
        if self.unit != other.unit {
            differences.push(format!("unit {} vs {}", self.unit, other.unit));
        }
        if self.semantics != other.semantics {
            differences.push(format!(
                "semantics {} vs {}",
                self.semantics, other.semantics
            ));
        }
        if self.better != other.better {
            let direction = |better: Option<Better>| better.map_or("undeclared", Better::id);
            differences.push(format!(
                "better {} vs {}",
                direction(self.better),
                direction(other.better)
            ));
        }
        (!differences.is_empty()).then(|| differences.join(", "))
    }
}

impl std::fmt::Display for MetricId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} {} [{}, semantics {}",
            self.scenario, self.name, self.unit, self.semantics
        )?;
        match self.better {
            Some(better) => write!(f, ", {better} is better]"),
            None => f.write_str("]"),
        }
    }
}

const fn measurement_passes(value: u64, comparison: Comparison, threshold: u64) -> bool {
    match comparison {
        Comparison::AtLeast => value >= threshold,
        Comparison::AtMost => value <= threshold,
        Comparison::Exactly => value == threshold,
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RepetitionResult {
    pub schema: u16,
    pub repetition: u8,
    pub outcome: Outcome,
    pub started_unix_millis: u64,
    pub duration_millis: u64,
    pub artifact_directory: PathBuf,
    pub attachments: Vec<Attachment>,
    pub measurements: Vec<Measurement>,
    pub failure: Option<Failure>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ScenarioResult {
    pub schema: u16,
    pub scenario: String,
    pub image: ImageClass,
    pub outcome: Outcome,
    pub required_repetitions: u8,
    pub repetitions: Vec<RepetitionResult>,
    pub failure: Option<Failure>,
}

impl ScenarioResult {
    pub fn from_repetitions(
        scenario: String,
        image: ImageClass,
        required_repetitions: u8,
        repetitions: Vec<RepetitionResult>,
    ) -> Self {
        let outcome = aggregate_outcome(repetitions.iter().map(|entry| entry.outcome));
        Self {
            schema: RUN_SCHEMA,
            scenario,
            image,
            outcome,
            required_repetitions,
            repetitions,
            failure: None,
        }
    }

    pub fn blocked(
        scenario: String,
        image: ImageClass,
        required_repetitions: u8,
        failure: Failure,
    ) -> Self {
        Self {
            schema: RUN_SCHEMA,
            scenario,
            image,
            outcome: Outcome::Blocked,
            required_repetitions,
            repetitions: Vec::new(),
            failure: Some(failure),
        }
    }
}

pub fn aggregate_outcome(outcomes: impl IntoIterator<Item = Outcome>) -> Outcome {
    let observed = outcomes.into_iter().collect::<Vec<_>>();
    if !observed.is_empty() && observed.iter().all(|outcome| outcome.is_passed()) {
        Outcome::Passed
    } else if observed.contains(&Outcome::Interrupted) {
        Outcome::Interrupted
    } else if observed.contains(&Outcome::BoardQuarantined) {
        Outcome::BoardQuarantined
    } else if observed.contains(&Outcome::Broken) {
        Outcome::Broken
    } else if observed.contains(&Outcome::Failed) {
        Outcome::Failed
    } else if observed.contains(&Outcome::Blocked) {
        Outcome::Blocked
    } else {
        Outcome::Skipped
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SuiteCounts {
    pub scenarios: usize,
    pub passed: usize,
    pub failed: usize,
    pub broken: usize,
    pub skipped: usize,
    pub blocked: usize,
    pub interrupted: usize,
}

impl SuiteCounts {
    pub fn from_results(results: &[ScenarioResult]) -> Self {
        let mut counts = Self {
            scenarios: results.len(),
            passed: 0,
            failed: 0,
            broken: 0,
            skipped: 0,
            blocked: 0,
            interrupted: 0,
        };
        for result in results {
            match result.outcome {
                Outcome::Passed => counts.passed += 1,
                Outcome::Failed => counts.failed += 1,
                Outcome::Broken => counts.broken += 1,
                Outcome::Skipped => counts.skipped += 1,
                // A quarantined board says nothing about the code under test.
                Outcome::Blocked | Outcome::BoardQuarantined => counts.blocked += 1,
                Outcome::Interrupted => counts.interrupted += 1,
            }
        }
        counts
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SuiteResult {
    pub schema: u16,
    pub run_id: String,
    pub target: String,
    pub outcome: Outcome,
    pub started_unix_millis: u64,
    pub finished_unix_millis: u64,
    pub duration_millis: u64,
    pub counts: SuiteCounts,
    pub scenarios: Vec<ScenarioResult>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PlanDisposition {
    Selected,
    Filtered,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PlanEntry {
    pub scenario: String,
    pub image: ImageClass,
    pub repetitions: u8,
    pub disposition: PlanDisposition,
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requirements: Option<oer_hil_scenario_catalog::requirements::Requirements>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RunPlan {
    pub schema: u16,
    pub run_id: String,
    pub selection: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub firmware: Option<PlannedFirmware>,
    pub entries: Vec<PlanEntry>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case", tag = "source")]
pub enum PlannedFirmware {
    BuildCurrent,
    Replay {
        source_run_id: String,
        image: ImageClass,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        build_id: Option<String>,
        application_sha256: String,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RepositoryProvenance {
    pub commit: String,
    pub dirty: bool,
    pub workspace_sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FirmwareReplayOrigin {
    pub source_run_id: String,
    pub firmware_repository: RepositoryProvenance,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_build_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RunnerProvenance {
    /// Identity captured by the running process and embedded at its own build.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observer: Option<serde_json::Value>,
    pub package: String,
    pub version: String,
    /// SHA-256 of the runner's `oer_hil_protocol::MESSAGES_LOCK`: runs
    /// whose runners share it spoke the same wire.
    pub messages_lock_sha256: String,
    pub host_os: String,
    pub host_arch: String,
    pub tools: Vec<ToolVersion>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ToolVersion {
    pub name: String,
    pub version: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CellProvenance {
    pub cell_id: String,
    pub device_id: String,
    pub serial_device: PathBuf,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FirmwareArtifact {
    pub image: ImageClass,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replayed_from: Option<FirmwareReplayOrigin>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_provenance_path: Option<PathBuf>,
    pub application_path: PathBuf,
    pub application_size_bytes: u64,
    pub application_sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_elf_path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_elf_size_bytes: Option<u64>,
    pub runtime_elf_sha256: String,
    /// The staged image's packed runtime and bootstrap, from which the boot
    /// files are encoded again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_bin_path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_bin_size_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_bin_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bootstrap_elf_path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bootstrap_elf_size_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bootstrap_elf_sha256: Option<String>,
    /// The seed the image's runtime was linked with, as its build record
    /// names it; absent for the natural order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout_seed: Option<std::num::NonZeroU32>,
}

/// One archived firmware subject besides the application, as the manifest
/// records it: path, size and digest are present together or not at all.
#[derive(Clone, Copy, Debug)]
pub struct SubjectRecord<'a> {
    /// The subject's file name inside `firmware/<image>/`.
    pub file: &'static str,
    /// What the subject is, for messages.
    pub kind: &'static str,
    pub path: Option<&'a PathBuf>,
    pub size_bytes: Option<u64>,
    pub sha256: Option<&'a str>,
}

impl FirmwareArtifact {
    /// Every subject besides the application, in archive order.
    pub fn subjects(&self) -> [SubjectRecord<'_>; 3] {
        [
            SubjectRecord {
                file: "runtime.elf",
                kind: "runtime ELF",
                path: self.runtime_elf_path.as_ref(),
                size_bytes: self.runtime_elf_size_bytes,
                sha256: Some(&self.runtime_elf_sha256),
            },
            SubjectRecord {
                file: "runtime.bin",
                kind: "runtime binary",
                path: self.runtime_bin_path.as_ref(),
                size_bytes: self.runtime_bin_size_bytes,
                sha256: self.runtime_bin_sha256.as_deref(),
            },
            SubjectRecord {
                file: "bootstrap.elf",
                kind: "bootstrap ELF",
                path: self.bootstrap_elf_path.as_ref(),
                size_bytes: self.bootstrap_elf_size_bytes,
                sha256: self.bootstrap_elf_sha256.as_deref(),
            },
        ]
    }

    /// The subjects a staged image requires besides the application and
    /// runtime ELF.
    pub fn required_subjects(&self) -> &'static [&'static str] {
        &["runtime.bin", "bootstrap.elf"]
    }
}

/// One line of the run's event stream as its writer records it.
#[derive(Serialize)]
pub struct EventRecord<'a> {
    pub timestamp_unix_millis: u64,
    pub kind: RunEventKind,
    pub scenario: Option<&'a str>,
    pub image: Option<ImageClass>,
    pub outcome: Option<Outcome>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RunManifest {
    pub schema: u16,
    pub run_id: String,
    pub target: String,
    pub state: RunState,
    pub started_unix_millis: u64,
    pub finished_unix_millis: Option<u64>,
    pub duration_millis: Option<u64>,
    /// The runner's command line; its first word is the runner executable.
    pub invocation: Vec<String>,
    pub repository: RepositoryProvenance,
    pub runner: RunnerProvenance,
    pub cell: CellProvenance,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lab_provenance_path: Option<PathBuf>,
    pub firmware: Vec<FirmwareArtifact>,
    /// The A/B experiment arm this run measured, when `cargo hil ab` ran it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub experiment: Option<crate::experiment::Experiment>,
    /// Every message path the run's captures record in either direction,
    /// sorted and unique; written when the run finishes.
    pub messages_used: Vec<String>,
}

impl RunManifest {
    /// The observer record of the runner that produced this run.
    pub fn observer(&self) -> Option<&serde_json::Value> {
        self.runner
            .observer
            .as_ref()
            .filter(|record| !record.is_null())
    }
}

/// One file of a directory's inventory ([`crate::run::inventory`]).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct InventoryFile {
    pub path: PathBuf,
    pub size_bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Serialize)]
pub struct CompletionReport {
    pub schema: u16,
    pub run_id: String,
    pub outcome: Outcome,
    pub run_directory: PathBuf,
    pub suite_report: PathBuf,
    pub junit_report: PathBuf,
    pub html_report: PathBuf,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rate() -> Measurement {
        Measurement::observed("udp.rate", 5, MeasurementUnit::BitsPerSecond)
    }

    #[test]
    fn a_metric_is_the_same_only_when_unit_semantics_and_direction_agree() {
        let metric = rate().better(Better::Higher).metric("udp-tx");
        assert_eq!(metric.incompatibility(&metric.clone()), None);
        // The value does not belong to the identity.
        let other_value = Measurement { value: 9, ..rate() }.better(Better::Higher);
        assert_eq!(metric.incompatibility(&other_value.metric("udp-tx")), None);

        let unit = Measurement::observed("udp.rate", 5, MeasurementUnit::Count)
            .better(Better::Higher)
            .metric("udp-tx");
        assert!(metric.incompatibility(&unit).unwrap().starts_with("unit "));
        let semantics = rate().better(Better::Higher).semantics(2).metric("udp-tx");
        assert_eq!(
            metric.incompatibility(&semantics).unwrap(),
            "semantics 1 vs 2"
        );
        let direction = rate().better(Better::Lower).metric("udp-tx");
        assert!(
            metric
                .incompatibility(&direction)
                .unwrap()
                .starts_with("better ")
        );
        let undeclared = rate().metric("udp-tx");
        assert!(
            metric
                .incompatibility(&undeclared)
                .unwrap()
                .contains("undeclared")
        );
        // Every difference is named.
        let all = Measurement::observed("udp.rate", 5, MeasurementUnit::Count)
            .semantics(3)
            .metric("udp-tx");
        assert_eq!(
            metric.incompatibility(&all).unwrap().matches(", ").count(),
            2
        );
    }

    #[test]
    fn a_directional_threshold_declares_the_direction_unless_one_is_declared() {
        assert_eq!(
            rate().evaluated(Comparison::AtMost, 9).better,
            Some(Better::Lower)
        );
        assert_eq!(rate().evaluated(Comparison::Exactly, 5).better, None);
        let declared = rate()
            .better(Better::Higher)
            .evaluated(Comparison::AtMost, 9);
        assert_eq!(declared.better, Some(Better::Higher));
        assert!(!declared.is_consistent());
    }
}
