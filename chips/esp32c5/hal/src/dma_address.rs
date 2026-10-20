//! The placed source of a typed DMA address (`docs/hardware-memory.md`).
//!
//! The PAC's [`DmaWindowAddress`] and [`DmaRef`] constructors are `unsafe`
//! and called only here: an address inside storage placed in the DMA window
//! ([`crate::dma_static`]) that stays pinned while the hardware may read it.

#![allow(
    clippy::disallowed_methods,
    reason = "the one module that makes typed DMA addresses"
)]

use crate::layouts::{DmaLayout, DmaRef, DmaWindowAddress};

/// An address inside storage placed in the DMA window.
///
/// # Safety
///
/// `address` lies in storage proved by [`crate::dma_static::Placed`] to be
/// in the DMA window. The caller retains and pins that storage for the
/// complete epoch in which hardware may follow the address. Native models
/// use the corresponding synthetic address without dereferencing it.
#[allow(unsafe_code, reason = "the placed source of typed DMA addresses")]
pub const unsafe fn placed(address: u32) -> DmaWindowAddress {
    // SAFETY: the caller's promise is the constructor's.
    unsafe { DmaWindowAddress::new_unchecked(address) }
}

/// A link to the `L` at `address` inside placed storage, or no link for zero.
///
/// # Safety
///
/// A zero `address`, or the address of an `L`, aligned as `L` requires, in
/// storage as for [`placed`].
#[allow(unsafe_code, reason = "the placed source of typed DMA addresses")]
pub const unsafe fn placed_link<L: DmaLayout>(address: u32) -> Option<DmaRef<L>> {
    if address == 0 {
        return None;
    }
    // SAFETY: the caller's promise is the constructors'.
    Some(unsafe { DmaRef::new_unchecked(DmaWindowAddress::new_unchecked(address)) })
}
