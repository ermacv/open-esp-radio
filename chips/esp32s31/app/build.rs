//! The interrupt table: the source's rows with the chip facts' layout.
use std::path::{Path, PathBuf};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/interrupt_table.in");
    let (facts, files) = oer_chips_family_facts::generate::tracked("esp32s31")?;
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    for file in files {
        println!("cargo:rerun-if-changed={}", root.join(file).display());
    }
    let out = PathBuf::from(std::env::var_os("OUT_DIR").ok_or("cargo sets OUT_DIR")?);
    let template = std::fs::read_to_string("src/interrupt_table.in")?;
    std::fs::write(
        out.join("interrupt_table.rs"),
        oer_chips_family_facts::generate::interrupt_table(&facts, &template)?,
    )?;
    Ok(())
}
