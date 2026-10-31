//! The one import of what the stand kept before its LAB service, from the
//! directory the file-locked arbiter wrote (`state.json`, schema 3, and
//! `maintenance.json`): owners' balances carry over, an owner's maintenance
//! stays maintenance, and a quarantined board becomes unavailable with
//! unknown emission until a person returns it. The lease history, the board
//! journal and the owner registry move. A restriction is never dropped: a
//! record this build cannot read refuses the import, and nothing changes.

use std::{collections::BTreeMap, path::Path};

use serde::Deserialize;

use crate::{
    Arbiter,
    balance::Balance,
    readiness::{Emission, Readiness},
    state::State,
};

/// The schema of the file-locked arbiter's queue state.
const PREVIOUS_STATE: u64 = 3;
/// The schema of its boards out of service.
const PREVIOUS_MAINTENANCE: u64 = 1;
/// The records that move as they are.
const RECORDS: [&str; 3] = ["history.jsonl", "board.jsonl", "owners.json"];
/// What the directory the import read is renamed to.
pub const IMPORTED: &str = "arbiter.before-lab-service";

/// What an import carried over.
#[derive(Debug, Default, Eq, PartialEq)]
pub struct Imported {
    pub balances: usize,
    pub maintenance: usize,
    pub quarantined: usize,
    pub records: Vec<String>,
}

#[derive(Deserialize)]
struct PreviousState {
    schema: u64,
    #[serde(default)]
    holders: Vec<serde_json::Value>,
    #[serde(default)]
    balances: BTreeMap<String, Balance>,
}

#[derive(Deserialize)]
struct PreviousBoards {
    schema: u64,
    #[serde(default)]
    boards: Vec<PreviousEntry>,
}

#[derive(Deserialize)]
struct PreviousEntry {
    mac: String,
    owner: String,
    reason: String,
    since_unix: u64,
    #[serde(default)]
    kind: Option<String>,
}

/// Import `previous`, the file-locked arbiter's directory, into `arbiter`
/// before its service first starts, then rename `previous` to [`IMPORTED`]
/// beside it. `None` when there is nothing to import: no such directory, or
/// a service state already in `arbiter`.
pub fn previous(previous: &Path, arbiter: &Arbiter) -> crate::Result<Option<Imported>> {
    if !previous.is_dir() || arbiter.state_path().exists() {
        return Ok(None);
    }
    let mut state = State::default();
    let mut imported = Imported::default();
    if let Some(old) = read::<PreviousState>(&previous.join("state.json"))? {
        if old.schema != PREVIOUS_STATE {
            return Err(format!(
                "{} has schema {}; this build imports schema {PREVIOUS_STATE} only",
                previous.join("state.json").display(),
                old.schema
            )
            .into());
        }
        if !old.holders.is_empty() {
            return Err(format!(
                "{} still holds {} lease(s): let them end before the LAB service takes over",
                previous.join("state.json").display(),
                old.holders.len()
            )
            .into());
        }
        imported.balances = old.balances.len();
        state.balances = old.balances;
    }
    if let Some(old) = read::<PreviousBoards>(&previous.join("maintenance.json"))? {
        if old.schema != PREVIOUS_MAINTENANCE {
            return Err(format!(
                "{} has schema {}; this build imports schema {PREVIOUS_MAINTENANCE} only",
                previous.join("maintenance.json").display(),
                old.schema
            )
            .into());
        }
        for entry in old.boards {
            match entry.kind.as_deref() {
                Some("quarantine") => {
                    imported.quarantined += 1;
                    state.readiness.insert(
                        format!("board:{}", entry.mac),
                        Readiness::Unavailable {
                            reason: format!("quarantined before the LAB service: {}", entry.reason),
                            emission: Emission::Unknown,
                            generation: 1,
                        },
                    );
                }
                None | Some("maintenance") => {
                    imported.maintenance += 1;
                    state.maintenance.push(crate::Maintenance {
                        mac: entry.mac,
                        owner: entry.owner,
                        reason: entry.reason,
                        since_unix: entry.since_unix,
                        unknown: Default::default(),
                    });
                }
                Some(other) => {
                    return Err(format!(
                        "{} takes board {} out of service as `{other}`, which this build does \
                         not import",
                        previous.join("maintenance.json").display(),
                        entry.mac
                    )
                    .into());
                }
            }
        }
    }
    for record in RECORDS {
        let from = previous.join(record);
        let to = arbiter.directory().join(record);
        if from.exists() && !to.exists() {
            std::fs::copy(&from, &to)?;
            imported.records.push(record.to_owned());
        }
    }
    oer_os_durable::atomic_write(&arbiter.state_path(), &serde_json::to_vec_pretty(&state)?)?;
    let parent = previous
        .parent()
        .ok_or("the previous arbiter directory has no parent")?;
    std::fs::rename(previous, parent.join(IMPORTED))?;
    Ok(Some(imported))
}

/// The JSON at `path`, `None` when there is no such file.
fn read<T: for<'a> Deserialize<'a>>(path: &Path) -> crate::Result<Option<T>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
mod tests;
