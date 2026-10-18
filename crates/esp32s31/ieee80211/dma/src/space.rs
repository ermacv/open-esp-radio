//! The DMA address space a storage arena publishes to the Wi-Fi MAC.
//!
//! A descriptor or buffer is published by its 32-bit DMA address. On the
//! chip that is the field's own address ([`ChipDma`]). A host model has
//! wider pointers and no DMA window, so [`HostDma`] (test support only)
//! gives each field a synthetic address: [`HOST_ARENA_BASE`] plus the
//! field's offset in its arena. Every arena check runs the same code in both
//! spaces; only this conversion differs.

#![forbid(unsafe_code)]

/// Where a host model's arena starts in the DMA window. Its low 20 bits are
/// not zero: the walker's NEXT register reads a zero low address as an
/// exhausted list.
#[cfg(any(test, feature = "test-support"))]
pub const HOST_ARENA_BASE: u32 = oer_esp32s31_hal::layouts::DMA_WINDOW.start + 0x1000;

/// How an arena's fields are addressed by DMA.
pub trait DmaSpace: 'static {
    /// The DMA address of `field`, a field of the arena at `arena`, or
    /// `None` when it has none in this space.
    fn address<Arena: ?Sized, Field: ?Sized>(arena: &Arena, field: *const Field) -> Option<u32>;
}

/// The chip's own address space: a field's address is its DMA address.
pub enum ChipDma {}

impl DmaSpace for ChipDma {
    fn address<Arena: ?Sized, Field: ?Sized>(_arena: &Arena, field: *const Field) -> Option<u32> {
        u32::try_from(field.addr()).ok()
    }
}

/// A host model's address space: [`HOST_ARENA_BASE`] plus the field's
/// offset in its arena. The arena's alignment carries over to its fields'
/// synthetic addresses.
#[cfg(any(test, feature = "test-support"))]
pub enum HostDma {}

#[cfg(any(test, feature = "test-support"))]
impl DmaSpace for HostDma {
    fn address<Arena: ?Sized, Field: ?Sized>(arena: &Arena, field: *const Field) -> Option<u32> {
        let arena: *const Arena = arena;
        let offset = field.addr().checked_sub(arena.addr())?;
        HOST_ARENA_BASE.checked_add(u32::try_from(offset).ok()?)
    }
}

#[cfg(test)]
mod tests;
