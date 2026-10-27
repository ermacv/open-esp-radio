fn main() -> Result<(), Box<dyn std::error::Error>> {
    oer_esp32s31_layout::build::configure_runtime("oer-firmware-hil-esp32s31-lower-mac")?;
    Ok(())
}
