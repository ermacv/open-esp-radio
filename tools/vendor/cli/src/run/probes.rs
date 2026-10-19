//! Build each chip's caller-owned vendor comparison artifacts.
//!
//! The probes are the packages of `firmware/probes/` (`<chip>-<name>`), built by
//! `cargo fw`'s probe build (`oer_image_build::probe`): the chip's
//! image compiler, with `cfg(oer_probe)`, in the probe target directory.

use crate::Result;
use oer_os_process::Checkout;
use std::{ffi::OsStr, num::NonZeroUsize};

const JOBS: &str = "OPEN_RADIO_ANALYSIS_BUILD_JOBS";

/// One probe image: its chip and the profile's `[[probe]]` entry.
struct Probe {
    /// The chip whose `firmware/probes/` (`<chip>-<name>`) holds the image.
    chip: String,
    probe: oer_chip_profile::Probe,
}

/// Every chip's probe images, from the chip profiles at `root`.
fn all(root: &std::path::Path) -> Result<Vec<Probe>> {
    Ok(oer_chip_profile::Profile::all(root)?
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

/// Where `probe`'s ELF is kept once built.
fn output(context: &Checkout, probe: &Probe) -> std::path::PathBuf {
    context
        .root
        .join("target/verification")
        .join(&probe.chip)
        .join(&probe.probe.package)
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
    build(
        context,
        chip,
        std::env::var_os(JOBS).as_deref(),
        |probe, jobs| {
            let package = probe.probe.package.as_str();
            let built =
                oer_image_build::probe::build(&context.root, package, jobs.map(NonZeroUsize::get))?;
            let elf = output(context, probe);
            std::fs::create_dir_all(elf.parent().ok_or("probe output has no parent")?)?;
            std::fs::copy(&built, &elf)?;
            let catalog = oer_probe_codegen::validate_elf(&std::fs::read(&elf)?, package)?;
            eprintln!(
                "Validated {} executable probe entries in {}",
                catalog.entries.len(),
                elf.display()
            );
            Ok(())
        },
    )?;
    eprintln!(
        "Rust analysis inputs are ready; the typed vendor scenarios compare them (cargo vendor-check scenario)."
    );
    Ok(())
}

/// Build every probe of `chip` in order through `execute`, with the job
/// override `jobs` validated before any build; stop at the first failure.
fn build(
    context: &Checkout,
    chip: &str,
    jobs: Option<&OsStr>,
    mut execute: impl FnMut(&Probe, Option<NonZeroUsize>) -> Result<()>,
) -> Result<()> {
    let jobs = parse_jobs(jobs)?;
    for probe in probes(&context.root, chip)? {
        eprintln!("Building {chip} {} comparison probe", probe.probe.role);
        super::phase::timed(&format!("build {chip} {} probe", probe.probe.role), || {
            execute(&probe, jobs)
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

#[cfg(test)]
mod tests;
