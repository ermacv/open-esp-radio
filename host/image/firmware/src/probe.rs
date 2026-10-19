//! The probe images of the vendor comparison: the packages of
//! `firmware/probes/`, built with `cfg(oer_probe)`.
//!
//! `cfg(oer_probe)` compiles the probe hooks of the HAL, PHY, PACs and radio
//! crates. Only this build sets it, only for a verification package, and in
//! its own target directory (`target/fw-probe`), so no production image or
//! host build ever compiles a hook and the production build cache never sees
//! the cfg.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
};

use crate::Result;

/// The cfg the probe hooks are compiled under.
pub const PROBE_CFG: &str = "oer_probe";
/// Every package of `firmware/probes/` (`<chip>-<name>`) is named after its path.
pub const PROBE_PACKAGE_PREFIX: &str = "oer-firmware-probes-";
/// The target directory of probe builds, below the checkout's `target/`.
pub const TARGET_SUBDIRECTORY: &str = "fw-probe";
const PROFILE: &str = "release";

/// The chip of the probe package `package`
/// (`oer-firmware-probes-<chip>-<name>`), one of `chips`.
///
/// # Errors
///
/// `package` is not a verification package of one of `chips`.
pub fn chip_of<'a>(package: &str, chips: &'a [String]) -> Result<&'a str> {
    let rest = package.strip_prefix(PROBE_PACKAGE_PREFIX).ok_or_else(|| {
        format!(
            "{package} is not a firmware/probes package, which alone builds with cfg({PROBE_CFG})"
        )
    })?;
    chips
        .iter()
        .find(|chip| {
            rest.strip_prefix(chip.as_str())
                .is_some_and(|name| name.starts_with('-'))
        })
        .map(String::as_str)
        .ok_or_else(|| format!("{package} names no supported chip").into())
}

/// The flags of a probe build: the caller's, then the probe cfg.
fn rustflags(existing: Option<OsString>) -> OsString {
    let mut flags = existing.unwrap_or_default();
    if !flags.is_empty() {
        flags.push(" ");
    }
    flags.push(format!("--cfg {PROBE_CFG}"));
    flags
}

/// Make `command`, which carries the chip's image compiler setup, build the
/// probe `package` for `rust_target` in the checkout `root`: the probe cfg
/// after the image compiler's flags, in the probe target directory.
pub fn command(
    mut command: Command,
    root: &Path,
    package: &str,
    rust_target: &str,
    jobs: Option<usize>,
) -> Command {
    let existing = command
        .get_envs()
        .find(|(name, _)| *name == "RUSTFLAGS")
        .map(|(_, value)| value.map(OsString::from))
        .unwrap_or_else(|| std::env::var_os("RUSTFLAGS"));
    command
        .current_dir(root)
        .args([
            "build",
            "--profile",
            PROFILE,
            "--locked",
            "--target",
            rust_target,
            "-p",
            package,
        ])
        .env("CARGO_TARGET_DIR", target_dir(root))
        .env("RUSTFLAGS", rustflags(existing))
        .env_remove("CARGO_ENCODED_RUSTFLAGS");
    if let Some(jobs) = jobs {
        command.arg("--jobs").arg(jobs.to_string());
    }
    command
}

/// The target directory of probe builds in the checkout `root`.
pub fn target_dir(root: &Path) -> PathBuf {
    root.join("target").join(TARGET_SUBDIRECTORY)
}

/// The linked image of the probe `package` a build for `rust_target` left.
pub fn image(root: &Path, package: &str, rust_target: &str) -> PathBuf {
    target_dir(root)
        .join(rust_target)
        .join(PROFILE)
        .join(package)
}

/// Build the probe `package` with the probe cfg and return its image.
///
/// # Errors
///
/// `package` is not a verification package of a supported chip, or the
/// build failed.
pub fn build(root: &Path, package: &str, jobs: Option<usize>) -> Result<PathBuf> {
    let chips: Vec<String> = oer_image_profile::Profile::all(root)?
        .into_iter()
        .map(|profile| profile.id)
        .collect();
    let chip = chip_of(package, &chips)?;
    let profile = oer_image_profile::Profile::load(root, chip)?;
    let policy = crate::gates::policy(root, &profile)?;
    let target = profile.rust_target.as_str();
    // The image compiler of the chip, as every image build of it applies it.
    let mut cargo = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    oer_image_toolchain::image::configure(
        &mut cargo,
        &oer_image_toolchain::image::ImageCompiler {
            root,
            linker_target: &root.join("target/image-linker"),
            rust_target: target,
            max_move_bytes: policy.max_move_bytes,
            runtime_environment: Vec::new(),
        },
    )?;
    cargo.env(
        oer_image_toolchain::image::ZEROED_INPUTS_ENV,
        profile.zeroed_inputs(),
    );
    let status = command(cargo, root, package, target, jobs)
        .status()
        .map_err(|error| format!("run cargo: {error}"))?;
    if !status.success() {
        return Err(format!("cargo build -p {package} failed ({status})").into());
    }
    Ok(image(root, package, target))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chips() -> Vec<String> {
        vec!["esp32s31".into(), "esp32c5".into()]
    }

    #[test]
    fn only_a_verification_package_of_a_known_chip_is_a_probe() {
        let chips = chips();
        assert_eq!(
            chip_of("oer-firmware-probes-esp32s31-radio", &chips).unwrap(),
            "esp32s31"
        );
        assert_eq!(
            chip_of("oer-firmware-probes-esp32c5-radio", &chips).unwrap(),
            "esp32c5"
        );
        assert!(chip_of("oer-firmware-apps-w1", &chips).is_err());
        assert!(chip_of("oer-firmware-probes-esp32s3-probe", &chips).is_err());
        assert!(chip_of("oer-firmware-probes-esp32s31", &chips).is_err());
    }

    #[test]
    fn a_probe_build_adds_the_cfg_in_its_own_target_directory() {
        let root = Path::new("/checkout");
        let mut compiler = Command::new("cargo");
        compiler.env("RUSTFLAGS", "-C linker=image-linker");
        let command = command(
            compiler,
            root,
            "oer-firmware-probes-esp32s31-radio",
            "rv",
            None,
        );
        let env: Vec<_> = command.get_envs().collect();
        assert!(env.contains(&(
            std::ffi::OsStr::new("CARGO_TARGET_DIR"),
            Some(Path::new("/checkout/target/fw-probe").as_os_str())
        )));
        let flags = env
            .iter()
            .find(|(name, _)| *name == "RUSTFLAGS")
            .and_then(|(_, value)| *value)
            .unwrap();
        // The image compiler's flags stay; the probe cfg follows them.
        assert_eq!(flags, "-C linker=image-linker --cfg oer_probe");
        assert_eq!(
            rustflags(Some("-C x".into())),
            OsString::from("-C x --cfg oer_probe")
        );
    }
}
