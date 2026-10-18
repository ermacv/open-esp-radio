//! Build each chip's caller-owned vendor comparison artifacts.
//!
//! The probes build into the host's shared firmware compile cache with
//! exactly the image compiler of their chip (`oer_tool_base_toolchain::image`: the
//! image linker, the stack-size section, the move limit of the chip's stack
//! policy, shared generics, and the zeroed inputs the image linker checks),
//! so the dependency units they share with the chip's images (esp-hal, the
//! PACs, production crates) are compiled once for both.

use crate::Result;
use oer_tool_base_process as process;
use oer_tool_base_process::Checkout;
use std::{ffi::OsStr, num::NonZeroUsize, process::Command};

const JOBS: &str = "OPEN_RADIO_ANALYSIS_BUILD_JOBS";

/// One probe image: its chip and the profile's `[[probe]]` entry.
struct Probe {
    /// The chip whose `verification/<chip>/probes` workspace owns the image.
    chip: String,
    probe: oer_tool_data_chip_profile::Probe,
}

/// Every chip's probe images, from the chip profiles at `root`.
fn all(root: &std::path::Path) -> Result<Vec<Probe>> {
    Ok(oer_tool_data_chip_profile::Profile::all(root)?
        .into_iter()
        .flat_map(|profile| {
            let chip = profile.id.clone();
            profile.probe.into_iter().map(move |probe| Probe {
                chip: chip.clone(),
                probe,
            })
        })
        .collect())
}

/// The probe images of `chip`, in build order.
fn probes(root: &std::path::Path, chip: &str) -> Result<Vec<Probe>> {
    Ok(all(root)?
        .into_iter()
        .filter(|probe| probe.chip == chip)
        .collect())
}

/// The built comparison ELF of the probe `package`: copied out of the
/// shared compile cache into the repository's `target/verification/<chip>/`,
/// one file per probe package.
pub fn elf(context: &Checkout, package: &str) -> Result<std::path::PathBuf> {
    let probe = all(&context.root)?
        .into_iter()
        .find(|p| p.probe.package == package)
        .ok_or_else(|| format!("no vendor probe package {package}"))?;
    Ok(output(context, &probe))
}

/// Where `probe`'s ELF is kept once built.
fn output(context: &Checkout, probe: &Probe) -> std::path::PathBuf {
    context
        .root
        .join("target/verification")
        .join(&probe.chip)
        .join(&probe.probe.package)
}

/// The host's one firmware compile cache, which the probes share with the
/// images.
fn compile_cache() -> Result<std::path::PathBuf> {
    Ok(oer_tool_base_toolchain::image::compile_cache(
        &oer_tool_base_toolchain::image::host_build_root()?,
    ))
}

pub fn run(context: &Checkout, chip: &str, list_roles: bool) -> Result<()> {
    let declared = probes(&context.root, chip)?;
    if declared.is_empty() {
        return Err(format!("unsupported vendor-probe chip: {chip}").into());
    }
    if list_roles {
        // Declaration only: no build is executed or inferred by this listing.
        for probe in &declared {
            println!("{}", probe.probe.role);
        }
        return Ok(());
    }
    let cache = compile_cache()?;
    let profile = oer_tool_data_chip_profile::Profile::load(&context.root, chip)?;
    let target = profile.rust_target.clone();
    let policy =
        oer_tool_data_image_policy::StackPolicy::load(&context.root.join(profile.stack_policy()))?;
    let linker = oer_tool_base_toolchain::image::linker_target(
        &oer_tool_base_toolchain::image::host_build_root()?,
    );
    build(
        context,
        chip,
        &cache,
        std::env::var_os(JOBS).as_deref(),
        |command| {
            let arguments: Vec<_> = command.get_args().collect();
            let package = arguments
                .windows(2)
                .find(|pair| pair[0] == "--package")
                .and_then(|pair| pair[1].to_str())
                .ok_or("missing probe package")?
                .to_owned();
            let probe = declared
                .iter()
                .find(|probe| probe.probe.package == package)
                .ok_or("unknown probe package")?;
            let elf = output(context, probe);
            // The image compiler of the chip, as every image build of it
            // applies it.
            command.env(
                oer_tool_base_toolchain::image::ZEROED_INPUTS_ENV,
                profile.zeroed_inputs(),
            );
            oer_tool_base_toolchain::image::configure(
                command,
                &policy.image_compiler(&context.root, &linker, &target),
            )?;
            {
                // Every build uplifts into the same `<target>/release`: hold
                // the cache until this probe's ELF is copied out.
                let _cache = oer_tool_base_toolchain::image::lock_compile_cache(&cache)?;
                process::run(command)?;
                std::fs::create_dir_all(elf.parent().ok_or("probe output has no parent")?)?;
                let built = cache.join(&target).join("release").join(&package);
                std::fs::copy(&built, &elf)?;
                // A shard's sources start from the image's dep-info beside it.
                let mut dep_info = elf.clone().into_os_string();
                dep_info.push(".d");
                std::fs::copy(built.with_extension("d"), dep_info)?;
            }
            let catalog =
                oer_tool_service_probe_codegen::validate_elf(&std::fs::read(&elf)?, &package)?;
            eprintln!(
                "Validated {} executable probe entries in {}",
                catalog.entries.len(),
                elf.display()
            );
            Ok(())
        },
    )?;
    eprintln!(
        "Rust analysis inputs are ready; the typed vendor scenarios compare them (cargo verification scenario)."
    );
    Ok(())
}

fn build(
    context: &Checkout,
    chip: &str,
    cache: &std::path::Path,
    jobs: Option<&OsStr>,
    mut execute: impl FnMut(&mut Command) -> Result<()>,
) -> Result<()> {
    // Validate before any invocation; stop at the first failed artifact.
    let jobs = parse_jobs(jobs)?;
    let target = oer_tool_data_chip_profile::rust_target(&context.root, chip)?;
    for probe in probes(&context.root, chip)? {
        eprintln!("Building {chip} {} comparison probe", probe.probe.role);
        super::phase::timed(&format!("build {chip} {} probe", probe.probe.role), || {
            execute(&mut command(context, &probe, &target, cache, jobs))
        })?;
    }
    Ok(())
}

fn parse_jobs(value: Option<&OsStr>) -> Result<Option<NonZeroUsize>> {
    let Some(value) = value else { return Ok(None) };
    let value = value
        .to_str()
        .ok_or_else(|| format!("{JOBS} must be a positive integer"))?;
    if value.is_empty()
        || value.starts_with('0')
        || !value.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(format!("{JOBS} must be a positive integer").into());
    }
    Ok(Some(value.parse::<NonZeroUsize>().map_err(|_| {
        format!("{JOBS} must be a positive integer within the host size range")
    })?))
}

fn command(
    context: &Checkout,
    probe: &Probe,
    target: &str,
    cache: &std::path::Path,
    jobs: Option<NonZeroUsize>,
) -> Command {
    let mut command = oer_tool_base_toolchain::cargo_in(&context.root);
    command
        .args(["build", "--manifest-path"])
        .arg(
            context
                .root
                .join("verification")
                .join(&probe.chip)
                .join("probes/Cargo.toml"),
        )
        .args([
            "--package",
            probe.probe.package.as_str(),
            "--target",
            target,
            "--release",
            "--locked",
        ])
        .env("CARGO_TARGET_DIR", cache);
    if let Some(jobs) = jobs {
        command.arg("--jobs").arg(jobs.to_string());
    }
    command
}

#[cfg(test)]
mod tests;
