//! Link the stage-two runtime with the shared staged-boot scripts and this
//! board's layout.

fn main() {
    oer_esp32c5_platform_layout::build::configure_runtime("oer-esp32c5-hil-agent");
}
