//! The image linker: rustc runs it in place of `rust-lld` for every
//! image (`-C linker`, set by
//! `oer_image_build::compiler::configure`, which builds it from the tree being
//! built).

#![forbid(unsafe_code)]

fn main() {
    std::process::exit(oer_image_checks::linker::run(
        std::env::args_os().skip(1).collect(),
        &oer_image_checks::linker::zeroed_inputs(),
    ));
}
