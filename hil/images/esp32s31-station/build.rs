fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("riscv32") {
        oer_esp32s31_layout::build::configure_runtime(
            "oer-hil-scenarios-images-esp32s31-sdk-station",
        )?;
    }
    Ok(())
}
