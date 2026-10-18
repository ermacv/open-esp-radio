//! Download what every workspace's lock file names and the local Cargo
//! cache lacks (`cargo tidy fetch`).
//!
//! The repository keeps Cargo offline, so a pull that changed a lock file
//! needs this before a build. It lives here, not in `oer-xtask`, because
//! xtask itself may be what needs the new dependencies: this package
//! depends only on a few crates that rarely change.

use std::{path::Path, process::Stdio};

use oer_repo::{Model, Repo};

use crate::Result;

/// Fetches every workspace of `repo`: an offline `cargo fetch` first, which
/// costs a fraction of a second when everything is present, and an online
/// one only for a workspace that misses something. Returns the workspaces
/// that went online.
pub fn run(repo: &Repo, cargo: &Path) -> Result<Vec<String>> {
    let mut fetched = Vec::new();
    for manifest in Model::load(repo)?.workspaces() {
        let path = repo.root().join(manifest);
        let fetch = |online: bool| {
            let mut command = oer_process::command(cargo);
            command
                .current_dir(repo.root())
                .args(["fetch", "--locked", "--quiet", "--manifest-path"])
                .arg(&path);
            if online {
                command.args(["--config", "net.offline=false"]);
            } else {
                command.stdout(Stdio::null()).stderr(Stdio::null());
            }
            command
                .status()
                .map_err(|error| format!("cannot run cargo: {error}"))
        };
        if fetch(false)?.success() {
            continue;
        }
        println!("fetching the dependencies of {manifest}");
        if !fetch(true)?.success() {
            return Err(format!("cargo fetch failed for {manifest}"));
        }
        fetched.push(manifest.clone());
    }
    Ok(fetched)
}
