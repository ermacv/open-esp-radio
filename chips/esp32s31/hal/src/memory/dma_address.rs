//! The two sources of a typed DMA address (`docs/hardware-memory.md`).
//!
//! The PAC's [`DmaWindowAddress`] and [`DmaRef`] constructors are `unsafe`
//! and called only here: an address inside storage placed in the DMA window
//! ([`crate::memory::dma_static`]), a descriptor table, a buffer arena or a placed
//! pool's buffer that stays pinned while the hardware may read it.
//!
//! A buffer lent to the hardware from outside the placed storage (a network
//! stack's or a PSRAM buffer) has no source yet: it returns as a loan type,
//! checked in the window where the loan is made, with the first such buffer.

#![allow(
    clippy::disallowed_methods,
    reason = "the one module that makes typed DMA addresses"
)]

use crate::layouts::{DmaLayout, DmaRef, DmaWindowAddress};

/// An address inside storage placed in the DMA window.
///
/// # Safety
///
/// `address` lies in storage proved by [`crate::memory::dma_static::Placed`] to be
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

/// A synthetic reference recorded by a host transaction model.
#[cfg(test)]
#[allow(
    unsafe_code,
    reason = "host model addresses are recorded, never dereferenced"
)]
pub(crate) fn model_reference<L: DmaLayout>(address: u32) -> DmaRef<L> {
    // SAFETY: host transaction models record this identity without accessing memory.
    unsafe { placed_link(address) }.expect("a model layout has a nonzero CPU address")
}

/// Refusal to project a field from a retained placed allocation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProjectionError {
    OutsideAllocation,
    AddressWidth,
    Misaligned,
    Empty,
    NonContiguous,
}

/// Address geometry of one consumed `Placed`, without an aliased Rust borrow.
/// Only the placement owner constructs this capability and retains the storage.
pub(crate) struct PlacementProjection {
    root: usize,
    bytes: usize,
    base: u32,
}

impl PlacementProjection {
    /// Consume the only placement authority; callers cannot supply an address.
    pub(crate) fn take<S: crate::ieee80211::dma::space::DmaSpace, T>(
        placed: crate::memory::dma_static::Placed<T>,
    ) -> Result<(&'static mut T, Self), (ProjectionError, crate::memory::dma_static::Placed<T>)> {
        let Some(base) = S::address(placed.as_ref(), core::ptr::from_ref(placed.as_ref())) else {
            return Err((ProjectionError::AddressWidth, placed));
        };
        let storage = placed.into_inner();
        let geometry = Self::from_storage(storage, base);
        Ok((storage, geometry))
    }

    fn from_storage<T>(storage: &T, base: u32) -> Self {
        Self {
            root: core::ptr::from_ref(storage).addr(),
            bytes: size_of::<T>(),
            base,
        }
    }

    pub(crate) fn try_with<S: crate::ieee80211::dma::space::DmaSpace, T, R>(
        placed: crate::memory::dma_static::Placed<T>,
        project: impl FnOnce(&T, &Self) -> Result<R, ProjectionError>,
    ) -> Result<(&'static mut T, R), (ProjectionError, crate::memory::dma_static::Placed<T>)> {
        let Some(base) = S::address(placed.as_ref(), core::ptr::from_ref(placed.as_ref())) else {
            return Err((ProjectionError::AddressWidth, placed));
        };
        let geometry = Self::from_storage(placed.as_ref(), base);
        match project(placed.as_ref(), &geometry) {
            Ok(value) => Ok((placed.into_inner(), value)),
            Err(error) => Err((error, placed)),
        }
    }

    #[cfg(feature = "bluetooth-engine")]
    pub(crate) fn take_controller<S: crate::bluetooth::engine::ControllerDmaSpace, T>(
        placed: crate::memory::dma_static::Placed<T>,
    ) -> (&'static mut T, Self) {
        let geometry = Self::controller_geometry::<S, T>(&placed);
        (placed.into_inner(), geometry)
    }

