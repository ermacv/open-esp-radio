//! The writer of a run bundle: canonical, immutable records for one host
//! HIL invocation.

use std::{
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::Write as _,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use crate::Result;
use crate::build;
use oer_hil_schema::image::ImageClass;

mod archive;
pub use archive::FirmwareArchive;
mod target;

// The documents this writer writes are the format's.
use oer_hil_run_bundle_format::run::*;
pub(crate) use oer_os_durable::{atomic_json, atomic_write, sha256_file, unix_millis};

pub struct RunSession {
    target_directory: PathBuf,
    directory: PathBuf,
    manifest: RunManifest,
    started: Instant,
    events: File,
    finished: bool,
    interruption: Option<InterruptionSink>,
}

/// The CI and human views a run derives from its suite and manifest. They
/// are kept with the run but are never proof inputs.
#[derive(Clone, Debug, Default)]
pub struct Views {
    pub junit: String,
    pub html: String,
}

/// Renders a finished run's [`Views`]; the runner passes the analysis crate's.
pub type Render = fn(&SuiteResult, &RunManifest) -> Views;

/// Where an interrupted run hands its report.
type InterruptionSink = Box<dyn FnOnce(&InterruptionReport) + Send>;

/// What a run that ends without finishing reports once it is recorded as
/// interrupted.
#[derive(Debug, serde::Serialize)]
pub struct InterruptionReport {
    pub schema: u16,
    pub run_id: String,
    pub outcome: &'static str,
    pub run_directory: PathBuf,
}

struct UnpublishedRunDirectory {
    path: PathBuf,
    published: bool,
}

impl UnpublishedRunDirectory {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            published: false,
        }
    }

    fn publish(&mut self) {
        self.published = true;
    }
}

