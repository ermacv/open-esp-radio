//! Which runs a runner process created: the run receipt.
//!
//! Whoever starts a runner names a fresh file in [`ENV`]; the runner appends
//! the id of every run it creates, one per line, as it publishes the run.
//! The receipt is the only way a launcher learns its runs: it never guesses
//! them from the store's newest directories.

use std::{
    fs::OpenOptions,
    io::Write as _,
    path::{Path, PathBuf},
};

use crate::Result;

/// Names the file the runner appends the ids of the runs it creates to.
pub const ENV: &str = "OER_HIL_RUN_RECEIPT";

/// The id of one run: its directory's name in the store.
#[derive(
    Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, serde::Deserialize, serde::Serialize,
)]
#[serde(transparent)]
pub struct RunId(String);

impl RunId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for RunId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl AsRef<Path> for RunId {
    fn as_ref(&self) -> &Path {
        Path::new(&self.0)
    }
}

/// A receipt a launcher hands one runner process.
pub struct Receipt {
    file: tempfile::NamedTempFile,
}

impl Receipt {
    pub fn new() -> Result<Self> {
        Ok(Self {
            file: tempfile::NamedTempFile::new()?,
        })
    }

    /// The path the runner's [`ENV`] names.
    pub fn path(&self) -> &Path {
        self.file.path()
    }

    /// The runs the runner recorded, in creation order.
    pub fn runs(&self) -> Result<Vec<RunId>> {
        read(self.path())
    }
}

/// The runs a receipt at `path` names, in creation order.
pub fn read(path: &Path) -> Result<Vec<RunId>> {
    Ok(std::fs::read_to_string(path)?
        .lines()
        .filter(|line| !line.is_empty())
        .map(RunId::new)
        .collect())
}

/// Append `runs` to the receipt the invoking process named in [`ENV`], when
/// it named one: the runner records each run it creates, and a launcher
/// that runs on behalf of another forwards the runs of its runner.
pub fn record(runs: &[RunId]) -> Result<()> {
    let Some(path) = std::env::var_os(ENV) else {
        return Ok(());
    };
    append(&PathBuf::from(path), runs)
}

fn append(path: &Path, runs: &[RunId]) -> Result<()> {
    let mut receipt = OpenOptions::new().create(true).append(true).open(path)?;
    for run in runs {
        writeln!(receipt, "{run}")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_receipt_names_its_runs_in_creation_order() {
        let receipt = Receipt::new().unwrap();
        assert!(receipt.runs().unwrap().is_empty());
        append(receipt.path(), &[RunId::new("1-a")]).unwrap();
        append(receipt.path(), &[RunId::new("2-b"), RunId::new("3-c")]).unwrap();
        assert_eq!(
            receipt.runs().unwrap(),
            [RunId::new("1-a"), RunId::new("2-b"), RunId::new("3-c")]
        );
    }
}
