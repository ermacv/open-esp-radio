//! The source a run's images are built from: the checkout's tree and the
//! paths Git names as changed, read once before the build.

use std::path::Path;

use crate::stand::Result;

/// The checkout's tree (`git rev-parse HEAD^{tree}`) and the paths `git
/// status --porcelain` names, empty for a clean checkout. A dirty checkout
/// is not refused: the results file says so.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub(crate) struct Source {
    pub(crate) tree: String,
    pub(crate) dirty: Vec<String>,
}

impl Source {
    /// Read the source of the checkout at `root`.
    ///
    /// # Errors
    ///
    /// Git cannot name the tree or the status: the run would not know what
    /// it ran, so it does not build.
    pub(crate) fn read(root: &Path) -> Result<Self> {
        let tree = oer_tools_os_process::git::text(root, ["rev-parse", "HEAD^{tree}"])?;
        let status = oer_tools_os_process::git::output(root, ["status", "--porcelain", "-z"])?;
        Ok(Self {
            tree,
            dirty: dirty_paths(&status),
        })
    }
}

/// The paths of `git status --porcelain -z`: each entry is two status
/// letters, a space and its path; a rename or a copy (`R` or `C` first) is
/// followed by the path it came from, which is named too.
fn dirty_paths(status: &[u8]) -> Vec<String> {
    let mut entries = status
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty());
    let mut paths = Vec::new();
    while let Some(entry) = entries.next() {
        let path = entry.get(3..).unwrap_or_default();
        paths.push(String::from_utf8_lossy(path).into_owned());
        if matches!(entry.first(), Some(b'R' | b'C'))
            && let Some(origin) = entries.next()
        {
            paths.push(String::from_utf8_lossy(origin).into_owned());
        }
    }
    paths
}

#[cfg(test)]
mod tests;
