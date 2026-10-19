//! The one reader of a run bundle.
//!
//! [`RunBundle::open`] reads a run's manifest into its typed form; every
//! other document of the bundle is read through it, typed: the plan, the
//! suite and its scenario and repetition results with their measurements,
//! the scenario snapshots, the cleanup and USB records of a repetition, the
//! laboratory provenance and the event stream. A document outside
//! this build's vocabulary, or a run of another schema, is an error, never a
//! silently absent value. Whoever reads a bundle, the `cargo hil` analyses
//! included, reads it here.

use std::{
    fs,
    path::{Path, PathBuf},
};

use oer_hil_schema::run::RunEvent;

use crate::{
    Result,
    lab::LabProvenance,
    run::{
        CLEANUP_FILE, CleanupRecord, OBSERVATIONS_FILE, Observations, PlannedFirmware, RUN_SCHEMA,
        RepetitionResult, RunManifest, RunPlan, RunState, ScenarioResult, SuiteResult,
        USB_EVENTS_FILE, UsbEvent,
        validation::{read_json, read_optional_json, validate_manifest},
    },
};

pub const MANIFEST: &str = "manifest.json";
pub const PLAN: &str = "plan.json";
pub const SUITE: &str = "suite.json";
pub const EVENTS: &str = "events.jsonl";
pub const SCENARIOS: &str = "scenarios";

/// One published run: its directory and typed manifest.
#[derive(Clone, Debug)]
pub struct RunBundle {
    directory: PathBuf,
    manifest: RunManifest,
}

impl RunBundle {
    /// The run in `directory`; `None` while it has no manifest (a run being
    /// created publishes its manifest first). A manifest of another schema,
    /// or outside this build's format, is an error; the run stays as it is.
    pub fn open(directory: &Path) -> Result<Option<Self>> {
        let Some(document) = read_optional_json::<serde_json::Value>(&directory.join(MANIFEST))?
        else {
            return Ok(None);
        };
        let schema = &document["schema"];
        if schema.as_u64() != Some(u64::from(RUN_SCHEMA)) {
            return Err(format!(
                "HIL run {} has schema {schema}, expected {RUN_SCHEMA}",
                directory.display()
            )
            .into());
        }
        let manifest: RunManifest = serde_json::from_value(document)?;
        if !fs::symlink_metadata(directory.join(MANIFEST))?
            .file_type()
            .is_file()
        {
            return Err(format!(
                "HIL run manifest is not a regular file: {}",
                directory.display()
            )
            .into());
        }
        Ok(Some(Self {
            directory: directory.to_owned(),
            manifest,
        }))
    }

    pub fn id(&self) -> &str {
        &self.manifest.run_id
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }

    pub fn manifest(&self) -> &RunManifest {
        &self.manifest
    }

    /// Check the manifest's schema, its timestamps against its state and
    /// that it names the directory it is in.
    pub fn validate(&self) -> Result<()> {
        validate_manifest(&self.manifest, &self.manifest.target, &self.directory)
    }

    pub fn plan(&self) -> Result<Option<RunPlan>> {
        read_optional_json(&self.directory.join(PLAN))
    }

    /// The suite a completed run wrote; `None` before it ended or when it
    /// was interrupted.
    pub fn suite(&self) -> Result<Option<SuiteResult>> {
        read_optional_json(&self.directory.join(SUITE))
    }

    /// The events the run recorded, up to its last complete line.
    pub fn events(&self) -> Result<Vec<RunEvent>> {
        let path = self.directory.join(EVENTS);
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let complete = text.rfind('\n').map_or("", |end| &text[..end]);
        complete
            .lines()
            .map(|line| {
                serde_json::from_str(line)
                    .map_err(|error| format!("{}: {error}", path.display()).into())
            })
            .collect()
    }

    /// Where `scenario`'s snapshot and results live.
    pub fn scenario_directory(&self, scenario: &str) -> PathBuf {
        self.directory.join(SCENARIOS).join(scenario)
    }

    /// The scenario document the run executed, as recorded.
    pub fn scenario_document(&self, scenario: &str) -> Result<serde_json::Value> {
        read_json(&self.scenario_directory(scenario).join("scenario.json"))
    }

