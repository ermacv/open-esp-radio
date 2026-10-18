//! Build the HIL observer (the runner) and keep its private copy.
use crate::compile::compile;
use std::{
    fs,
    path::{Path, PathBuf},
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Where a checkout keeps the private copies of the runners it built,
/// relative to its root: one directory per executable digest. (The observer *builds* runs refer to live in the run store's
/// [`oer_tool_data_hil_run_bundle_format::observer::store::DIRECTORY`].)
pub const RUNNERS: &str = "target/hil/runners";

/// Build the runner of the checkout at `root` and keep a content-addressed
/// private copy below [`RUNNERS`]`/<sha256>/runner`; return the copy.
pub fn build(root: &Path) -> Result<PathBuf> {
    let compilation = compile(root)?;
    Ok(private_copy(root, &compilation.executable)?.0)
}

/// The content-addressed private copy of `executable` below [`RUNNERS`] and
/// its digest. A private copy prevents concurrent Cargo rebuilds from
/// replacing a run's inode.
fn private_copy(root: &Path, executable: &Path) -> Result<(PathBuf, String)> {
    let bytes = fs::read(executable)?;
    let hash = oer_tool_base_durable::sha256_bytes(&bytes);
    let directory = root.join(RUNNERS).join(&hash);
    fs::create_dir_all(&directory)?;
    let runner = directory.join("runner");
    if !runner.exists() {
        fs::write(&runner, &bytes)?;
        fs::set_permissions(&runner, fs::metadata(executable)?.permissions())?;
    }
    if fs::read(&runner)? != bytes {
        return Err("observer executable identity conflict".into());
    }
    Ok((runner, hash))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_runner_is_kept_by_its_digest_and_never_replaced() {
        let root = tempfile::tempdir().unwrap();
        let executable = root.path().join("built");
        fs::write(&executable, b"runner one").unwrap();
        let (runner, hash) = private_copy(root.path(), &executable).unwrap();
        assert_eq!(runner, root.path().join(RUNNERS).join(&hash).join("runner"));
        assert_eq!(fs::read(&runner).unwrap(), b"runner one");
        // The same build reuses its copy; a rebuild gets its own.
        assert_eq!(private_copy(root.path(), &executable).unwrap().0, runner);
        fs::write(&executable, b"runner two").unwrap();
        let (rebuilt, _) = private_copy(root.path(), &executable).unwrap();
        assert_ne!(rebuilt, runner);
        assert_eq!(fs::read(&runner).unwrap(), b"runner one");
        // A copy that no longer matches its digest is refused.
        fs::write(&runner, b"replaced").unwrap();
        fs::write(&executable, b"runner one").unwrap();
        assert!(private_copy(root.path(), &executable).is_err());
    }
}