    #[cfg(feature = "bluetooth-engine")]
    pub(crate) fn try_with_controller<S: crate::bluetooth::engine::ControllerDmaSpace, T, R>(
        placed: crate::memory::dma_static::Placed<T>,
        project: impl FnOnce(&T, &Self) -> Result<R, ProjectionError>,
    ) -> Result<(&'static mut T, Self, R), (ProjectionError, crate::memory::dma_static::Placed<T>)> {
        let geometry = Self::controller_geometry::<S, T>(&placed);
        match project(placed.as_ref(), &geometry) {
            Ok(value) => Ok((placed.into_inner(), geometry, value)),
            Err(error) => Err((error, placed)),
        }
    }

    #[cfg(feature = "bluetooth-engine")]
    fn controller_geometry<S: crate::bluetooth::engine::ControllerDmaSpace, T>(
        placed: &crate::memory::dma_static::Placed<T>,
    ) -> Self {
        #[cfg(target_arch = "riscv32")]
        const {
            assert!(size_of::<usize>() == size_of::<u32>());
        }
        const {
            if let Some(base) = S::HOST_BASE {
                let window = crate::layouts::DMA_WINDOW;
                assert!(base > window.start && base < window.end);
                assert!(base.is_multiple_of(align_of::<T>() as u32));
                assert!(size_of::<T>() <= (window.end - base) as usize);
            }
        }
        let storage = placed.as_ref();
        let base = S::HOST_BASE.unwrap_or_else(|| core::ptr::from_ref(storage).addr() as u32);
        Self::from_storage(storage, base)
    }

    #[cfg(test)]
    pub(crate) fn for_test<T>(
        placed: crate::memory::dma_static::Placed<T>,
        base: u32,
    ) -> (&'static mut T, Self) {
        let storage = placed.into_inner();
        let geometry = Self::from_storage(storage, base);
        (storage, geometry)
    }

    fn checked(&self, cpu: usize, bytes: usize, alignment: usize) -> Result<u32, ProjectionError> {
        if bytes == 0 {
            return Err(ProjectionError::Empty);
        }
        let offset = cpu
            .checked_sub(self.root)
            .ok_or(ProjectionError::OutsideAllocation)?;
        if offset > self.bytes || bytes > self.bytes - offset {
            return Err(ProjectionError::OutsideAllocation);
        }
        let offset = u32::try_from(offset).map_err(|_| ProjectionError::AddressWidth)?;
        let bytes = u32::try_from(bytes).map_err(|_| ProjectionError::AddressWidth)?;
        let address = self
            .base
            .checked_add(offset)
            .ok_or(ProjectionError::AddressWidth)?;
        address
            .checked_add(bytes)
            .ok_or(ProjectionError::AddressWidth)?;
        if !cpu.is_multiple_of(alignment) || !(address as usize).is_multiple_of(alignment) {
            return Err(ProjectionError::Misaligned);
        }
        Ok(address)
    }

    pub(crate) fn address<T: ?Sized>(
        &self,
        field: &T,
    ) -> Result<DmaWindowAddress, ProjectionError> {
        self.checked_region(
            core::ptr::from_ref(field).addr(),
            size_of_val(field),
            align_of_val(field),
        )
        .map(|region| region.start())
    }

    #[allow(unsafe_code, reason = "the checked subrange of the consumed Placed")]
    fn checked_region(
        &self,
        cpu: usize,
        bytes: usize,
        alignment: usize,
    ) -> Result<PlacedRegion, ProjectionError> {
        let address = self.checked(cpu, bytes, alignment)?;
        // SAFETY: only consuming constructors over sealed address spaces mint
        // this geometry. The complete subrange lies in that placed allocation;
        // the owner retains or quarantines it for every hardware epoch.
        let start = unsafe { placed(address) };
        let len = u32::try_from(bytes).map_err(|_| ProjectionError::AddressWidth)?;
        PlacedRegion::new(start, len).ok_or(ProjectionError::AddressWidth)
    }