    /// The result of one scenario as its own record holds it.
    pub fn scenario_result(&self, scenario: &str) -> Result<ScenarioResult> {
        read_json(&self.scenario_directory(scenario).join("result.json"))
    }

    /// Where a repetition's artifacts are.
    pub fn repetition_directory(&self, repetition: &RepetitionResult) -> PathBuf {
        self.directory.join(&repetition.artifact_directory)
    }

    /// The restorations a repetition's cleanup scope attempted.
    pub fn cleanup(&self, repetition: &RepetitionResult) -> Result<Vec<CleanupRecord>> {
        Ok(
            read_optional_json(&self.repetition_directory(repetition).join(CLEANUP_FILE))?
                .unwrap_or_default(),
        )
    }

    /// The typed results a repetition's workload recorded: its observations
    /// and claim, when it recorded any.
    pub fn observations(&self, repetition: &RepetitionResult) -> Result<Option<Observations>> {
        read_optional_json(
            &self
                .repetition_directory(repetition)
                .join(OBSERVATIONS_FILE),
        )
    }

    /// The typed fixture record `R` a repetition's fixtures wrote, labelled
    /// `label` for a kind a repetition holds several of; `None` when the
    /// fixture recorded none.
    pub fn fixture_record<R: crate::run::fixtures::FixtureRecord>(
        &self,
        repetition: &RepetitionResult,
        label: Option<&str>,
    ) -> Result<Option<R>> {
        crate::run::fixtures::read(&self.repetition_directory(repetition), label)
    }

    /// The host's USB events of the boards a repetition used.
    pub fn usb_events(&self, repetition: &RepetitionResult) -> Result<Vec<UsbEvent>> {
        Ok(
            read_optional_json(&self.repetition_directory(repetition).join(USB_EVENTS_FILE))?
                .unwrap_or_default(),
        )
    }

    pub fn lab_provenance(&self) -> Result<Option<LabProvenance>> {
        self.manifest
            .lab_provenance_path
            .as_ref()
            .map(|path| read_json(&self.directory.join(path)))
            .transpose()
    }

    /// The checkout the runner ran from: the directory holding the `target`
    /// directory of the runner executable the run's command line names.
    pub fn checkout(&self) -> Option<String> {
        let runner = Path::new(self.manifest.invocation.first()?);
        runner
            .ancestors()
            .find(|ancestor| ancestor.ends_with("target"))
            .and_then(Path::parent)
            .and_then(Path::file_name)
            .map(|name| name.to_string_lossy().into_owned())
    }

    /// The runs whose archived firmware this run replayed.
    pub fn replayed_runs(&self) -> Result<Vec<String>> {
        let mut runs = self
            .manifest
            .firmware
            .iter()
            .filter_map(|artifact| artifact.replayed_from.as_ref())
            .map(|origin| origin.source_run_id.clone())
            .collect::<Vec<_>>();
        if let Some(PlannedFirmware::Replay { source_run_id, .. }) =
            self.plan()?.and_then(|plan| plan.firmware)
        {
            runs.push(source_run_id);
        }
        runs.sort();
        runs.dedup();
        Ok(runs)
    }

    /// Whether the runner that started the run still runs. The run id ends
    /// with the runner's PID in hexadecimal; a live process with that PID
    /// that started after the run is another process reusing it.
    pub fn runner_alive(&self) -> bool {
        let Some(pid) = self
            .directory
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.split('-').nth(1))
            .and_then(|pid| u32::from_str_radix(pid, 16).ok())
        else {
            // An unknown naming scheme is not evidence of a dead runner.
            return true;
        };
        oer_os_process::proc::started_unix_millis(pid)
            // Boot time has a resolution of one second.
            .is_some_and(|process| process <= self.manifest.started_unix_millis + 2000)
    }

    /// A run recorded as running whose runner is gone: it ended without
    /// finishing its bundle.
    pub fn abandoned(&self) -> bool {
        self.manifest.state == RunState::Running && !self.runner_alive()
    }
}
