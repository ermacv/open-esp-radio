//! Build the HIL observer (the runner) with a receipt from Cargo's actual
//! artifacts.
use crate::compile::compile;
use oer_hil_run_bundle_format::observer::{artifacts, receipt};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Where a checkout keeps the private copies of the runners it built and
/// their receipts, relative to its root: one directory per executable
/// digest. (The observer *builds* runs refer to live in the run store's
/// [`oer_hil_run_bundle_format::observer::store::DIRECTORY`].)
pub const RUNNERS: &str = "target/hil/runners";

/// A prepared runner: its private copy and this invocation's receipt.
#[derive(Clone, Debug)]
pub struct Prepared {
    pub runner: PathBuf,
    pub receipt: PathBuf,
}

/// Build the runner of the checkout at `root`, keep a content-addressed
/// private copy below [`RUNNERS`]`/<sha256>/runner`, write this invocation's
/// receipt beside it and publish the checkout's [`receipt::CURRENT`].
pub fn prepare(root: &Path) -> Result<Prepared> {
    let compilation = compile(root)?;
    let artifacts = &compilation.artifacts;
    let (runner, hash) = private_copy(root, &compilation.executable)?;
    let output = oer_process::output(Command::new(&runner).arg("--observer-build"), None)?;
    if !output.status.success() {
        return Err("cannot read executable's embedded build".into());
    }
    let mut build: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    artifacts::apply(&mut build["resolved"], artifacts)?;
    build["resolved"]["selected_profile"] = serde_json::json!(compilation.profile);
    let document = serde_json::json!({"executable_sha256":hash,"build":build,"artifacts":artifacts,"profile":compilation.profile});
    // Each invocation owns its receipt, including concurrent launches of the same binary.
    let bytes = serde_json::to_vec(&document)?;
    let receipt_path = runner.with_file_name(format!(
        "receipt-{}.json",
        oer_durable::sha256_bytes(&bytes)
    ));
    if !receipt_path.exists() {
        oer_durable::atomic_write(&receipt_path, &bytes)?;
    }
    if fs::read(&receipt_path)? != bytes {
        return Err("observer receipt identity conflict".into());
    }
    oer_durable::atomic_write(&root.join(receipt::CURRENT), &bytes)?;
    drop(compilation);
    Ok(Prepared {
        runner,
        receipt: receipt_path,
    })
}

/// Build the runner of the checkout at `root` and keep its private copy as
/// [`prepare`] does, without reading its build or writing a receipt: for a
/// revision whose runner embeds its build in that revision's own format.
pub fn build(root: &Path) -> Result<PathBuf> {
    let compilation = compile(root)?;
    Ok(private_copy(root, &compilation.executable)?.0)
}

/// The content-addressed private copy of `executable` below [`RUNNERS`] and
/// its digest. A private copy prevents concurrent Cargo rebuilds from
/// replacing a run's inode.
fn private_copy(root: &Path, executable: &Path) -> Result<(PathBuf, String)> {
    let bytes = fs::read(executable)?;
    let hash = oer_durable::sha256_bytes(&bytes);
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
