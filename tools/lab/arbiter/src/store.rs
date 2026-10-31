//! The arbiter directory: the service's state, the lease history and the
//! board journal, and the stand file the service reads.

use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::history::{self, LeaseRecord};

use oer_lab_model::paths::ARBITER_ENV;

/// One stand per host: the service keeps its records in this directory.
#[derive(Clone, Debug)]
pub struct Arbiter {
    directory: PathBuf,
    /// The stand file the service reads the stand's boards from.
    stand_file: PathBuf,
}

/// Whether `exe` is a Cargo test harness: Cargo places test executables in
/// a `deps` directory of its target directory, and binaries beside it.
fn is_test_executable(exe: &Path) -> bool {
    exe.parent()
        .and_then(Path::file_name)
        .is_some_and(|directory| directory == "deps")
}

impl Arbiter {
    /// The stand's directory in the stand model's arbiter directory
    /// (`$OER_STAND_ARBITER_DIR` for a private one), its boards from the
    /// user's stand file. A test process is refused the user's one: its
    /// leases, balances and board records would be the stand's.
    pub fn open() -> crate::Result<Self> {
        if std::env::var_os(ARBITER_ENV).is_none_or(|value| value.is_empty())
            && std::env::current_exe().is_ok_and(|exe| is_test_executable(&exe))
        {
            return Err(format!(
                "a test must not open the stand's arbiter: use Arbiter::at with a temporary \
                 directory, or set {ARBITER_ENV}"
            )
            .into());
        }
        Ok(Self::at(oer_lab_model::paths::arbiter()?)?
            .with_stand_file(oer_lab_model::paths::stand_file()?))
    }

    /// An arbiter in `directory`, which reads the stand's boards from the
    /// `stand.toml` beside it: a test's own.
    pub fn at(directory: impl Into<PathBuf>) -> crate::Result<Self> {
        let directory = directory.into();
        fs::create_dir_all(&directory)?;
        Ok(Self {
            stand_file: directory.join("stand.toml"),
            directory,
        })
    }

    /// The same directory with the stand's boards from `stand_file`.
    pub fn with_stand_file(mut self, stand_file: PathBuf) -> Self {
        self.stand_file = stand_file;
        self
    }

    pub fn stand_file(&self) -> &Path {
        &self.stand_file
    }

    /// The stand's boards, from the stand file.
    pub fn stand(&self) -> crate::Result<oer_lab_model::Stand> {
        Ok(oer_lab_model::Stand::load(&self.stand_file)?)
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// The service's persisted scheduler state.
    pub fn state_path(&self) -> PathBuf {
        self.directory.join("service.json")
    }

    pub fn history_path(&self) -> PathBuf {
        self.directory.join("history.jsonl")
    }

    /// The board journal.
    pub fn journal(&self) -> crate::journal::Journal {
        crate::journal::Journal::at(self.directory.join("board.jsonl"))
    }

    pub fn history(&self) -> crate::Result<Vec<LeaseRecord>> {
        history::read(&self.history_path())
    }
}
