#![no_std]
#![forbid(unsafe_code)]

//! Staged-boot address map of the ESP32-C5 devkit with the N16R8 module
//! (16-MiB flash, 8-MiB quad PSRAM).
//!
//! The chip-neutral contract (the `Layout` type, the stage-two header, the
//! zeroed sections and the shared linker scripts) is
//! `oer-espressif-staged-layout`. This crate's build generates the board's
//! [`LAYOUT`] from the chip facts (`oer-espressif-facts`: its layers from the
//! ISA to the board, `chips/esp32c5/platform/`) and the chip's stack policy,
//! for the bootstrap, the board, and the build scripts of every image; each
//! value's `SOURCE:` is in those files. With the `build` feature, [`build`]
//! links a binary with the shared scripts and this chip's ROM script
//! (`chips/esp32c5/layout/linker`).

include!(concat!(env!("OUT_DIR"), "/layout.rs"));

#[cfg(feature = "build")]
pub mod build {
    //! Link a binary of this board with the shared linker scripts.
    extern crate std;

    use std::path::Path;

    fn linker_dir() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("linker")
    }

    /// Link binary `bin` as a stage-two runtime of this board.
    pub fn configure_runtime(bin: &str) {
        oer_espressif_staged_layout::build::configure_runtime(bin, &super::LAYOUT, &linker_dir());
    }

    /// Link binary `bin` as this board's Flash-resident bootstrap.
    pub fn configure_bootstrap(bin: &str) {
        oer_espressif_staged_layout::build::configure_bootstrap(bin, &super::LAYOUT, &linker_dir());
    }
}
