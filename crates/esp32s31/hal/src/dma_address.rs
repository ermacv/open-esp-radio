//! The two sources of a typed DMA address (`docs/hardware-memory.md`).
//!
//! The PAC's [`DmaWindowAddress`] and [`DmaRef`] constructors are `unsafe`
//! and called only here, once per source:
//!
//! - **placed**: an address inside storage placed in the DMA window, a
//!   descriptor table or a buffer arena that stays pinned for the DMA epoch;
//! - **loaned**: a buffer lent to the hardware from outside, checked in the
//!   window where the loan is made, that stays reserved until the hardware
//!   is proven done with it.
//!
//! Until the linker proves placement, the storages derive placed addresses
//! from what `DmaSpace` reported for their own statics and checked in the
//! window when they were bound.

#![allow(
    clippy::disallowed_methods,
    reason = "the one module that makes typed DMA addresses"
)]

use crate::layouts::{DmaLayout, DmaRef, DmaWindowAddress};

/// An address inside storage placed in the DMA window.
///
/// # Safety
///
/// `address` lies in storage the caller holds bound for the DMA epoch, which
/// stays pinned while the hardware may read the address, and a window check
/// accepted it.
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

/// The address of a buffer lent to the hardware.
///
/// # Safety
///
/// `address` is the start of a buffer the caller retains for the hardware,
/// accepted by a window check when the buffer was lent, and released only
/// after the hardware is proven done with it.
#[allow(unsafe_code, reason = "the loaned source of typed DMA addresses")]
pub const unsafe fn loaned(address: u32) -> DmaWindowAddress {
    // SAFETY: the caller's promise is the constructor's.
    unsafe { DmaWindowAddress::new_unchecked(address) }
}