    pub(crate) fn pool_regions<const F: usize, const H: usize, const T: usize, const N: usize>(
        placed: crate::memory::dma_static::Placed<oer_memory::PinnedDmaTxPool<F, H, T, N>>,
    ) -> Result<
        (
            &'static mut oer_memory::PinnedDmaTxPool<F, H, T, N>,
            [PlacedRegion; N],
        ),
        (
            ProjectionError,
            crate::memory::dma_static::Placed<oer_memory::PinnedDmaTxPool<F, H, T, N>>,
        ),
    > {
        const {
            assert!(F + H + T > 0 && F + H + T <= u32::MAX as usize);
        }
        #[cfg(target_arch = "riscv32")]
        type Space = crate::ieee80211::dma::space::ChipDma;
        #[cfg(not(target_arch = "riscv32"))]
        type Space = crate::ieee80211::dma::space::HostDma;
        Self::try_with::<Space, _, _>(placed, |storage, projection| {
            let whole = projection.checked_region(
                projection.root,
                projection.bytes,
                align_of_val(storage),
            )?;
            let mut regions = [whole; N];
            for (region, (pointer, bytes)) in
                regions.iter_mut().zip(storage.dma_storage_locations())
            {
                *region = projection.checked_region(pointer.addr(), bytes, 1)?;
            }
            Ok(regions)
        })
    }

    #[cfg(feature = "bluetooth-engine")]
    pub(crate) fn offset(
        &self,
        base: DmaWindowAddress,
        offset: usize,
        bytes: usize,
    ) -> Result<DmaWindowAddress, ProjectionError> {
        let start = base
            .get()
            .checked_sub(self.base)
            .ok_or(ProjectionError::OutsideAllocation)? as usize;
        let offset = start
            .checked_add(offset)
            .ok_or(ProjectionError::OutsideAllocation)?;
        let cpu = self
            .root
            .checked_add(offset)
            .ok_or(ProjectionError::AddressWidth)?;
        let address = self.checked(cpu, bytes, 1)?;
        // Reuse the byte-region stepping proof; the checked full allocation
        // address is already typed by the caller's base.
        let delta = address
            .checked_sub(base.get())
            .ok_or(ProjectionError::OutsideAllocation)?;
        PlacedRegion::new(
            base,
            delta.checked_add(1).ok_or(ProjectionError::AddressWidth)?,
        )
        .and_then(|region| region.at(delta))
        .ok_or(ProjectionError::AddressWidth)
    }

    pub(crate) fn link<L: DmaLayout>(&self, field: &L) -> Result<DmaRef<L>, ProjectionError> {
        self.link_at::<L>(core::ptr::from_ref(field).addr())
    }

    #[allow(unsafe_code, reason = "a checked layout within the consumed Placed")]
    fn link_at<L: DmaLayout>(&self, cpu: usize) -> Result<DmaRef<L>, ProjectionError> {
        const {
            assert!(size_of::<L>() > 0 && size_of::<L>() <= u32::MAX as usize);
        }
        let address = self.checked(cpu, size_of::<L>(), align_of::<L>())?;
        // SAFETY: the caller supplies an initialized L (or a reviewed volatile
        // workspace overlay). The complete aligned extent is within Placed,
        // whose owner retains or quarantines it for the hardware's full use.
        unsafe { placed_link(address) }.ok_or(ProjectionError::AddressWidth)
    }

    pub(crate) fn table<'a, L: DmaLayout + 'a>(
        &self,
        fields: impl IntoIterator<Item = &'a L>,
    ) -> Result<PlacedTable<L>, ProjectionError> {
        let mut fields = fields.into_iter();
        let first = self.link(fields.next().ok_or(ProjectionError::Empty)?)?;
        let mut len = 1usize;
        for field in fields {
            let address = self.checked(
                core::ptr::from_ref(field).addr(),
                size_of::<L>(),
                align_of::<L>(),
            )?;
            let offset = len
                .checked_mul(size_of::<L>())
                .and_then(|n| u32::try_from(n).ok())
                .ok_or(ProjectionError::AddressWidth)?;
            if first.bits().checked_add(offset) != Some(address) {
                return Err(ProjectionError::NonContiguous);
            }
            len = len.checked_add(1).ok_or(ProjectionError::AddressWidth)?;
        }
        PlacedTable::new(first, len).ok_or(ProjectionError::AddressWidth)
    }