impl Drop for UnpublishedRunDirectory {
    fn drop(&mut self) {
        if !self.published {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

impl RunSession {
    pub fn create(
        root: &Path,
        target: &str,
        cell_id: &str,
        device_id: &str,
        serial_device: &Path,
        invocation: Vec<OsString>,
    ) -> Result<Self> {
        let started = Instant::now();
        let started_unix_millis = unix_millis();
        let run_id = create_run_id(started_unix_millis);
        // Build objects stay per chip; every chip's runs share one store.
        let target_directory = root.join("target/hil").join(target);
        let runs = root.join(crate::store::CHECKOUT_RUNS);
        fs::create_dir_all(&runs)?;
        let directory = create_unique_directory(&runs, &run_id)?;
        let mut unpublished_directory = UnpublishedRunDirectory::new(directory.clone());
        let run_id = directory
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or("HIL run directory does not have a UTF-8 name")?
            .to_owned();
        let repository = build::repository(root)?;
        let mut runner = runner_provenance()?;
        // The observer build is shared by the runs of one observer: it is
        // stored once beside the runs directory, where that directory really
        // is when it links to the shared store, and the manifest names it by
        // digest.
        if let Some(observer) = &runner.observer {
            runner.observer = Some(oer_hil_run_bundle_format::observer::store::detach(
                observer,
                crate::store::RunStore::of_runs(&runs)?.observers(),
            )?);
        }
        let events = OpenOptions::new()
            .create_new(true)
            .append(true)
            .open(directory.join("events.jsonl"))?;
        let manifest = RunManifest {
            schema: RUN_SCHEMA,
            run_id,
            target: target.to_owned(),
            state: RunState::Running,
            started_unix_millis,
            finished_unix_millis: None,
            duration_millis: None,
            invocation: invocation
                .into_iter()
                .map(|argument| argument.to_string_lossy().into_owned())
                .collect(),
            repository,
            runner,
            cell: CellProvenance {
                cell_id: cell_id.to_owned(),
                device_id: device_id.to_owned(),
                serial_device: serial_device.to_owned(),
            },
            lab_provenance_path: None,
            firmware: Vec::new(),
            experiment: oer_hil_run_bundle_format::experiment::Experiment::from_environment()?,
            messages_used: Vec::new(),
        };
        atomic_json(&directory.join("manifest.json"), &manifest)?;
        crate::launch_runs::record(&[crate::launch_runs::RunId::new(&manifest.run_id)])?;
        let mut session = Self {
            target_directory,
            directory,
            manifest,
            started,
            events,
            finished: false,
            interruption: None,
        };
        session.record_event(RunEventKind::RunStarted, None, None, None)?;
        unpublished_directory.publish();
        Ok(session)
    }

    pub fn id(&self) -> &str {
        &self.manifest.run_id
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// The source the run's current-source images are built from.
    pub fn repository(&self) -> &RepositoryProvenance {
        &self.manifest.repository
    }

    pub fn write_plan(&self, plan: &RunPlan) -> Result<()> {
        atomic_json(&self.directory.join("plan.json"), plan)
    }

    pub fn record_lab_provenance(
        &mut self,
        provenance: &oer_hil_run_bundle_format::lab::LabProvenance,
    ) -> Result<()> {
        let path = PathBuf::from("lab-provenance.json");
        atomic_json(&self.directory.join(&path), provenance)?;
        self.manifest.lab_provenance_path = Some(path);
        atomic_json(&self.directory.join("manifest.json"), &self.manifest)
    }

    pub fn scenario_directory(&self, scenario: &str) -> PathBuf {
        self.directory.join("scenarios").join(scenario)
    }

    pub fn record_event(
        &mut self,
        kind: RunEventKind,
        scenario: Option<&str>,
        image: Option<ImageClass>,
        outcome: Option<Outcome>,
    ) -> Result<()> {
        let mut record = serde_json::to_vec(&EventRecord {
            timestamp_unix_millis: unix_millis(),
            kind,
            scenario,
            image,
            outcome,
        })?;
        record.push(b'\n');
        self.events.write_all(&record)?;
        self.events.sync_data()?;
        Ok(())
    }

    /// Finish the run with its scenarios' results: the suite, the views
    /// `render` derives from it, then the final manifest.
    pub fn finish(
        mut self,
        scenarios: Vec<ScenarioResult>,
        render: Render,
    ) -> Result<(SuiteResult, CompletionReport)> {
        let finished_unix_millis = unix_millis();
        let duration_millis = duration_millis(self.started.elapsed());
        let counts = SuiteCounts::from_results(&scenarios);
        let outcome = if scenarios.iter().all(|result| result.outcome.is_passed()) {
            Outcome::Passed
        } else {
            Outcome::Failed
        };
        let suite = SuiteResult {
            schema: RUN_SCHEMA,
            run_id: self.manifest.run_id.clone(),
            target: self.manifest.target.clone(),
            outcome,
            started_unix_millis: self.manifest.started_unix_millis,
            finished_unix_millis,
            duration_millis,
            counts,
            scenarios,
        };
        atomic_json(&self.directory.join("suite.json"), &suite)?;
        let views = render(&suite, &self.manifest);
        atomic_write(&self.directory.join("junit.xml"), views.junit.as_bytes())?;
        atomic_write(&self.directory.join("report.html"), views.html.as_bytes())?;
        self.record_event(RunEventKind::RunFinished, None, None, Some(outcome))?;
        self.manifest.messages_used = messages_used(&self.directory)?;
        self.manifest.state = RunState::Completed;
        self.manifest.finished_unix_millis = Some(finished_unix_millis);
        self.manifest.duration_millis = Some(duration_millis);
        atomic_json(&self.directory.join("manifest.json"), &self.manifest)?;
        self.finished = true;
        let completion = CompletionReport {
            schema: RUN_SCHEMA,
            run_id: self.manifest.run_id.clone(),
            outcome,
            run_directory: self.directory.clone(),
            suite_report: self.directory.join("suite.json"),
            junit_report: self.directory.join("junit.xml"),
            html_report: self.directory.join("report.html"),
        };
        Ok((suite, completion))
    }
}

impl RunSession {
    /// Hand the report of an interrupted run to `report`, which the caller
    /// publishes where it publishes run outcomes.
    pub fn report_interruption_to(
        &mut self,
        report: impl FnOnce(&InterruptionReport) + Send + 'static,
    ) {
        self.interruption = Some(Box::new(report));
    }
}

impl Drop for RunSession {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        let _ = self.record_event(
            RunEventKind::RunInterrupted,
            None,
            None,
            Some(Outcome::Interrupted),
        );
        self.manifest.messages_used = messages_used(&self.directory).unwrap_or_default();
        self.manifest.state = RunState::Interrupted;
        self.manifest.finished_unix_millis = Some(unix_millis());
        self.manifest.duration_millis = Some(duration_millis(self.started.elapsed()));
        match atomic_json(&self.directory.join("manifest.json"), &self.manifest) {
            Ok(()) => {
                if let Some(report) = self.interruption.take() {
                    report(&InterruptionReport {
                        schema: RUN_SCHEMA,
                        run_id: self.manifest.run_id.clone(),
                        outcome: "interrupted",
                        run_directory: self.directory.clone(),
                    });
                }
            }
            Err(error) => eprintln!("cannot record the interrupted HIL run: {error}"),
        }
    }
}

/// Every message path the run's protocol captures record: the target's
/// messages and the host's requests, sorted and unique.
fn messages_used(directory: &Path) -> Result<Vec<String>> {
    fn captures(directory: &Path, found: &mut Vec<PathBuf>) -> Result<()> {
        for entry in fs::read_dir(directory)? {
            let path = entry?.path();
            if path.is_dir() {
                captures(&path, found)?;
            } else if path
                .file_name()
                .is_some_and(|name| name == "protocol.jsonl")
            {
                found.push(path);
            }
        }
        Ok(())
    }
    let mut files = Vec::new();
    captures(directory, &mut files)?;
    let mut paths = std::collections::BTreeSet::new();
    for file in files {
        for line in fs::read_to_string(&file)?.lines() {
            let record: serde_json::Value = serde_json::from_str(line)
                .map_err(|error| format!("{}: {error}", file.display()))?;
            let path = match record["record"].as_str() {
                Some("target-event") => &record["message"]["path"],
                Some("host-command") => &record["command"]["path"],
                _ => continue,
            };
            if let Some(path) = path.as_str() {
                paths.insert(path.to_owned());
            }
        }
    }
    Ok(paths.into_iter().collect())
}

pub fn duration_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn create_run_id(started_unix_millis: u64) -> String {
    format!("{started_unix_millis}-{:08x}", std::process::id())
}

fn create_unique_directory(parent: &Path, base: &str) -> Result<PathBuf> {
    for suffix in 0_u16..=u16::MAX {
        let name = if suffix == 0 {
            base.to_owned()
        } else {
            format!("{base}-{suffix:04}")
        };
        let path = parent.join(name);
        match fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
    }
    Err("cannot allocate a unique HIL run directory".into())
}

/// Build identity of the executable that records runs.
#[derive(Clone, Copy, Debug)]
pub struct RunnerBuild {
    /// The executable's embedded host build record, as JSON.
    pub record: &'static str,
    pub package: &'static str,
    pub version: &'static str,
}

static RUNNER_BUILD: std::sync::OnceLock<RunnerBuild> = std::sync::OnceLock::new();

/// Register the running executable's identity before it records any run.
pub fn register_runner(build: RunnerBuild) -> Result<()> {
    RUNNER_BUILD
        .set(build)
        .map_err(|_| "runner build identity was registered more than once".into())
}

fn runner_build() -> Result<RunnerBuild> {
    // Unit tests record runs without the runner executable's build script.
    #[cfg(test)]
    let _ = RUNNER_BUILD.set(RunnerBuild {
        record: "{}",
        package: "oer-hil-runner",
        version: "0.0.0",
    });
    RUNNER_BUILD
        .get()
        .copied()
        .ok_or_else(|| "runner build identity is not registered".into())
}

pub fn runner_provenance() -> Result<RunnerProvenance> {
    // On Linux this reads the actual running inode even after a rebuild replaces
    // the pathname. The source record comes from this executable's build script.
    #[cfg(target_os = "linux")]
    let executable_sha256 = Some(sha256_file(Path::new("/proc/self/exe"))?);
    // A pathname on another host does not establish the running inode after
    // replacement. Retain the embedded build and leave this identity unknown.
    #[cfg(not(target_os = "linux"))]
    let executable_sha256: Option<String> = None;
    let runner = runner_build()?;
    let build: serde_json::Value = serde_json::from_str(runner.record)?;
    let build_sha256 = oer_os_durable::sha256_bytes(&serde_json::to_vec(&build)?);
    Ok(RunnerProvenance {
        observer: Some(
            serde_json::json!({"schema":1,"executable_sha256":executable_sha256,"build_sha256":build_sha256,"build":build}),
        ),
        package: runner.package.to_owned(),
        version: runner.version.to_owned(),
        messages_lock_sha256: oer_os_durable::sha256_bytes(
            oer_hil_protocol::MESSAGES_LOCK.as_bytes(),
        ),
        host_os: std::env::consts::OS.to_owned(),
        host_arch: std::env::consts::ARCH.to_owned(),
        tools: oer_os_toolchain::versions()
            .into_iter()
            .map(|tool| ToolVersion {
                name: tool.tool.name().to_owned(),
                version: tool.version,
            })
            .collect(),
    })
}

#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
#[cfg(test)]
mod tests;
