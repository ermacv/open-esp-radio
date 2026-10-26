//! Linker-proven storage and addresses of its retained fields.

#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

use crate::{
    dma_static::Placed,
    layouts::{DmaLayout, DmaRef, DmaWindowAddress},
};
use core::marker::PhantomData;

mod sealed {
    pub trait Sealed {
        const HOST_BASE: Option<u32>;
    }
}

/// Address space of placed controller storage. Implementations belong to this crate.
pub trait ControllerDmaSpace: sealed::Sealed {}

/// Controller storage uses its own CPU address on the chip.
#[cfg(target_arch = "riscv32")]
pub enum ChipDma {}
#[cfg(target_arch = "riscv32")]
impl sealed::Sealed for ChipDma {
    const HOST_BASE: Option<u32> = None;
}
#[cfg(target_arch = "riscv32")]
impl ControllerDmaSpace for ChipDma {}

/// A native model maps a placed arena to a distinct compile-time base.
#[cfg(not(target_arch = "riscv32"))]
pub enum HostDma<const BASE: u32> {}
#[cfg(not(target_arch = "riscv32"))]
impl<const BASE: u32> sealed::Sealed for HostDma<BASE> {
    const HOST_BASE: Option<u32> = Some(BASE);
}
#[cfg(not(target_arch = "riscv32"))]
impl<const BASE: u32> ControllerDmaSpace for HostDma<BASE> {}

/// Geometry of one consumed Placed allocation; only its owner creates it.
///
/// It holds addresses, without a Rust borrow that could alias the retained
/// owner's mutable storage. Field checks are against this allocation.
#[doc(hidden)]
pub(crate) struct Placement<S> {
    projection: crate::dma_address::PlacementProjection,
    space: PhantomData<fn() -> S>,
}

impl<S: ControllerDmaSpace> Placement<S> {
    pub(crate) fn take<T>(placed: Placed<T>) -> (&'static mut T, Self) {
        let (storage, projection) =
            crate::dma_address::PlacementProjection::take_controller::<S, _>(placed);
        let geometry = Self {
            projection,
            space: PhantomData,
        };
        (storage, geometry)
    }

    pub(super) fn from_projection(projection: crate::dma_address::PlacementProjection) -> Self {
        Self {
            projection,
            space: PhantomData,
        }
    }

    pub(super) fn projection(&self) -> &crate::dma_address::PlacementProjection {
        &self.projection
    }

    pub(crate) fn address<T>(&self, field: &T) -> DmaWindowAddress {
        self.projection
            .address(field)
            .expect("the field belongs to the placed allocation")
    }

    pub(crate) fn offset(
        &self,
        base: DmaWindowAddress,
        offset: usize,
        bytes: usize,
    ) -> DmaWindowAddress {
        self.projection
            .offset(base, offset, bytes)
            .expect("the field lies inside its placed allocation")
    }

    pub(crate) fn reference<L: DmaLayout>(&self, layout: &L) -> DmaRef<L> {
        self.projection
            .link(layout)
            .expect("a placed layout has a nonzero address")
    }

    pub(crate) fn reference_at<L: DmaLayout, B, F>(
        &self,
        workspace: &B,
        field: &F,
    ) -> Result<DmaRef<L>, crate::dma_address::ProjectionError> {
        self.projection.reference_at::<L, B, F>(workspace, field)
    }
}

#[cfg(test)]
mod tests;