    pub(crate) fn slots<B>(&self, fields: &[B]) -> Result<PlacedSlots, ProjectionError> {
        const {
            assert!(size_of::<B>() <= u32::MAX as usize);
        }
        if size_of::<B>() == 0 {
            return Err(ProjectionError::Empty);
        }
        let first = self.address(fields)?;
        PlacedSlots::new(first, size_of::<B>() as u32, fields.len())
            .ok_or(ProjectionError::AddressWidth)
    }

    pub(crate) fn region(&self, bytes: &[u8]) -> Result<PlacedRegion, ProjectionError> {
        self.checked_region(bytes.as_ptr().addr(), bytes.len(), 1)
    }

    /// A reviewed generated volatile layout inside a workspace field.
    #[cfg(feature = "bluetooth-engine")]
    pub(crate) fn reference_at<L: DmaLayout, B, F>(
        &self,
        workspace: &B,
        field: &F,
    ) -> Result<DmaRef<L>, ProjectionError> {
        const {
            assert!(size_of::<L>() <= size_of::<B>());
            assert!(align_of::<B>() >= align_of::<L>());
        }
        let root = core::ptr::from_ref(workspace).addr();
        self.checked(root, size_of::<B>(), align_of::<B>())?;
        let cpu = core::ptr::from_ref(field).addr();
        let offset = cpu
            .checked_sub(root)
            .ok_or(ProjectionError::OutsideAllocation)?;
        if offset > size_of::<B>() || size_of::<L>() > size_of::<B>() - offset {
            return Err(ProjectionError::OutsideAllocation);
        }
        self.link_at(cpu)
    }
}

#[cfg(any(test, feature = "test-support"))]
pub(crate) fn model_region(len: usize) -> Result<PlacedRegion, ProjectionError> {
    let len = u32::try_from(len).map_err(|_| ProjectionError::AddressWidth)?;
    let base = crate::ieee80211::dma::space::HOST_ARENA_BASE;
    base.checked_add(len).ok_or(ProjectionError::AddressWidth)?;
    #[allow(
        unsafe_code,
        reason = "synthetic host identities are never dereferenced"
    )]
    // SAFETY: the model compares this address only; no DMA actor exists.
    let start = unsafe { placed(base) };
    PlacedRegion::new(start, len).ok_or(ProjectionError::AddressWidth)
}

/// The DMA addresses of `len` `L`s laid out back to back in placed storage:
/// a descriptor table typed once, where its storage was placed.
///
/// Its elements are reached as [`DmaRef`]s; no caller re-types a stored
/// integer. The integer views ([`Self::base`], [`Self::address`]) serve
/// comparisons with the walker's cursor registers and diagnostics.
pub struct PlacedTable<L: DmaLayout> {
    first: DmaRef<L>,
    len: u32,
}

impl<L: DmaLayout> Clone for PlacedTable<L> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<L: DmaLayout> Copy for PlacedTable<L> {}

impl<L: DmaLayout> PartialEq for PlacedTable<L> {
    fn eq(&self, other: &Self) -> bool {
        self.first == other.first && self.len == other.len
    }
}

impl<L: DmaLayout> Eq for PlacedTable<L> {}

impl<L: DmaLayout> core::fmt::Debug for PlacedTable<L> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("PlacedTable")
            .field("base", &self.first.bits())
            .field("len", &self.len)
            .finish()
    }
}

impl<L: DmaLayout> PlacedTable<L> {
    /// The bytes between two elements: the layout's size.
    const STRIDE: u32 = {
        let size = core::mem::size_of::<L>();
        assert!(
            size > 0 && size <= u32::MAX as usize,
            "a DMA layout has a size in the address width"
        );
        size as u32
    };

