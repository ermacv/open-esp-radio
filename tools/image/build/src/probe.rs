//! The probe images of the vendor comparison: the packages of
//! `firmware/probes/`, built with `cfg(oer_probe)`.
//!
//! `cfg(oer_probe)` compiles the probe hooks of the HAL, PHY, PACs and radio
//! crates. Only this build sets it, only for a verification package, and in
//! its own target directory (`target/fw-probe`), so no production image or
//! host build ever compiles a hook and the production build cache never sees
//! the cfg.

use std::{
    cell::RefCell,
    ffi::OsString,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use crate::{Result, strict};

/// The cfg the probe hooks are compiled under.
pub const PROBE_CFG: &str = "oer_probe";
/// Every package of `firmware/probes/` (`<chip>-<name>`) is named after its path.
pub const PROBE_PACKAGE_PREFIX: &str = "oer-firmware-probes-";
/// The target directory of probe builds, below the checkout's `target/`.
pub const TARGET_SUBDIRECTORY: &str = "fw-probe";
const PROFILE: &str = crate::FIRMWARE_PROFILE;

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
    let chips: Vec<String> = oer_chip_profile::Profile::all(root)?
        .into_iter()
        .map(|profile| profile.id)
        .collect();
    let chip = chip_of(package, &chips)?;
    let profile = oer_chip_profile::Profile::load(root, chip)?;
    let policy = oer_chip_profile::policy::StackPolicy::load(&root.join(profile.stack_policy()))?;
    let target = profile.rust_target.as_str();
    let cache = target_dir(root);
    let _cache = crate::compiler::lock_compile_cache(&cache)?;
    let output = root.join("target/fw/probes").join(package);
    std::fs::create_dir_all(&output)?;
    // The image compiler of the chip, as every image build of it applies it.
    let mut cargo = crate::cargo::command();
    crate::compiler::configure(
        &mut cargo,
        &crate::compiler::ImageCompiler {
            root,
            linker_target: &crate::compiler::linker_target(root),
            rust_target: target,
            max_move_bytes: policy.max_move_bytes,
            runtime_environment: Vec::new(),
            // A probe compiles the probe hooks alone, never a class.
            instrumentation: crate::instrumentation::Instrumentation::default(),
        },
    )?;
    cargo.env(crate::compiler::ZEROED_INPUTS_ENV, profile.zeroed_inputs());
    let analysis = oer_image_checks::linker::strict::Spec {
        root: root.to_owned(),
        chip: chip.to_owned(),
        stack_policy: profile.stack_policy(),
    };
    let specification = strict::configure(&mut cargo, &analysis, &output, true)?;
    let mut cargo = command(cargo, root, package, target, jobs);
    cargo
        .arg("--message-format=json-render-diagnostics")
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    let images = RefCell::new(Vec::new());
    strict::build_with_retry(
        |invalidate| {
            if invalidate {
                let status = crate::cargo::command()
                    .current_dir(root)
                    .args([
                        "clean",
                        "--locked",
                        "--profile",
                        PROFILE,
                        "--target",
                        target,
                        "-p",
                        package,
                    ])
                    .env("CARGO_TARGET_DIR", &cache)
                    .status()?;
                if !status.success() {
                    return Err(
                        format!("invalidate probe link -p {package} failed ({status})").into(),
                    );
                }
            }
            let built = cargo
                .output()
                .map_err(|error| format!("run cargo: {error}"))?;
            if !built.status.success() {
                return Err(format!("cargo build -p {package} failed ({})", built.status).into());
            }
            *images.borrow_mut() = linked_images(&built.stdout)?;
            Ok(())
        },
        || {
            runtime_links_match(
                &images.borrow(),
                &output,
                &specification,
                &crate::compiler::linker_target(root).join("debug/oer-image-linker"),
            )
        },
    )?;
    Ok(image(root, package, target))
}

/// Cargo reports executables for both newly linked and Fresh binaries.
fn linked_images(messages: &[u8]) -> Result<Vec<PathBuf>> {
    let mut images = Vec::new();
    for line in messages
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        let message: serde_json::Value = serde_json::from_slice(line)?;
        if message.get("reason").and_then(serde_json::Value::as_str) == Some("compiler-artifact")
            && let Some(path) = message
                .get("executable")
                .and_then(serde_json::Value::as_str)
        {
            images.push(path.into());
        }
    }
    if images.is_empty() {
        return Err("cargo reported no linked probe images".into());
    }
    Ok(images)
}

