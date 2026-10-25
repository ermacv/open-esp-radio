#![no_std]
#![forbid(unsafe_code)]

//! Staged-boot address map of ESP32-S31-Function-CoreBoard-1.
//!
//! The chip-neutral contract (the `Layout` type, the stage-two header, the
//! zeroed sections and the shared linker scripts) is
//! `oer-espressif-staged-layout`. This crate's build generates the board's
//! [`LAYOUT`] from the chip facts (`oer-espressif-facts`: its layers from the
//! ISA to the board, `chips/esp32s31/platform/`) and the chip's stack policy,
//! for the bootstrap, the board, and the build scripts of every image; each
//! value's `SOURCE:` is in those files. With the `build` feature, [`build`]
//! links a binary with the shared scripts and this chip's ROM script
//! (generated in this crate's build directory).

include!(concat!(env!("OUT_DIR"), "/layout.rs"));

#[cfg(feature = "build")]
pub mod build {
    //! Link a binary of this board with the shared linker scripts.
    extern crate std;

    use std::path::Path;

    fn linker_dir() -> std::path::PathBuf {
        Path::new(env!("OUT_DIR")).to_path_buf()
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

#[cfg(test)]
mod tests {
    extern crate std;

    #[test]
    fn the_linker_inputs_are_generated_from_the_tracked_facts() {
        let (facts, _) = oer_espressif_facts::generate::tracked("esp32s31").expect("chip facts");
        let rom = facts.rom.as_ref().expect("ROM facts");
        let out = std::path::Path::new(env!("OUT_DIR"));
        let script = std::fs::read_to_string(out.join(super::LAYOUT.rom_script))
            .expect("generated ROM script");
        assert_eq!(
            script,
            oer_espressif_facts::generate::rom_script(rom).expect("ROM generation")
        );
        assert_eq!(
            std::fs::read(out.join("dma-window.x")).expect("staged DMA fragment"),
            include_bytes!("../linker/dma-window.x")
        );
    }
}
