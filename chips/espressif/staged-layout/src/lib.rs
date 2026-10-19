#![no_std]
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

//! Staged-boot layout contract of the Espressif chips.
//!
//! A staged image boots in two stages: the ESP-IDF second-stage bootloader
//! loads a Flash-resident bootstrap, which initializes PSRAM, copies the
//! separately linked stage-two runtime there and enters it. The bootstrap,
//! the stage-two runtime, the shared linker scripts (`chips/espressif/staged-layout/linker`)
//! and the host image packer share these definitions; each chip's platform
//! (`platform/<chip>/layout`) states its address map as a [`Layout`].
//!
//! [`memory`] owns the address map type; [`interrupts`] the chip-neutral part
//! of the interrupt contract the interrupt-stack gate checks; [`stage_two`]
//! the image header and payload checksum that the bootstrap validates before
//! handoff; [`zeroed`] the input sections the boot zeroes. With the `build`
//! feature, [`build`] binds a chip's layout to the linker scripts.

pub mod handoff;
pub mod interrupts;
pub mod memory;
pub mod stage_two;
pub mod zeroed;

#[cfg(feature = "build")]
pub mod build;

pub use memory::{Layout, Region};
