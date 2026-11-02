//! Generates the hart geometry of the chip the HAL's feature selects
//! (`geometry.rs`, the PAC's generator `hart_geometry` over the same facts)
//! and publishes the chip's id as this package's `chip` metadata. Without a
//! chip feature it generates nothing.

use std::path::{Path, PathBuf};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rustc-check-cfg=cfg(chip_geometry)");
    let chips: Vec<&str> = oer_espressif_facts::tracked::ids()
        .into_iter()
        .filter(|id| std::env::var_os(format!("CARGO_FEATURE_{}", id.to_uppercase())).is_some())
        .collect();
    let id = match chips.as_slice() {
        [] => return Ok(()),
        [id] => *id,
        _ => return Err(format!("a HAL selects exactly one chip, not {chips:?}").into()),
    };
    let (facts, files) = oer_espressif_facts::generate::tracked(id)?;
    let vectors = oer_espressif_facts::generate::Vectors::of(&facts)?;
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    for file in files {
        println!("cargo::rerun-if-changed={}", root.join(file).display());
    }
    let out = PathBuf::from(std::env::var_os("OUT_DIR").ok_or("cargo sets OUT_DIR")?);
    std::fs::write(
        out.join("geometry.rs"),
        oer_espressif_facts::generate::hart_geometry(vectors.harts, vectors.boot_hart)?,
    )?;
    println!("cargo::rustc-cfg=chip_geometry");
    println!("cargo::metadata=chip={id}");
    Ok(())
}
