//! Linker-proven storage and addresses of its retained fields.

#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

use core::marker::PhantomData;
use oer_esp32s31_hal::{
    dma_static::Placed,
    layouts::{DmaLayout, DmaRef, DmaWindowAddress},
};

mod sealed {
    pub trait Sealed {}
}

/// Address space of placed controller storage. Implementations belong to this crate.
pub trait ControllerDmaSpace: sealed::Sealed {
    #[doc(hidden)]
    fn base<T>(storage: &T) -> u32;
}

/// Controller storage uses its own CPU address on the chip.
#[cfg(target_arch = "riscv32")]
pub enum ChipDma {}
#[cfg(target_arch = "riscv32")]
impl sealed::Sealed for ChipDma {}
#[cfg(target_arch = "riscv32")]
impl ControllerDmaSpace for ChipDma {
    fn base<T>(storage: &T) -> u32 {
        // ESP32-S31 has 32-bit pointers. Placed proves the complete allocation.
        core::ptr::from_ref(storage).addr() as u32
    }
}

/// A native model maps a placed arena to a distinct compile-time base.
#[cfg(not(target_arch = "riscv32"))]
pub enum HostDma<const BASE: u32> {}
#[cfg(not(target_arch = "riscv32"))]
impl<const BASE: u32> sealed::Sealed for HostDma<BASE> {}
#[cfg(not(target_arch = "riscv32"))]
impl<const BASE: u32> ControllerDmaSpace for HostDma<BASE> {
    fn base<T>(_storage: &T) -> u32 {
        const {
            let window = oer_esp32s31_hal::layouts::DMA_WINDOW;
            assert!(BASE > window.start && BASE < window.end);
            assert!(BASE.is_multiple_of(align_of::<u32>() as u32));
            assert!(size_of::<T>() <= (window.end - BASE) as usize);
        }
        BASE
    }
}

/// Geometry of one consumed Placed allocation; only its owner creates it.
///
/// It holds addresses, without a Rust borrow that could alias the retained
/// owner's mutable storage. Field checks are against this allocation.
#[doc(hidden)]
pub struct Placement<S> {
    root: usize,
    bytes: usize,
    base: u32,
    space: PhantomData<fn() -> S>,
}

impl<S: ControllerDmaSpace> Placement<S> {
    pub(crate) fn take<T>(placed: Placed<T>) -> (&'static mut T, Self) {
        let storage = placed.into_inner();
        let geometry = Self {
            root: core::ptr::from_ref(storage).addr(),
            bytes: size_of::<T>(),
            base: S::base(storage),
            space: PhantomData,
        };
        (storage, geometry)
    }

    pub(crate) fn address<T>(&self, field: &T) -> DmaWindowAddress {
        let offset = core::ptr::from_ref(field)
            .addr()
            .checked_sub(self.root)
            .expect("the field belongs to the placed allocation");
        self.at_offset(offset, size_of::<T>())
    }

    pub(crate) fn offset(
        &self,
        base: DmaWindowAddress,
        offset: usize,
        bytes: usize,
    ) -> DmaWindowAddress {
        let start = base
            .get()
            .checked_sub(self.base)
            .expect("the object belongs to this allocation") as usize;
        self.at_offset(
            start
                .checked_add(offset)
                .expect("a member offset fits the allocation"),
            bytes,
        )
    }

    #[allow(
        unsafe_code,
        reason = "the consumed Placed proves this allocation's DMA placement"
    )]
    fn at_offset(&self, offset: usize, bytes: usize) -> DmaWindowAddress {
        assert!(
            offset <= self.bytes && bytes <= self.bytes - offset,
            "the field lies inside its placed allocation"
        );
        let offset = u32::try_from(offset).expect("the placed allocation has 32-bit offsets");
        let address = self
            .base
            .checked_add(offset)
            .expect("a placed field has a 32-bit address");
        // SAFETY: Placed proves the entire static allocation; the subrange
        // above belongs to it. Its CPU owner retains that allocation through
        // every hardware epoch in which this address can be followed.
        unsafe { oer_esp32s31_hal::dma_address::placed(address) }
    }

    #[allow(
        unsafe_code,
        reason = "the referenced generated layout is a field of the placed allocation"
    )]
    pub(crate) fn reference<L: DmaLayout>(&self, layout: &L) -> DmaRef<L> {
        let address = self.address(layout);
        // SAFETY: layout is this exact initialized generated type within the
        // retained Placed allocation; its Rust field alignment is the layout's.
        unsafe { oer_esp32s31_hal::dma_address::placed_link(address.get()) }
            .expect("a placed layout has a nonzero address")
    }
}

#[cfg(test)]
mod tests;
