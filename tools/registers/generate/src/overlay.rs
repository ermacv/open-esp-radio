//! `cargo registers overlay`: import a chip's strictness overlay from the
//! pinned ESP-IDF register headers, and check every entry against them
//! (see [`oer_registers_model::check_overlay`]).

use std::{
    fs,
    path::{Path, PathBuf},
};

use oer_registers_model::{HeaderTree, StrictnessOverlay};
use serde::Deserialize;

use crate::Result;

/// A checkout of ESP-IDF on disk.
struct IdfTree(PathBuf);

impl HeaderTree for IdfTree {
    fn read(&self, path: &str) -> oer_registers_model::Result<String> {
        fs::read_to_string(self.0.join(path))
            .map_err(|error| oer_registers_model::Error::Invalid(format!("{path}: {error}")))
    }

    fn list(&self, directory: &str) -> oer_registers_model::Result<Vec<String>> {
        let entries = fs::read_dir(self.0.join(directory)).map_err(|error| {
            oer_registers_model::Error::Invalid(format!("{directory}: {error}"))
        })?;
        let mut names: Vec<String> = entries
            .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
            .collect();
        names.sort();
        Ok(names)
    }
}

#[derive(Deserialize)]
struct Manifest {
    soc: Option<SocPaths>,
}

#[derive(Deserialize)]
struct SocPaths {
    svd: PathBuf,
    overlay: PathBuf,
}

struct Inputs {
    overlay_path: PathBuf,
    overlay_text: String,
    overlay: StrictnessOverlay,
    svd: String,
}

fn load(root: &Path, chip: &str, idf: &Path) -> Result<Inputs> {
    let manifest_path = root
        .join("chips")
        .join(chip)
        .join("registers/publication/registers.toml");
    let manifest: Manifest = toml::from_str(&fs::read_to_string(&manifest_path)?)?;
    let soc = manifest
        .soc
        .ok_or_else(|| format!("{} has no [soc]", manifest_path.display()))?;
    let base = manifest_path.parent().ok_or("manifest has no directory")?;
    let overlay_path = base.join(soc.overlay).canonicalize()?;
    let overlay_text = fs::read_to_string(&overlay_path)?;
    let overlay = StrictnessOverlay::parse(&overlay_text)?;
    let svd = fs::read_to_string(base.join(soc.svd))?;
    let head = oer_os_process::git::text(idf, ["rev-parse", "HEAD"])?;
    if head != overlay.esp_idf {
        return Err(format!(
            "ESP-IDF at {} is {head}, the overlay pins {}",
            idf.display(),
            overlay.esp_idf
        )
        .into());
    }
    Ok(Inputs {
        overlay_path,
        overlay_text,
        overlay,
        svd,
    })
}

/// Verify every overlay entry of `chip` against the headers of the ESP-IDF
/// checkout `idf` and the vendor SVD; an error lists every mismatch.
pub fn check(root: &Path, chip: &str, idf: &Path) -> Result<()> {
    let inputs = load(root, chip, idf)?;
    let verdict =
        oer_registers_model::check_overlay(&inputs.overlay, &inputs.svd, &IdfTree(idf.to_owned()))?;
    println!(
        "overlay of {chip}: {} entries checked, {} claims not in the headers",
        verdict.checked,
        verdict.unchecked.len()
    );
    for line in &verdict.unchecked {
        println!("  unchecked: {line}");
    }
    if verdict.failures.is_empty() {
        return Ok(());
    }
    for line in &verdict.failures {
        eprintln!("  FAIL: {line}");
    }
    Err(format!(
        "{} overlay entries of {chip} do not match their source",
        verdict.failures.len()
    )
    .into())
}

/// Pin the headers of `peripherals` (every peripheral the overlay pins, when
/// empty) in the overlay file of `chip` and print what the headers and the
/// vendor SVD disagree on, as correction entries; `write` also appends the
/// corrections to the overlay file.
pub fn import(
    root: &Path,
    chip: &str,
    idf: &Path,
    peripherals: &[String],
    write: bool,
) -> Result<()> {
    let inputs = load(root, chip, idf)?;
    let import = oer_registers_model::import_overlay(
        &inputs.overlay_text,
        &inputs.overlay,
        chip,
        &inputs.svd,
        peripherals,
        &IdfTree(idf.to_owned()),
    )?;
    let text = if write {
        &import.text_with_corrections
    } else {
        &import.text
    };
    if *text != inputs.overlay_text {
        fs::write(&inputs.overlay_path, text)?;
    }
    println!(
        "overlay of {chip}: {} headers pinned, {} corrections {}, {} field mismatches",
        import.pins.len(),
        import.corrections.len(),
        if write { "written" } else { "proposed" },
        import.mismatches.len()
    );
    if !write {
        for correction in &import.corrections {
            println!("{correction}");
        }
    }
    for line in &import.mismatches {
        println!("  mismatch: {line}");
    }
    Ok(())
}
