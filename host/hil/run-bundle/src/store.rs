//! The run store every checkout of this user shares, and its sidecars.
//!
//! HIL run bundles are self-contained and immutable once finished, and the stand is shared by
//! all checkouts, so their runs, every chip's, live in one store; a run's
//! manifest names its chip. A checkout's [`CHECKOUT_RUNS`] is a symbolic link
//! to the store's `runs/`; everything else below `target/hil` (build caches,
//! snapshots) stays per checkout. Beside `runs/` the store keeps the observer
//! builds its runs refer to ([`oer_hil_run_bundle_format::observer::store`]) and its
//! [`Sidecar`]s.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::Result;
use oer_hil_run_bundle_format::read::RunBundle;

/// Overrides the store's root directory.
pub const ENV: &str = "OER_HIL_STORE";

/// Where a checkout reaches the store's runs, relative to its root.
pub const CHECKOUT_RUNS: &str = "target/hil/runs";

/// One run store: `runs/`, the observer builds and the sidecars.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunStore {
    root: PathBuf,
}

/// A file the store keeps beside its runs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Sidecar {
    /// Runs pinned by owners: [`Notes`] by run id.
    Pins,
    /// Scenarios left out of a run of every scenario: [`Notes`] by scenario.
    Quarantine,
    /// The reviewed performance baseline of each scenario.
    PerfBaselines,
}

impl Sidecar {
    pub const fn file(self) -> &'static str {
        match self {
            Self::Pins => "pins.json",
            Self::Quarantine => "quarantine.json",
            Self::PerfBaselines => "perf-baselines.json",
        }
    }
}

/// Why an owner pinned a run or quarantined a scenario.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Note {
    pub by: String,
    pub reason: String,
    pub unix_millis: u64,
}

/// The notes of a [`Sidecar::Pins`] or [`Sidecar::Quarantine`] file.
pub type Notes = BTreeMap<String, Note>;

impl RunStore {
    /// The store at `$OER_HIL_STORE`, or the user's data directory's.
    pub fn shared() -> Result<Self> {
        Ok(Self::at(oer_os_durable::xdg::overridable(
            ENV,
            oer_os_durable::xdg::Base::Data,
            "hil",
        )?))
    }

    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The store whose runs directory `runs` is, or links to.
    pub fn of_runs(runs: &Path) -> Result<Self> {
        Ok(Self::at(
            fs::canonicalize(runs)?
                .parent()
                .ok_or("HIL runs directory has no parent")?,
        ))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn runs(&self) -> PathBuf {
        self.root.join("runs")
    }

    /// Runs another revision's runner wrote in that revision's own bundle
    /// format, kept beside the store's runs so they outlive the checkout
    /// that launched them. Nothing reads them as runs of this store, and no
    /// retention or size budget deletes them: they are removed by hand.
    pub fn revision_runs(&self) -> PathBuf {
        self.root.join("revision-runs")
    }

    pub fn run(&self, id: &str) -> PathBuf {
        self.runs().join(id)
    }

    /// The published run `id`.
    pub fn open(&self, id: &str) -> Result<RunBundle> {
        if id.is_empty() || Path::new(id).components().count() != 1 || id.starts_with('.') {
            return Err(format!("invalid HIL run id `{id}`").into());
        }
        RunBundle::open(&self.run(id))?
            .ok_or_else(|| format!("no run {id} in the run store {}", self.runs().display()).into())
    }

    /// The names of the run directories, newest first: a run's directory
    /// name starts with its start time.
    pub fn ids_newest_first(&self) -> Result<Vec<String>> {
        let entries = match fs::read_dir(self.runs()) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let mut names = Vec::new();
        for entry in entries {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                names.push(entry.file_name().to_string_lossy().into_owned());
            }
        }
        names.sort_unstable_by(|a, b| b.cmp(a));
        Ok(names)
    }

    /// Every published run, oldest first. A directory without a manifest
    /// (a run being created) or with one this build cannot read is skipped.
    pub fn bundles(&self) -> Result<Vec<RunBundle>> {
        let mut found = self
            .ids_newest_first()?
            .into_iter()
            .filter_map(|id| RunBundle::open(&self.run(&id)).ok().flatten())
            .collect::<Vec<_>>();
        found.sort_by(|a, b| {
            (a.manifest().started_unix_millis, a.id())
                .cmp(&(b.manifest().started_unix_millis, b.id()))
        });
        Ok(found)
    }

    /// The directory holding the observer builds the runs refer to.
    pub fn observers(&self) -> &Path {
        &self.root
    }

    /// Where the store caches derived per-run summaries.
    pub fn cache(&self, name: &str) -> PathBuf {
        self.root.join(format!("{name}-cache"))
    }

