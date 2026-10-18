//! Completed leases, the input of duration estimates and the status report.

use std::{fs, path::Path};

use serde::{Deserialize, Serialize};

/// A history larger than this keeps only its newest records.
const MAX_BYTES: u64 = 1 << 20;
const RETAINED_RECORDS: usize = 2000;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum LeaseOutcome {
    Released,
    /// The holder exited without releasing; another process reaped it.
    Abandoned,
    /// Released at a boundary to a waiter with a higher balance, to queue
    /// its remaining work.
    YieldedToBalance,
    /// Terminated at the hard limit every lease has.
    HardLimit,
    /// Stopped by `cargo stand preempt`; the record names who and why.
    PreemptedOnRequest,
}

/// Why a lease was granted: its owner's balance against the other owners
/// whose conflicting requests waited.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct GrantReason {
    pub balance_ms: i64,
    pub over: Vec<OwnerBalance>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct OwnerBalance {
    pub owner: String,
    pub balance_ms: i64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LeaseRecord {
    pub id: u64,
    pub owner: String,
    pub work: String,
    pub granted_unix: u64,
    pub released_unix: u64,
    pub outcome: LeaseOutcome,
    /// The held time charged to the owner.
    #[serde(default)]
    pub charged_ms: u64,
    /// The owner's balance at release.
    #[serde(default)]
    pub balance_after_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<GrantReason>,
    /// Who stopped the lease with `cargo stand preempt`, and why.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preempted: Option<crate::preempt::Preemption>,
    /// HIL scenarios the lease executed, when its holder named them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scenarios: Vec<String>,
    /// The HIL run the lease executed, when a runner requested it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    /// Fields a newer build wrote, kept when this build rewrites the record.
    #[serde(flatten)]
    pub unknown: crate::Unknown,
}

impl LeaseRecord {
    pub fn duration_secs(&self) -> u64 {
        self.released_unix.saturating_sub(self.granted_unix)
    }
}

/// Append a record. Callers hold the state lock, which also serializes the
/// rewrite that bounds the file.
pub(crate) fn append(path: &Path, record: &LeaseRecord) -> crate::Result<()> {
    oer_durable::jsonl::append(path, record)?;
    if fs::metadata(path)?.len() > MAX_BYTES {
        oer_durable::jsonl::retain_newest::<LeaseRecord>(path, RETAINED_RECORDS)?;
    }
    Ok(())
}

/// Records in append order; lines another version cannot parse are skipped.
/// Rename the owner `old` to `new` in every record. Callers hold the state
/// lock. Lines this build cannot read are kept as they are.
pub(crate) fn rename_owner(path: &Path, old: &str, new: &str) -> crate::Result<()> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    let mut renamed = String::with_capacity(text.len());
    for line in text.lines() {
        match serde_json::from_str::<LeaseRecord>(line) {
            Ok(mut record) if record.owner == old => {
                record.owner = new.to_owned();
                renamed.push_str(&serde_json::to_string(&record)?);
            }
            _ => renamed.push_str(line),
        }
        renamed.push('\n');
    }
    oer_durable::atomic_write(path, renamed.as_bytes())
}

/// The lease history, a telemetry view (estimates, status): lines another
/// version cannot parse are skipped.
pub(crate) fn read(path: &Path) -> crate::Result<Vec<LeaseRecord>> {
    oer_durable::jsonl::read_lossy(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn history_skips_unreadable_lines_and_stays_bounded() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("history.jsonl");
        assert!(read(&path).unwrap().is_empty());
        let record = LeaseRecord {
            id: 1,
            owner: "phy".into(),
            work: "run x".into(),
            granted_unix: 10,
            released_unix: 70,
            outcome: LeaseOutcome::Released,
            charged_ms: 60_000,
            balance_after_ms: -60_000,
            reason: None,
            preempted: None,
            scenarios: Vec::new(),
            run: None,
            unknown: Default::default(),
        };
        append(&path, &record).unwrap();
        fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"{\"future\":true}\n")
            .unwrap();
        append(&path, &record).unwrap();
        assert_eq!(read(&path).unwrap(), [record.clone(), record.clone()]);
        for _ in 0..RETAINED_RECORDS + 10 {
            oer_durable::jsonl::append(&path, &record).unwrap();
        }
        oer_durable::jsonl::retain_newest::<LeaseRecord>(&path, RETAINED_RECORDS).unwrap();
        assert_eq!(read(&path).unwrap().len(), RETAINED_RECORDS);
    }
}
