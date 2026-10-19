//! The host tools other tools build and run from the repository's single
//! workspace: Blobray, which the register inventory runs over the pinned
//! vendor binaries.

use std::path::{Path, PathBuf};

/// The optimized profile the host tools build with (the root `Cargo.toml`).
pub const PROFILE: &str = "tools";

/// The executable `name` built with [`PROFILE`] in `root`'s workspace.
pub fn binary(root: &Path, name: &str) -> PathBuf {
    root.join("target").join(PROFILE).join(name)
}

/// Build `package`'s binary `name` with [`PROFILE`] and return its path.
pub fn build(root: &Path, package: &str, name: &str) -> crate::Result<PathBuf> {
    oer_os_process::run(crate::cargo_in(root).args([
        "build",
        "--locked",
        "--profile",
        PROFILE,
        "-p",
        package,
        "--bin",
        name,
    ]))?;
    Ok(binary(root, name))
}

/// Build the `blobray` host and return its path.
pub fn blobray_host(root: &Path) -> crate::Result<PathBuf> {
    build(root, "oer-tool-blobray", "blobray")
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_host_binary_lies_in_the_tools_profile() {
        assert_eq!(
            super::binary(std::path::Path::new("/r"), "blobray"),
            std::path::Path::new("/r/target/tools/blobray")
        );
    }
}