    pub fn sidecar_path(&self, sidecar: Sidecar) -> PathBuf {
        self.root.join(sidecar.file())
    }

    /// The sidecar's contents; its default while it does not exist.
    pub fn read<T: DeserializeOwned + Default>(&self, sidecar: Sidecar) -> Result<T> {
        let path = self.sidecar_path(sidecar);
        match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|error| format!("{}: {error}", path.display()).into()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
            Err(error) => Err(error.into()),
        }
    }

    pub fn write<T: Serialize>(&self, sidecar: Sidecar, value: &T) -> Result<()> {
        fs::create_dir_all(&self.root)?;
        oer_os_durable::atomic_json(&self.sidecar_path(sidecar), value)
    }

    /// Set or, with `None`, remove `key`'s note in a notes sidecar.
    pub fn note(&self, sidecar: Sidecar, key: &str, note: Option<Note>) -> Result<()> {
        self.update_notes(sidecar, |notes| {
            match note {
                Some(note) => notes.insert(key.to_owned(), note),
                None => notes.remove(key),
            };
        })
    }

    /// Change a notes sidecar with `change` as one read-modify-write under
    /// the sidecar's lock file: every checkout shares the store, and a
    /// concurrent change of another process is never lost.
    pub fn update_notes<R>(
        &self,
        sidecar: Sidecar,
        change: impl FnOnce(&mut Notes) -> R,
    ) -> Result<R> {
        let _lock = self.lock_notes(sidecar, oer_os_process::lock::Mode::Exclusive)?;
        let mut notes: Notes = self.read(sidecar)?;
        let result = change(&mut notes);
        self.write(sidecar, &notes)?;
        Ok(result)
    }

    /// The lock of a notes sidecar: [`Self::update_notes`] holds it
    /// exclusively; a reader that acts on the notes, such as pruning, which
    /// deletes every run its pins do not keep, holds it shared from reading
    /// them until its last run deletion, so no pin lands in between.
    pub fn lock_notes(
        &self,
        sidecar: Sidecar,
        mode: oer_os_process::lock::Mode,
    ) -> Result<oer_os_process::lock::FileLock> {
        use oer_os_process::lock::{FileLock, Mode};
        fs::create_dir_all(&self.root)?;
        // Waiting says why once and ends on Ctrl-C (exit 130), unlike a
        // blocking `flock`.
        let waiting = match (mode, sidecar) {
            // Pruning takes only the pins' lock.
            (Mode::Exclusive, Sidecar::Pins) => format!(
                "hil: waiting for another change of the run store's {} or a pruning to finish",
                sidecar.file()
            ),
            (Mode::Exclusive, _) => format!(
                "hil: waiting for another change of the run store's {} to finish",
                sidecar.file()
            ),
            (Mode::Shared, _) => format!(
                "hil: waiting for a change of the run store's {} to finish",
                sidecar.file()
            ),
        };
        FileLock::wait(
            &self.root.join(format!("{}.lock", sidecar.file())),
            mode,
            &waiting,
        )
    }

    /// Make the checkout's [`CHECKOUT_RUNS`] a link to this store's runs. A
    /// run directory of its own that holds runs is refused: every checkout
    /// has used the store since it was introduced, so such runs were written
    /// past it.
    pub fn link(&self, checkout: &Path) -> Result<Linked> {
        let shared = self.runs();
        let local = checkout.join(CHECKOUT_RUNS);
        fs::create_dir_all(&shared)?;
        match fs::symlink_metadata(&local) {
            Ok(metadata) if metadata.file_type().is_symlink() => return Ok(Linked::Existing),
            Ok(metadata) if metadata.is_dir() => {
                if fs::read_dir(&local)?.next().is_some() {
                    return Err(format!(
                        "{} holds runs outside the shared store {}; move them there",
                        local.display(),
                        shared.display()
                    )
                    .into());
                }
                fs::remove_dir(&local)?;
            }
            Ok(_) => return Err(format!("{} is not a directory", local.display()).into()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir_all(local.parent().ok_or("run directory has no parent")?)?;
            }
            Err(error) => return Err(error.into()),
        }
        std::os::unix::fs::symlink(&shared, &local)?;
        Ok(Linked::Created)
    }
}

#[derive(Debug, PartialEq)]
pub enum Linked {
    /// The checkout already uses a link.
    Existing,
    /// A new link, in place of no directory or an empty one.
    Created,
}

/// The run `path` (a run directory or a file below one) belongs to: the
/// nearest ancestor directory holding a run manifest.
pub fn run_of(path: &Path) -> Option<String> {
    path.ancestors()
        .find(|directory| {
            directory
                .join(oer_hil_run_bundle_format::read::MANIFEST)
                .is_file()
        })
        .and_then(Path::file_name)
        .map(|name| name.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests;
