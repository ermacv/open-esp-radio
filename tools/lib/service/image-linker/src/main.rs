//! The image linker: rustc runs it in place of `rust-lld` for every
//! image (`-C linker`, set by
//! `oer_tool_base_toolchain::image::configure`, which builds it from the tree being
//! built).

fn main() {
    std::process::exit(oer_tool_service_image_linker::run(
        std::env::args_os().skip(1).collect(),
        &oer_tool_service_image_linker::zeroed_inputs(),
    ));
}