fn runtime_links_match(
    images: &[PathBuf],
    output: &Path,
    specification: &Path,
    linker: &Path,
) -> Result<bool> {
    for image in images {
        let bytes = std::fs::read(image)?;
        let elf = oer_analysis_elf::Elf::parse(&bytes)?;
        // The runtime script defines this boundary. The package's other
        // binary uses a standalone script and writes no placement record.
        if elf.symbol("__runtime_image_start").is_some()
            && !strict::restore_if_matching(output, output, image, specification, linker)?
        {
            return Ok(false);
        }
    }
    Ok(true)
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

    #[test]
    fn both_fresh_and_rebuilt_executables_are_validated_in_a_mixed_probe_package() {
        use object::{
            Architecture, BinaryFormat, Endianness, SectionKind, SymbolFlags, SymbolKind,
            SymbolScope,
            write::{Object, Symbol, SymbolSection},
        };
        use oer_image_checks::linker::strict::{FRAGMENT, Fingerprint, PLACEMENT, Placement};

        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let ordinary = root.join("probe");
        let runtime = root.join("probe-dma");
        let make_elf = |symbol: &str, value: u8| {
            let mut object =
                Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
            let section = object.add_section(Vec::new(), b".text".to_vec(), SectionKind::Text);
            object.append_section_data(section, &[value; 4], 4);
            object.add_symbol(Symbol {
                name: symbol.as_bytes().to_vec(),
                value: 0,
                size: 4,
                kind: SymbolKind::Text,
                scope: SymbolScope::Linkage,
                weak: false,
                section: SymbolSection::Section(section),
                flags: SymbolFlags::None,
            });
            object.write().unwrap()
        };
        std::fs::write(&ordinary, make_elf("ordinary_entry", 0)).unwrap();
        std::fs::write(&runtime, make_elf("__runtime_image_start", 0)).unwrap();
        let specification = root.join("image-linker.toml");
        let linker = root.join("linker");
        for path in [&specification, &linker] {
            std::fs::write(path, b"analysis").unwrap();
        }
        // A standalone bin needs no record, but a reused runtime without
        // one must invalidate the package instead of passing unexamined.
        assert!(
            runtime_links_match(
                std::slice::from_ref(&ordinary),
                root,
                &specification,
                &linker
            )
            .unwrap()
        );
        assert!(
            !runtime_links_match(
                std::slice::from_ref(&runtime),
                root,
                &specification,
                &linker
            )
            .unwrap()
        );
        let placement = Placement {
            fingerprint: Some(Fingerprint {
                elf_sha256: oer_os_durable::sha256_file(&runtime).unwrap(),
                linker_sha256: oer_os_durable::sha256_file(&linker).unwrap(),
                inputs: [(
                    specification.clone(),
                    oer_os_durable::sha256_file(&specification).unwrap(),
                )]
                .into(),
            }),
            ..Placement::default()
        };
        std::fs::write(root.join(PLACEMENT), toml::to_string(&placement).unwrap()).unwrap();
        std::fs::write(root.join(FRAGMENT), placement.fragment()).unwrap();
        for fresh in [false, true] {
            let messages = [
                serde_json::json!({"reason":"compiler-artifact", "executable":null}),
                serde_json::json!({"reason":"compiler-artifact", "executable":ordinary, "fresh":fresh}),
                serde_json::json!({"reason":"compiler-artifact", "executable":runtime, "fresh":fresh}),
                serde_json::json!({"reason":"build-finished", "success":true}),
            ].map(|message| message.to_string()).join("\n");
            let images = linked_images(messages.as_bytes()).unwrap();
            assert_eq!(images, [ordinary.clone(), runtime.clone()]);
            assert!(runtime_links_match(&images, root, &specification, &linker).unwrap());
            std::fs::write(&runtime, make_elf("__runtime_image_start", 1)).unwrap();
            assert!(!runtime_links_match(&images, root, &specification, &linker).unwrap());
            std::fs::write(&runtime, make_elf("__runtime_image_start", 0)).unwrap();
        }
        assert!(linked_images(b"{\"reason\":\"build-finished\"}\n").is_err());
        assert!(linked_images(b"invalid JSON\n").is_err());
    }
}
