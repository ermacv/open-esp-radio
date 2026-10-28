fn main() {
    if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() != Ok("riscv32") {
        return;
    }
    println!("cargo:rerun-if-env-changed=PSRAM_RUNTIME_BIN");
    assert!(
        std::env::var_os("PSRAM_RUNTIME_BIN").is_some(),
        "PSRAM_RUNTIME_BIN must name the packed stage-two runtime"
    );
    let (facts, files) = oer_espressif_facts::generate::tracked("esp32s31").expect("S31 facts");
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    for file in files {
        println!("cargo:rerun-if-changed={}", root.join(file).display());
    }
    let flash = facts.flash.expect("the module has flash");
    let bytes = flash
        .application_encoding
        .size_mib
        .checked_mul(1024 * 1024)
        .expect("flash capacity fits u32");
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("Cargo sets OUT_DIR"));
    std::fs::write(out.join("flash-capacity.rs"), format!("{bytes}u32\n"))
        .expect("write flash capacity from module facts");
    oer_esp32s31_layout::build::configure_bootstrap("oer-firmware-bootstrap-esp32s31");
}
