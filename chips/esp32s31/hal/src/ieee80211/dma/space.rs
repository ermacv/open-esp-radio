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
#[cfg(any(not(target_arch = "riscv32"), test, feature = "test-support"))]
pub const HOST_ARENA_BASE: u32 = crate::layouts::DMA_WINDOW.start + 0x1000;

mod sealed {
    pub trait Sealed {}
}

impl sealed::Sealed for ChipDma {}
#[cfg(any(not(target_arch = "riscv32"), test, feature = "test-support"))]
impl sealed::Sealed for HostDma {}

/// How an arena's fields are addressed by DMA. Sealed so a safe caller cannot
/// substitute an unrelated allocation's DMA address.
///
/// ```compile_fail,E0277
/// use oer_esp32s31_hal::ieee80211::dma::space::DmaSpace;
/// enum Relocated {}
/// impl DmaSpace for Relocated {
///     fn address<A: ?Sized, F: ?Sized>(_: &A, _: *const F) -> Option<u32> {
///         Some(0x2f00_1000)
///     }
/// }
/// ```
pub trait DmaSpace: sealed::Sealed + 'static {
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

/// The CPU identity space, selected internally by the 802.15.4 constructor.
/// Chip pointers have the DMA address width; native models record low CPU
/// identities and never dereference their DMA representation.
pub(crate) enum NativeDma {}
impl sealed::Sealed for NativeDma {}
impl DmaSpace for NativeDma {
    fn address<A: ?Sized, F: ?Sized>(_: &A, field: *const F) -> Option<u32> {
        #[cfg(target_arch = "riscv32")]
        const {
            assert!(size_of::<usize>() == size_of::<u32>());
        }
        Some(field.addr() as u32)
    }
}

/// A host model's address space: [`HOST_ARENA_BASE`] plus the field's
/// offset in its arena. The arena's alignment carries over to its fields'
/// synthetic addresses.
#[cfg(any(not(target_arch = "riscv32"), test, feature = "test-support"))]
pub enum HostDma {}

#[cfg(any(not(target_arch = "riscv32"), test, feature = "test-support"))]
impl DmaSpace for HostDma {
    fn address<Arena: ?Sized, Field: ?Sized>(arena: &Arena, field: *const Field) -> Option<u32> {
        let arena: *const Arena = arena;
        let offset = field.addr().checked_sub(arena.addr())?;
        HOST_ARENA_BASE.checked_add(u32::try_from(offset).ok()?)
    }
}

#[cfg(test)]
mod tests;
