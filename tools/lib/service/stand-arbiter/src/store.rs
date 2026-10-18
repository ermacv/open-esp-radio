//! The arbiter directory and its locked state transactions.

use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::{
    history::{self, LeaseOutcome, LeaseRecord},
    state::{STATE_SCHEMA, State},
};

use oer_stand_file::paths::ARBITER_ENV;

/// One stand per host user: every checkout shares this directory, as the
/// fixture leases already do.
#[derive(Clone, Debug)]
pub struct Arbiter {
    directory: PathBuf,
    /// The stand file the arbiter reads the stand's boards from.
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
    /// The stand's arbiter: its state in the stand model's arbiter directory
    /// (`$OER_STAND_ARBITER_DIR` for a private one), its boards from the
    /// user's stand file, which a private arbiter shares. A test process is
    /// refused the user's one: its leases, balances and board records would
    /// be the stand's.
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
        Ok(Self::at(oer_stand_file::paths::arbiter()?)?
            .with_stand_file(oer_stand_file::paths::stand_file()?))
    }

    /// An arbiter in `directory`, which reads the stand's boards from the
    /// `stand.toml` beside its state: a test's own.
    pub fn at(directory: impl Into<PathBuf>) -> crate::Result<Self> {
        let directory = directory.into();
        fs::create_dir_all(&directory)?;
        Ok(Self {
            stand_file: directory.join("stand.toml"),
            directory,
        })
    }

    /// This arbiter reading the stand's boards from `stand_file`: the one a
    /// runner was given with `--stand-file`.
    pub fn with_stand_file(mut self, stand_file: PathBuf) -> Self {
        self.stand_file = stand_file;
        self
    }

    /// The stand file this arbiter reads the stand's boards from.
    pub fn stand_file(&self) -> &Path {
        &self.stand_file
    }

    /// The stand file, loaded.
    pub fn stand(&self) -> crate::Result<oer_stand_file::StandFile> {
        Ok(oer_stand_file::StandFile::load(&self.stand_file)?)
    }

    /// Whether the stand's state was written by a newer build, whose schema
    /// this build cannot read.
    pub fn outdated(&self) -> bool {
        fs::read(self.directory.join("state.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .and_then(|state| state["schema"].as_u64())
            .is_some_and(|schema| schema > u64::from(STATE_SCHEMA))
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }

    pub(crate) fn history_path(&self) -> PathBuf {
        self.directory.join("history.jsonl")
    }

    /// The board journal beside this arbiter's state.
    pub fn journal(&self) -> oer_stand_journal::Journal {
        oer_stand_journal::Journal::at(self.directory.join("board.jsonl"))
    }

    /// Read, reap and possibly modify the state under the exclusive lock.
    /// Leases and tickets of processes that no longer exist are removed first.
    pub(crate) fn transaction<T>(
        &self,
        action: impl FnOnce(&mut State) -> crate::Result<T>,
    ) -> crate::Result<T> {
        self.locked(|| self.state_transaction(action))
    }

    /// Run `action` under the exclusive lock of every arbiter file.
    pub(crate) fn locked<T>(&self, action: impl FnOnce() -> crate::Result<T>) -> crate::Result<T> {
        let lock = oer_process::lock::FileLock::acquire(
            &self.directory.join("arbiter.lock"),
            oer_process::lock::Mode::Exclusive,
        )?;
        let result = action();
        drop(lock);
        result
    }

    fn state_transaction<T>(
        &self,
        action: impl FnOnce(&mut State) -> crate::Result<T>,
    ) -> crate::Result<T> {
        let path = self.directory.join("state.json");
        let before = match fs::read(&path) {
            Ok(bytes) => {
                let value: serde_json::Value = serde_json::from_slice(&bytes)?;
                let schema = value["schema"].as_u64();
                if schema != Some(u64::from(STATE_SCHEMA)) {
                    return Err(crate::state::unreadable(&path, &value["schema"]).into());
                }
                serde_json::from_value(value)?
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => State::default(),
            Err(error) => return Err(error.into()),
        };
        let mut state = before.clone();
        crate::balance::settle(&mut state, oer_durable::unix_millis());
        self.reap(&mut state)?;
        let result = action(&mut state)?;
        if state != before {
            oer_durable::atomic_write(&path, &serde_json::to_vec_pretty(&state)?)?;
        }
        Ok(result)
    }

    fn reap(&self, state: &mut State) -> crate::Result<()> {
        state.queue.retain(|ticket| ticket.process.alive());
        let (live, dead): (Vec<_>, Vec<_>) = std::mem::take(&mut state.holders)
            .into_iter()
            .partition(|holder| holder.ticket.process.alive());
        state.holders = live;
        for holder in dead {
            history::append(
                &self.history_path(),
                &LeaseRecord {
                    id: holder.ticket.id,
                    owner: holder.ticket.owner.clone(),
                    work: holder.ticket.work.clone(),
                    granted_unix: holder.granted_unix,
                    released_unix: oer_durable::unix_seconds(),
                    outcome: if holder.preempted.is_some() {
                        LeaseOutcome::PreemptedOnRequest
                    } else {
                        LeaseOutcome::Abandoned
                    },
                    charged_ms: crate::preempt::charged_ms(
                        &holder,
                        oer_durable::unix_seconds()
                            .saturating_sub(holder.granted_unix)
                            .saturating_mul(1000),
                    ),
                    balance_after_ms: crate::balance::of(&state.balances, &holder.ticket.owner),
                    reason: holder.reason.clone(),
                    preempted: holder.preempted.clone(),
                    scenarios: Vec::new(),
                    run: holder.ticket.run.clone(),
                    unknown: Default::default(),
                },
            )?;
            crate::notify::send(
                "HIL stand lease abandoned",
                &format!(
                    "lease of {} ({}) ended without release",
                    holder.ticket.owner, holder.ticket.work
                ),
            );
        }
        Ok(())
    }

    pub fn history(&self) -> crate::Result<Vec<LeaseRecord>> {
        history::read(&self.history_path())
    }
}

#[cfg(test)]
mod guard_tests {
    use super::*;

    #[test]
    fn a_test_process_is_refused_the_stands_arbiter() {
        assert!(is_test_executable(Path::new(
            "/r/target/debug/deps/oer_stand_arbiter-1a2b"
        )));
        assert!(!is_test_executable(Path::new("/r/target/debug/oer-xtask")));
        // This very test runs from deps/.
        if std::env::var_os(ARBITER_ENV).is_none() {
            let error = Arbiter::open().err().unwrap().to_string();
            assert!(error.contains("a test must not open"), "{error}");
        }
    }
}
