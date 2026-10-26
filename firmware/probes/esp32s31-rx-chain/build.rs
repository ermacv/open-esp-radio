fn main() {
    if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("riscv32") {
        oer_esp32s31_layout::build::configure_runtime("oer-firmware-probes-esp32s31-rx-chain");
    }
}
