//! The Cargo commands of an image build: an environment that cannot change
//! the image unseen, local source overrides and dependency fetching.

use std::{
    env,
    ffi::OsString,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use crate::Result;

/// The build environment's layout seed variable.
pub const LAYOUT_SEED_ENV: &str = "OER_LAYOUT_SEED";

/// A Cargo command for an image: inherited variables that would change the
/// image without appearing in the checkout are removed, and so is any
/// layout seed (a seed reaches a build only as its recorded seed).
pub fn command() -> Command {
    let mut command = oer_process::command(oer_toolchain::cargo_program());
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

/// Local checkouts that replace pinned Git dependencies for one build.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Overrides {
    /// `ESP_HAL_ROOT`: esp-hal, esp-sync and the ESP-IDF bootloader crate.
    pub esp_hal: Option<PathBuf>,
}

const ESP_HAL_PACKAGES: [&str; 3] = ["esp-bootloader-esp-idf", "esp-hal", "esp-sync"];

impl Overrides {
    /// The overrides the caller's environment names, each checked for the
    /// package directories it must hold.
    pub fn from_environment() -> Result<Self> {
        let checked = |variable: &str, packages: &[&str]| -> Result<Option<PathBuf>> {
            let Some(local) = env::var_os(variable).map(PathBuf::from) else {
                return Ok(None);
            };
            let missing = packages
                .iter()
                .filter(|path| !local.join(path).is_dir())
                .copied()
                .collect::<Vec<_>>();
            if !missing.is_empty() {
                return Err(format!(
                    "{variable}={} is missing required package directories: {}",
                    local.display(),
                    missing.join(", ")
                )
                .into());
            }
            Ok(Some(local))
        };
        Ok(Self {
            esp_hal: checked("ESP_HAL_ROOT", &ESP_HAL_PACKAGES)?,
        })
    }

    /// Whether no dependency is overridden: the build then resolves exactly
    /// the committed pins (`--locked`).
    pub fn is_empty(&self) -> bool {
        self.esp_hal.is_none()
    }

    /// Patch the build `command` with every override.
    pub(crate) fn apply(&self, command: &mut Command) {
        if let Some(local) = &self.esp_hal {
            for package in ESP_HAL_PACKAGES {
                patch(
                    command,
                    "https://github.com/ermacv/esp-hal",
                    package,
                    &local.join(package),
                );
            }
        }
    }
}

fn patch(command: &mut Command, source: &str, package: &str, path: &Path) {
    command.arg("--config").arg(format!(
        "patch.\"{source}\".{package}.path=\"{}\"",
        path.display()
    ));
}

/// Downloads what the workspace of `manifest` at `root` needs and the local
/// Cargo cache lacks, with the lock file `configure` selects. Cargo runs
/// offline in this repository; an offline `cargo fetch` of a complete cache
/// takes a fraction of a second, and only a missing dependency goes online.
pub fn ensure_fetched(
    root: &Path,
    manifest: &Path,
    configure: impl Fn(&mut Command),
) -> Result<()> {
    let fetch = |online: bool| -> Result<bool> {
        let mut command = command();
        command
            .current_dir(root)
            .args(["fetch", "--locked", "--manifest-path"])
            .arg(manifest);
        if online {
            command.args(["--config", "net.offline=false"]);
        } else {
            command.stdout(Stdio::null()).stderr(Stdio::null());
        }
        configure(&mut command);
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

    #[test]
    fn the_esp_hal_override_patches_each_of_its_packages() {
        let overrides = Overrides {
            esp_hal: Some("/esp-hal".into()),
        };
        let mut command = oer_process::command("cargo");
        overrides.apply(&mut command);
        let arguments = command
            .get_args()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        for package in ESP_HAL_PACKAGES {
            assert!(
                arguments.iter().any(|argument| argument.contains(package)),
                "{package}"
            );
        }
        assert!(Overrides::default().is_empty());
        assert!(!overrides.is_empty());
    }
}
