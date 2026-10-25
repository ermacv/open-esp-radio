//! Generates the board's `LAYOUT` and ROM linker script from chip facts.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (layout, files) = oer_espressif_facts::generate::tracked_layout("esp32s31")?;
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    for file in files {
        println!("cargo:rerun-if-changed={}", root.join(file).display());
    }
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").ok_or("cargo sets OUT_DIR")?);
    std::fs::write(out.join("layout.rs"), layout)?;
    let (facts, _) = oer_espressif_facts::generate::tracked("esp32s31")?;
    let rom = facts.rom.as_ref().ok_or("S31 has ROM facts")?;
    let script = out.join(&rom.linker_script);
    std::fs::create_dir_all(script.parent().ok_or("ROM script has a parent directory")?)?;
    std::fs::write(script, oer_espressif_facts::generate::rom_script(rom)?)?;

    // The shared linker binding takes one directory containing the chip's
    // ROM script and its register-model-generated DMA window.
    let dma = root.join("chips/esp32s31/layout/linker/dma-window.x");
    println!("cargo:rerun-if-changed={}", dma.display());
    std::fs::copy(dma, out.join("dma-window.x"))?;
    Ok(())
}
