//! The two sources of a typed DMA address (`docs/hardware-memory.md`).
//!
//! The PAC's [`DmaWindowAddress`] and [`DmaRef`] constructors are `unsafe`
//! and called only here: an address inside storage placed in the DMA window
//! ([`crate::dma_static`]), a descriptor table, a buffer arena or a placed
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

    /// The table of `len` `L`s whose first lies at `base`; `None` for an
    /// empty table, a zero base or one whose end passes the address width.
    ///
    /// # Safety
    ///
    /// `len` `L`s, aligned as `L` requires, lie back to back from `base` in
    /// storage as for [`placed`], retained for as long as the hardware may
    /// follow any of their addresses.
    #[allow(unsafe_code, reason = "the placed source of typed DMA tables")]
    pub const unsafe fn new(base: u32, len: usize) -> Option<Self> {
        if len == 0 || len > u32::MAX as usize || base == 0 {
            return None;
        }
        let len = len as u32;
        let Some(bytes) = len.checked_mul(Self::STRIDE) else {
            return None;
        };
        if base.checked_add(bytes).is_none() {
            return None;
        }
        // SAFETY: the caller's promise covers the first element.
        match unsafe { placed_link(base) } {
            Some(first) => Some(Self { first, len }),
            None => None,
        }
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
        unsafe { Self::new(base, len) }
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
            // SAFETY: `new`'s caller promised `len` placed, aligned `L`s
            // from the base, and `address` admits only their indices.
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
    /// The region of `len` bytes at `start`; `None` when its end passes
    /// the address width.
    ///
    /// # Safety
    ///
    /// The `len` bytes from `start` lie in storage as for [`placed`].
    #[allow(unsafe_code, reason = "the placed source of typed DMA regions")]
    pub const unsafe fn new(start: u32, len: u32) -> Option<Self> {
        if start.checked_add(len).is_none() {
            return None;
        }
        // SAFETY: the caller's promise is the constructor's.
        let start = unsafe { placed(start) };
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
        // SAFETY: `new`'s caller promised every byte of the region; the
        // sum fits, as `new` checked the end.
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
    /// `len` slots of `stride` bytes from `first`; `None` for no slot, a
    /// zero stride or an end past the address width.
    ///
    /// # Safety
    ///
    /// The `len * stride` bytes from `first` lie in storage as for
    /// [`placed`].
    #[allow(unsafe_code, reason = "the placed source of typed DMA slots")]
    pub const unsafe fn new(first: u32, stride: u32, len: usize) -> Option<Self> {
        if len == 0 || len > u32::MAX as usize || stride == 0 {
            return None;
        }
        let len = len as u32;
        let Some(bytes) = len.checked_mul(stride) else {
            return None;
        };
        if first.checked_add(bytes).is_none() {
            return None;
        }
        // SAFETY: the caller's promise is the constructor's.
        let first = unsafe { placed(first) };
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
            // SAFETY: `new`'s caller promised every slot's bytes, and
            // `address` admits only their indices.
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
