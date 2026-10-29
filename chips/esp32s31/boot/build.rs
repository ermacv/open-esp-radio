//! Bind the chip/profile LP-fast choice without embedding policy in HAL.
use std::path::{Path, PathBuf};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=build.rs");
    let (facts, files) = oer_espressif_facts::generate::tracked("esp32s31")?;
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    for file in files {
        println!("cargo:rerun-if-changed={}", root.join(file).display());
    }
    let out = PathBuf::from(std::env::var_os("OUT_DIR").ok_or("cargo sets OUT_DIR")?);
    std::fs::write(
        out.join("lp-fast-source.rs"),
        oer_espressif_facts::generate::lp_fast_source(&facts)?,
    )?;
    Ok(())
}