    /// Complete geometry of a table whose first element is already proved.
    const fn new(first: DmaRef<L>, len: usize) -> Option<Self> {
        if len == 0 || len > u32::MAX as usize {
            return None;
        }
        let len = len as u32;
        let Some(bytes) = len.checked_mul(Self::STRIDE) else {
            return None;
        };
        if first.bits().checked_add(bytes).is_none() {
            return None;
        }
        Some(Self { first, len })
    }

    /// A host model's table at the synthetic `base`, never dereferenced.
    #[cfg(any(test, feature = "test-support"))]
    #[allow(
        unsafe_code,
        reason = "host model addresses are recorded, never dereferenced"
    )]
    pub fn for_host_model(base: u32, len: usize) -> Option<Self> {
        // SAFETY: host models record these identities without accessing
        // memory through them, and have no DMA actor.
        Self::new(unsafe { placed_link(base) }?, len)
    }

    /// The first element.
    pub const fn first(self) -> DmaRef<L> {
        self.first
    }

    /// The first element's address, as the hardware reads it.
    pub const fn base(self) -> u32 {
        self.first.bits()
    }

    /// The number of elements.
    pub const fn len(self) -> usize {
        self.len as usize
    }

    /// Whether the table has no element; a table is never empty.
    pub const fn is_empty(self) -> bool {
        self.len == 0
    }

    /// The address of element `index`, as the hardware reads it.
    pub const fn address(self, index: usize) -> Option<u32> {
        if index >= self.len as usize {
            return None;
        }
        // In range: `new` checked that the table's end fits the width.
        Some(self.first.bits() + index as u32 * Self::STRIDE)
    }

    /// Element `index`, or `None` past the table.
    #[allow(unsafe_code, reason = "an element of a placed table")]
    pub const fn element(self, index: usize) -> Option<DmaRef<L>> {
        match self.address(index) {
            // SAFETY: the projection checked every initialized, aligned L
            // and their contiguity. `new` checked the extent's address width;
            // `address` admits only its indices. Host models never dereference.
            Some(address) => unsafe { placed_link(address) },
            None => None,
        }
    }

    /// The element at `address`, or `None` outside the table or between
    /// two elements.
    pub const fn index_of(self, address: u32) -> Option<usize> {
        let Some(offset) = address.checked_sub(self.first.bits()) else {
            return None;
        };
        if !offset.is_multiple_of(Self::STRIDE) {
            return None;
        }
        let index = (offset / Self::STRIDE) as usize;
        if index < self.len as usize {
            Some(index)
        } else {
            None
        }
    }
}

/// A placed byte region retained for the hardware: `len` bytes from
/// `start`, typed once where its placement is proved.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PlacedRegion {
    start: DmaWindowAddress,
    len: u32,
}

impl PlacedRegion {
    /// Geometry of bytes whose starting address is already proved.
    const fn new(start: DmaWindowAddress, len: u32) -> Option<Self> {
        if start.get().checked_add(len).is_none() {
            return None;
        }
        Some(Self { start, len })
    }

    /// The region's first byte.
    pub const fn start(self) -> DmaWindowAddress {
        self.start
    }

    /// The region's bytes.
    pub const fn len(self) -> u32 {
        self.len
    }

    /// Whether the region has no byte.
    pub const fn is_empty(self) -> bool {
        self.len == 0
    }

    /// The address `offset` bytes in, or `None` at or past the end.
    #[allow(unsafe_code, reason = "a byte of a placed region")]
    pub const fn at(self, offset: u32) -> Option<DmaWindowAddress> {
        if offset >= self.len {
            return None;
        }
        // SAFETY: the projection checked the complete placed byte extent;
        // the sum fits, as `new` checked the end.
        Some(unsafe { placed(self.start.get() + offset) })
    }
}

/// `len` placed slots of `stride` bytes each (buffers of one arena), typed
/// once where their placement is proved.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PlacedSlots {
    first: DmaWindowAddress,
    stride: u32,
    len: u32,
}

impl PlacedSlots {
    /// Geometry of slots whose starting address is already proved.
    const fn new(first: DmaWindowAddress, stride: u32, len: usize) -> Option<Self> {
        if len == 0 || len > u32::MAX as usize || stride == 0 {
            return None;
        }
        let len = len as u32;
        let Some(bytes) = len.checked_mul(stride) else {
            return None;
        };
        if first.get().checked_add(bytes).is_none() {
            return None;
        }
        Some(Self { first, stride, len })
    }

