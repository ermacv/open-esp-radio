fn main() {
    // Cargo can reuse this compiled build script after the package is moved.
    // Resolve the current invocation's manifest directory, not its build path.
    let directory = std::env::var_os("CARGO_MANIFEST_DIR")
        .expect("Cargo supplies the package manifest directory");
    let directory = std::path::PathBuf::from(directory);
    oer_probe_codegen::build(
        &directory.join("../library/src/lib.rs"),
        &std::env::var("CARGO_PKG_NAME").expect("Cargo package name"),
        &std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("Cargo output directory")),
    )
    .expect("valid probe declarations");
    let script = directory.join("link.x");
    println!("cargo:rerun-if-changed={}", script.display());
    println!("cargo:rustc-link-arg=-T{}", script.display());
}
