#![forbid(unsafe_code)]

//! Regenerate the checked-in S31 coverage inventory from facts and ABI reviews.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (facts, _) = oer_espressif_facts::generate::tracked("esp32s31")?;
    let vectors = oer_espressif_facts::generate::Vectors::of(&facts)?;
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../chips/espressif/staged-layout/stack-coverage.toml");
    std::fs::write(path, vectors.stack_coverage()?)?;
    Ok(())
}