    /// The first slot's address, as the hardware reads it.
    pub const fn base(self) -> u32 {
        self.first.get()
    }

    /// The number of slots.
    pub const fn len(self) -> usize {
        self.len as usize
    }

    /// Whether there is no slot; a slot list is never empty.
    pub const fn is_empty(self) -> bool {
        self.len == 0
    }

    /// The address of slot `index`, as the hardware reads it.
    pub const fn address(self, index: usize) -> Option<u32> {
        if index >= self.len as usize {
            return None;
        }
        // In range: `new` checked that the end fits the width.
        Some(self.first.get() + index as u32 * self.stride)
    }

    /// Slot `index`, or `None` past the list.
    #[allow(unsafe_code, reason = "a slot of placed storage")]
    pub const fn slot(self, index: usize) -> Option<DmaWindowAddress> {
        match self.address(index) {
            // SAFETY: the projection checked the complete placed slice;
            // `new` checked its extent and `address` admits only its indices.
            Some(address) => Some(unsafe { placed(address) }),
            None => None,
        }
    }
}

/// One address per element of placed storage, each recorded from a typed
/// [`DmaWindowAddress`] and read back as one.
///
/// Its zero bytes record nothing, so storage that must be `Zeroable` (a
/// placed arena) holds it; the window never contains zero, so an entry is
/// either a recorded address or empty.
#[repr(transparent)]
#[derive(bytemuck::Zeroable)]
pub struct PlacedAddresses<const N: usize>([u32; N]);

const _: () = assert!(
    crate::layouts::DMA_WINDOW.start < crate::layouts::DMA_WINDOW.end
        && crate::layouts::DMA_WINDOW.start != 0,
    "zero lies outside the DMA window"
);

impl<const N: usize> PlacedAddresses<N> {
    /// No address recorded.
    pub const fn new() -> Self {
        Self([0; N])
    }

    /// Record `address` for element `index`; `false` past the table.
    pub fn record(&mut self, index: usize, address: DmaWindowAddress) -> bool {
        match self.0.get_mut(index) {
            Some(entry) => {
                *entry = address.get();
                true
            }
            None => false,
        }
    }

    /// The address recorded for element `index`, if one was.
    #[allow(unsafe_code, reason = "an address recorded from a typed one")]
    pub fn get(&self, index: usize) -> Option<DmaWindowAddress> {
        let address = *self.0.get(index)?;
        if address == 0 {
            return None;
        }
        // SAFETY: a nonzero entry was stored only by `record`, from a
        // `DmaWindowAddress` whose own construction made this promise; the
        // owner of these addresses retains their storage as it did then.
        Some(unsafe { placed(address) })
    }

    /// The recorded addresses as the hardware reads them, zero where none
    /// was recorded.
    pub const fn bits(&self) -> &[u32; N] {
        &self.0
    }

    /// The recorded addresses as integers, in element order.
    pub fn iter(&self) -> core::slice::Iter<'_, u32> {
        self.0.iter()
    }
}

impl<const N: usize> core::fmt::Debug for PlacedAddresses<N> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.debug_list().entries(self.0.iter()).finish()
    }
}

/// The recorded addresses compare as the hardware reads them.
impl<const N: usize> PartialEq<[u32; N]> for PlacedAddresses<N> {
    fn eq(&self, other: &[u32; N]) -> bool {
        self.0 == *other
    }
}

/// Element `index`'s address as the hardware reads it.
impl<const N: usize> core::ops::Index<usize> for PlacedAddresses<N> {
    type Output = u32;

    fn index(&self, index: usize) -> &u32 {
        &self.0[index]
    }
}

impl<'a, const N: usize> IntoIterator for &'a PlacedAddresses<N> {
    type Item = &'a u32;
    type IntoIter = core::slice::Iter<'a, u32>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl<const N: usize> Default for PlacedAddresses<N> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
