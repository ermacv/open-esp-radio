#![cfg_attr(
    feature = "build",
    expect(
        clippy::cast_possible_truncation,
        reason = "step 16b: the boot's and the family drivers' rows"
    )
)]
#![no_std]
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]
#![cfg_attr(
    test,
    allow(
        clippy::disallowed_methods,
        clippy::disallowed_macros,
        clippy::unreachable,
        clippy::arithmetic_side_effects
    )
)]
#![expect(
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    reason = "step 16b: the boot's and the family drivers' rows"
)]

//! Staged-boot layout contract of the Espressif chips.
//!
//! A staged image boots in two stages: the ESP-IDF second-stage bootloader
//! loads a Flash-resident bootstrap, which initializes PSRAM, copies the
//! separately linked stage-two runtime there and enters it. The bootstrap,
//! the stage-two runtime, the shared linker scripts (`chips/family/platform/layout/linker`)
//! and the host image packer share these definitions; each chip's platform
//! (`chips/<chip>/platform/layout`) states its address map as a [`Layout`].
//!
//! [`boot`] names the one hart that boots; [`memory`] owns the address map
//! type; [`stage_two`] the image header and payload checksum that the
//! bootstrap validates before handoff; [`zeroed`] the input sections the boot
//! zeroes. With the `build` feature, [`build`] binds a chip's layout to the
//! linker scripts.

pub mod boot;
pub mod handoff;
pub mod memory;
pub mod stage_two;
pub mod zeroed;

#[cfg(feature = "build")]
pub mod build;

pub use memory::{Layout, LayoutFault, Region};
