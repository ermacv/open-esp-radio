//! The Cargo commands of an image build: an environment that cannot change
//! the image unseen, and dependency fetching.

use std::{
    env,
    ffi::OsString,
    path::Path,
    process::{Command, Stdio},
};

use crate::Result;

/// The build environment's layout seed variable.
pub const LAYOUT_SEED_ENV: &str = "OER_LAYOUT_SEED";

/// A Cargo command for an image: inherited variables that would change the
/// image without appearing in the checkout are removed, and so is any
/// layout seed (a seed reaches a build only as its recorded seed).
pub fn command() -> Command {
    let mut command = oer_os_process::command(oer_image_toolchain::cargo_program());
    for variable in inherited_build_overrides(env::vars_os().map(|(name, _)| name)) {
        command.env_remove(variable);
    }
    command.env_remove(LAYOUT_SEED_ENV);
    command
}

/// Inherited Cargo variables that would change a firmware image without
/// appearing in the checkout: profiles, build and target settings. The build
/// relies on the repository's Cargo configuration instead. `RUSTFLAGS` stays
/// and is recorded in the build provenance; job count and target directory do
/// not change the image.
fn inherited_build_overrides(names: impl Iterator<Item = OsString>) -> Vec<OsString> {
    names
        .filter(|name| {
            let name = name.to_string_lossy();
            ["CARGO_PROFILE_", "CARGO_BUILD_", "CARGO_TARGET_"]
                .iter()
                .any(|prefix| name.starts_with(prefix))
                && name != "CARGO_BUILD_JOBS"
                && name != "CARGO_TARGET_DIR"
        })
        .collect()
}

/// Downloads what the workspace at `root` needs and the local Cargo cache
/// lacks, against its one `Cargo.lock`. Cargo runs
/// offline in this repository; an offline `cargo fetch` of a complete cache
/// takes a fraction of a second, and only a missing dependency goes online.
pub fn ensure_fetched(root: &Path) -> Result<()> {
    let manifest = root.join("Cargo.toml");
    let fetch = |online: bool| -> Result<bool> {
        let mut command = command();
        command
            .current_dir(root)
            .args(["fetch", "--locked", "--manifest-path"])
            .arg(&manifest);
        if online {
            command.args(["--config", "net.offline=false"]);
        } else {
            command.stdout(Stdio::null()).stderr(Stdio::null());
        }
        Ok(command.status()?.success())
    };
    if fetch(false)? || fetch(true)? {
        Ok(())
    } else {
        Err(format!("cannot fetch the dependencies of {}", manifest.display()).into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn firmware_builds_drop_inherited_cargo_overrides_that_change_the_image() {
        let names = [
            "CARGO_PROFILE_RELEASE_OPT_LEVEL",
            "CARGO_BUILD_RUSTFLAGS",
            "CARGO_TARGET_RISCV32IMAFC_UNKNOWN_NONE_ELF_RUSTFLAGS",
            "CARGO_BUILD_JOBS",
            "CARGO_TARGET_DIR",
            "RUSTFLAGS",
            "PATH",
        ]
        .map(OsString::from);
        assert_eq!(
            inherited_build_overrides(names.into_iter()),
            [
                "CARGO_PROFILE_RELEASE_OPT_LEVEL",
                "CARGO_BUILD_RUSTFLAGS",
                "CARGO_TARGET_RISCV32IMAFC_UNKNOWN_NONE_ELF_RUSTFLAGS",
            ]
            .map(OsString::from)
        );
    }

    #[test]
    fn an_inherited_layout_seed_never_reaches_a_build() {
        let command = command();
        assert!(
            command
                .get_envs()
                .any(|(name, value)| name == LAYOUT_SEED_ENV && value.is_none()),
            "the image's Cargo command must remove {LAYOUT_SEED_ENV}"
        );
    }
}
