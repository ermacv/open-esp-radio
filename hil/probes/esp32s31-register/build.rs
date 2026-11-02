fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() != Ok("riscv32") {
        return Ok(());
    }
    // Cargo can reuse this compiled build script after the package is moved.
    // Resolve the current invocation's manifest directory, not its build path.
    let directory = std::env::var_os("CARGO_MANIFEST_DIR")
        .ok_or("Cargo supplies the package manifest directory")?;
    let directory = std::path::PathBuf::from(directory);
    oer_probe_codegen::build(
        &directory.join("src/lib.rs"),
        &std::env::var("CARGO_PKG_NAME")?,
        &std::path::PathBuf::from(std::env::var_os("OUT_DIR").ok_or("Cargo output directory")?),
    )
    .map_err(|error| -> Box<dyn std::error::Error> { error })?;
    let script = directory.join("link.x");
    println!("cargo:rerun-if-changed={}", script.display());
    // The script places the image alone; the library is linked into it.
    println!("cargo:rustc-link-arg-bins=-T{}", script.display());
    Ok(())
}
