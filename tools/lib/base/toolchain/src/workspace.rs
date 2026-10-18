//! The separate host-tool workspaces — Blobray and vendor verification —
//! and their build outputs, as every caller that builds or runs them from
//! another workspace reaches them: the vendor scenarios and probes, the
//! register inventory and the conformance check.

use std::{
    path::{Path, PathBuf},
    process::Command,
};

/// A host-tool workspace with its own lock file, target directory and
/// optimized incremental profile.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Workspace {
    /// Workspace directory, relative to the repository root.
    pub directory: &'static str,
    /// The optimized profile its aliases and runs build with.
    pub profile: &'static str,
}

/// Blobray: the binary-analysis engine and its `blobray` command line.
pub const BLOBRAY: Workspace = Workspace {
    directory: "tools/blobray",
    profile: "blobray",
};

/// Vendor verification: the scenario engine, the chips' scenarios, the
/// evidence producers and `cargo vendor`.
pub const VERIFICATION: Workspace = Workspace {
    directory: "verification",
    profile: "verification",
};

impl Workspace {
    /// Workspace manifest, relative to the repository root.
    pub fn manifest(self) -> String {
        format!("{}/Cargo.toml", self.directory)
    }

    /// The workspace keeps its own target directory; builds never share
    /// another workspace's Cargo lock or artifacts.
    pub fn target_directory(self, root: &Path) -> PathBuf {
        root.join(self.directory).join("target")
    }

    /// A Cargo command for `subcommand` in this workspace of `root` with
    /// its fixed target directory, independent of the caller's
    /// `CARGO_TARGET_DIR`.
    pub fn cargo(self, root: &Path, subcommand: &str) -> Command {
        let mut command = crate::cargo_in(root);
        command
            .env("CARGO_TARGET_DIR", self.target_directory(root))
            .arg(subcommand)
            .arg("--manifest-path")
            .arg(root.join(self.manifest()));
        command
    }

    /// The executable `name` built with the workspace's profile.
    pub fn binary(self, root: &Path, name: &str) -> PathBuf {
        self.target_directory(root).join(self.profile).join(name)
    }

    /// Build `package`'s binary `name` with the workspace's profile and
    /// return its path.
    pub fn build(self, root: &Path, package: &str, name: &str) -> crate::Result<PathBuf> {
        oer_process::run(self.cargo(root, "build").args([
            "--locked",
            "--profile",
            self.profile,
            "-p",
            package,
            "--bin",
            name,
        ]))?;
        Ok(self.binary(root, name))
    }
}

/// Build the `blobray` host with its profile and return its path.
pub fn blobray_host(root: &Path) -> crate::Result<PathBuf> {
    BLOBRAY.build(root, "blobray-cli", "blobray")
}
