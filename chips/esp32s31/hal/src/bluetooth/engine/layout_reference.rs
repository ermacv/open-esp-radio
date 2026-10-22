//! Conversion at the bound memory owner's publication boundary.

#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

use crate::layouts::{DmaLayout, DmaRef};

/// A reference to a layout inside the caller's retained controller arena.
///
/// # Safety
/// An initialized `L` occupies `address` in the caller's bound static storage,
/// aligned as `L` requires and retained for the complete hardware epoch. Host
/// models supply the corresponding synthetic address without dereferencing it.
#[allow(
    unsafe_code,
    reason = "the bound owner vouches for the layout and epoch"
)]
pub(crate) const unsafe fn controller_reference<L: DmaLayout>(address: u32) -> DmaRef<L> {
    // SAFETY: the caller retains this exact layout in its bound arena.
    unsafe { crate::dma_address::placed_link(address) }
        .expect("a bound controller layout has a nonzero CPU address")
}
