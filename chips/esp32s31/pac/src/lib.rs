#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]
//! The ESP32-S31 PAC: the exact hardware model generated from the chip's
//! register model (`chips/esp32s31/registers/`) by `cargo registers
//! generate`, and the root that takes every claimed part once.
//!
//! The generated module holds the typed register blocks with their field
//! accessors by class, value domains, typed DMA pointers, the interrupt
//! sources, the instances' bases, the ownership parts and partitions, the
//! memory layouts and the analog-I2C dictionary. It has no logic: every
//! sequence, poll and policy over it is the HAL's.

#![no_std]
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

#[cfg(test)]
extern crate std;

mod generated;

pub use generated::*;

use core::sync::atomic::{AtomicBool, Ordering};

/// Whether the root's one claim was taken.
static TAKEN: AtomicBool = AtomicBool::new(false);

/// Every partition of the claimed register blocks, the first time it is
/// called; `None` after.
///
/// The PAC is the only code that constructs these blocks: no other crate
/// of an image reaches the claimed words.
pub fn take() -> Option<ownership::Partitions> {
    if TAKEN.swap(true, Ordering::AcqRel) {
        return None;
    }
    #[allow(
        unsafe_code,
        clippy::disallowed_methods,
        reason = "the PAC root takes its one claim"
    )]
    // SAFETY: `TAKEN` lets this run once per boot, so no other claim of
    // these blocks exists; the generated `disjoint_blocks` check proves no
    // two blocks of the claim list one word, and no other code of an image
    // accesses a claimed word (the PAC is the only constructor of a block).
    let claims = unsafe { oer_mmio::assume_owned::<ownership::Claims>() };
    Some(ownership::partition(claims))
}
