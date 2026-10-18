fn main() {
    println!("cargo:rerun-if-env-changed=PSRAM_RUNTIME_BIN");
    assert!(
        std::env::var_os("PSRAM_RUNTIME_BIN").is_some(),
        "PSRAM_RUNTIME_BIN must name the packed stage-two runtime"
    );
    oer_esp32c5_platform_layout::build::configure_bootstrap("oer-esp32c5-platform-bootstrap");
}
