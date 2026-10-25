//! Generates the board's `LAYOUT` from the chip facts
//! (`oer_espressif_facts::generate::tracked_layout`).

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (layout, files) = oer_espressif_facts::generate::tracked_layout("esp32s31")?;
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    for file in files {
        println!("cargo:rerun-if-changed={}", root.join(file).display());
    }
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").ok_or("cargo sets OUT_DIR")?);
    std::fs::write(out.join("layout.rs"), layout)?;
    Ok(())
}
