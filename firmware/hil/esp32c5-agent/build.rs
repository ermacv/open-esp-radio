//! Link the stage-two runtime with the shared staged-boot scripts and this
//! board's layout.

fn main() {
    oer_firmware_platform_esp32c5_layout::build::configure_runtime(
        "oer-firmware-hil-esp32c5-agent",
    );
}
