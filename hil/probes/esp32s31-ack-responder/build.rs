fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("riscv32") {
        oer_chips_esp32s31_platform_layout::build::configure_runtime(
            "oer-hil-probes-esp32s31-ack-responder",
        )?;
    }
    Ok(())
}
