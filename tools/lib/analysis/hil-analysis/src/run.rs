//! One run as the analyses read it: its bundle and its suite.

use std::{
    fs,
    path::{Path, PathBuf},
};

use oer_hil_run_bundle::RunStore;
use oer_hil_run_bundle_format::RunBundle;
use oer_hil_run_bundle_format::read::MANIFEST;
use oer_hil_run_bundle_format::run::Outcome;
use oer_hil_run_bundle_format::run::RUN_SCHEMA;
use oer_hil_run_bundle_format::run::RunState;
use oer_hil_run_bundle_format::run::ScenarioResult;
use oer_hil_run_bundle_format::run::SuiteResult;

use crate::Result;

/// A published run this build cannot read as a [`Run`].
#[derive(Clone, Debug)]
pub struct Unreadable {
    pub id: String,
    pub directory: PathBuf,
    /// The schema its manifest names, when the manifest is JSON naming one.
    pub schema: Option<u64>,
    /// When it started: its manifest's `started_unix_millis`, else its
    /// directory's modification time.
    pub started_millis: u64,
    pub reason: String,
}

impl Unreadable {
    /// Whether its schema is older than this build's: no checkout of this
    /// repository, newer or older, reads it any more. A run of this or a
    /// newer schema may be a newer branch's that only this build cannot
    /// read; the store is shared by every checkout.
    pub fn older_schema(&self) -> bool {
        self.schema
            .is_some_and(|schema| schema < u64::from(RUN_SCHEMA))
    }

    /// Whether it names this build's schema.
    pub fn current_schema(&self) -> bool {
        self.schema == Some(u64::from(RUN_SCHEMA))
    }
}

/// A published run with its suite, when it has one.
#[derive(Clone, Debug)]
pub struct Run {
    pub bundle: RunBundle,
    pub suite: Option<SuiteResult>,
    /// Recorded as running, but its runner is gone.
    pub abandoned: bool,
}

impl Run {
    /// The run in `directory`; `None` when it has no manifest yet, or a
    /// manifest or suite outside this build's format.
    pub fn load(directory: &Path) -> Option<Self> {
        Self::of(RunBundle::open(directory).ok()??).ok()
    }

    /// The run `id` of `store`.
    pub fn open(store: &RunStore, id: &str) -> Result<Self> {
        Self::of(store.open(id)?)
    }

    /// Every readable run of `store`, oldest first.
    pub fn all(store: &RunStore) -> Result<Vec<Self>> {
        Ok(store
            .bundles()?
            .into_iter()
            .filter_map(|bundle| Self::of(bundle).ok())
            .collect())
    }

    /// Every published run of `store` that is not a [`Run`]: its manifest
    /// does not read, names another schema, or its suite does not read.
    pub fn unreadable(store: &RunStore) -> Result<Vec<Unreadable>> {
        let mut found = Vec::new();
        for id in store.ids_newest_first()? {
            let directory = store.run(&id);
            let reason = match RunBundle::open(&directory) {
                Ok(None) => continue,
                Ok(Some(bundle)) => match Self::of(bundle) {
                    Ok(_) => continue,
                    Err(error) => error.to_string(),
                },
                Err(error) => error.to_string(),
            };
            // The schema the manifest names, read without its types: a
            // manifest of another schema need not parse as this one.
            let manifest = fs::read(directory.join(MANIFEST))
                .ok()
                .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok());
            let schema = manifest
                .as_ref()
                .and_then(|manifest| manifest["schema"].as_u64());
            let started_millis = manifest
                .as_ref()
                .and_then(|manifest| manifest["started_unix_millis"].as_u64())
                .or_else(|| {
                    let modified = fs::metadata(&directory).ok()?.modified().ok()?;
                    let since = modified.duration_since(std::time::UNIX_EPOCH).ok()?;
                    u64::try_from(since.as_millis()).ok()
                })
                .unwrap_or(0);
            found.push(Unreadable {
                id,
                directory,
                schema,
                started_millis,
                reason,
            });
        }
        found.reverse();
        Ok(found)
    }

    /// `bundle` as a run; an error for a bundle of another schema, whose
    /// documents this build reads only by accident, or whose suite does not
    /// read.
    pub fn of(bundle: RunBundle) -> Result<Self> {
        if bundle.manifest().schema != RUN_SCHEMA {
            return Err(format!("run schema {}", bundle.manifest().schema).into());
        }
        let suite = bundle.suite()?;
        let abandoned = bundle.abandoned();
        Ok(Self {
            bundle,
            suite,
            abandoned,
        })
    }

    pub fn id(&self) -> &str {
        self.bundle.id()
    }

    pub fn directory(&self) -> &Path {
        self.bundle.directory()
    }

    pub fn started_millis(&self) -> u64 {
        self.bundle.manifest().started_unix_millis
    }

    pub fn state(&self) -> RunState {
        self.bundle.manifest().state
    }

    /// A completed or interrupted bundle can no longer change.
    pub fn is_sealed(&self) -> bool {
        matches!(self.state(), RunState::Completed | RunState::Interrupted)
    }

    /// The suite outcome; `None` when the run has no suite.
    pub fn outcome(&self) -> Option<Outcome> {
        self.suite.as_ref().map(|suite| suite.outcome)
    }

    /// The run's status for display and selection: its suite outcome, or
    /// else `abandoned` or its state.
    pub fn status(&self) -> &'static str {
        match self.outcome() {
            Some(outcome) => outcome.id(),
            None if self.abandoned => "abandoned",
            None => self.state().id(),
        }
    }

    pub fn scenarios(&self) -> &[ScenarioResult] {
        self.suite
            .as_ref()
            .map_or(&[], |suite| suite.scenarios.as_slice())
    }

    pub fn scenario(&self, id: &str) -> Option<&ScenarioResult> {
        self.scenarios()
            .iter()
            .find(|scenario| scenario.scenario == id)
    }

    pub fn commit(&self) -> &str {
        &self.bundle.manifest().repository.commit
    }

    pub fn dirty(&self) -> bool {
        self.bundle.manifest().repository.dirty
    }

    /// The commit's first nine characters, `+` when the tree was dirty.
    pub fn short_commit(&self) -> String {
        let commit = self.commit();
        let commit = if commit.is_empty() {
            "unknown"
        } else {
            &commit[..commit.len().min(9)]
        };
        if self.dirty() {
            format!("{commit}+")
        } else {
            commit.to_owned()
        }
    }
}
