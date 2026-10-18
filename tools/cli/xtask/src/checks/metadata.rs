//! The workspaces' lock files: `cargo xtask lock` and `lock --check`.

use crate::Result;
use oer_process::Checkout;
use std::{collections::BTreeSet, fs};

/// Every Cargo workspace of the repository, by its root manifest, as the
/// repository model (`oer-repo`) discovers them from the manifests as text.
pub fn workspaces(context: &Checkout) -> Result<BTreeSet<std::path::PathBuf>> {
    let workspaces: BTreeSet<_> = super::common::model(context)?
        .workspaces()
        .iter()
        .map(|manifest| context.root.join(manifest))
        .collect();
    if workspaces.is_empty() {
        return Err("no Cargo workspace found".into());
    }
    Ok(workspaces)
}

/// Brings every workspace lock in line with its manifests (`cargo xtask
/// lock`), so a dependency or pin change updates all locks in one step.
pub fn update_locks(context: &Checkout) -> Result<()> {
    for manifest in workspaces(context)? {
        let lock = manifest.with_file_name("Cargo.lock");
        let before = fs::read_to_string(&lock).ok();
        oer_process::capture(
            oer_toolchain::cargo_in(&context.root)
                .args(["metadata", "--format-version", "1"])
                // The one Cargo call of the repository that may go online.
                .args(["--config", "net.offline=false"])
                .arg("--manifest-path")
                .arg(&manifest),
        )?;
        let changed = before != fs::read_to_string(&lock).ok();
        println!(
            "{} {}",
            if changed { "updated  " } else { "unchanged" },
            lock.strip_prefix(&context.root)?.display()
        );
    }
    Ok(())
}

/// Fails, naming every one, when a workspace's lock no longer matches its
/// manifests (`cargo xtask lock --check`). Offline: a dependency missing
/// from the local cache is reported as such, and `cargo tidy fetch`
/// downloads it.
pub fn check_locks(context: &Checkout, manifests: &[std::path::PathBuf]) -> Result<()> {
    let mut stale = Vec::new();
    for manifest in manifests {
        let result = oer_process::capture(
            oer_toolchain::cargo_in(&context.root)
                .args(["metadata", "--format-version", "1", "--locked"])
                .arg("--manifest-path")
                .arg(manifest),
        );
        if let Err(error) = result {
            let reason = error
                .to_string()
                .lines()
                .rev()
                .find(|line| line.starts_with("error"))
                .unwrap_or("cargo metadata --locked failed")
                .to_owned();
            stale.push(format!(
                "{}: {reason}",
                manifest.strip_prefix(&context.root)?.display()
            ));
        }
    }
    if stale.is_empty() {
        return Ok(());
    }
    Err(format!(
        "{} workspace lock(s) do not match their manifests; `cargo xtask lock` updates them:\n{}",
        stale.len(),
        stale.join("\n")
    )
    .into())
}
