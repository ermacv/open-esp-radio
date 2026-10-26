//! Typed addresses of storage retained for DMA.

use core::marker::PhantomData;

/// The RAM region a chip's DMA engines reach, as the generated `layouts`
/// module of its PAC names it.
pub trait DmaWindow {
    /// The region's addresses.
    const RANGE: core::ops::Range<u32>;
}

/// An address inside the DMA window `W` whose memory stays reserved for
/// the hardware while it may read the address, as the hardware reads it.
pub struct DmaWindowAddress<W: DmaWindow> {
    address: u32,
    window: PhantomData<fn() -> W>,
}

impl<W: DmaWindow> DmaWindowAddress<W> {
    /// The address `address`.
    ///
    /// # Safety
    ///
    /// `address` lies inside `W::RANGE`, in a static placed there or a
    /// buffer lent to the hardware, which stays reserved for as long as the
    /// hardware may read the address.
    #[allow(unsafe_code, reason = "the caller promises a placed address")]
    pub const unsafe fn new_unchecked(address: u32) -> Self {
        Self {
            address,
            window: PhantomData,
        }
    }

    /// The address as the hardware reads it.
    pub const fn get(self) -> u32 {
        self.address
    }
}

impl<W: DmaWindow> Clone for DmaWindowAddress<W> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<W: DmaWindow> Copy for DmaWindowAddress<W> {}

impl<W: DmaWindow> PartialEq for DmaWindowAddress<W> {
    fn eq(&self, other: &Self) -> bool {
        self.address == other.address
    }
}

impl<W: DmaWindow> Eq for DmaWindowAddress<W> {}

impl<W: DmaWindow> core::hash::Hash for DmaWindowAddress<W> {
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        self.address.hash(state);
    }
}

impl<W: DmaWindow> core::fmt::Debug for DmaWindowAddress<W> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_tuple("DmaWindowAddress")
            .field(&self.address)
            .finish()
    }
}

/// A memory layout placed only in its DMA window, which a [`DmaRef`] may
/// point at.
pub trait DmaLayout {
    /// The window the layout lies in.
    type Window: DmaWindow;
}

/// The address of an `L` inside its DMA window.
pub struct DmaRef<L: DmaLayout> {
    address: DmaWindowAddress<L::Window>,
    layout: PhantomData<fn() -> L>,
}

impl<L: DmaLayout> DmaRef<L> {
    /// The reference to the `L` at `address`.
    ///
    /// # Safety
    ///
    /// An `L`, aligned as `L` requires, lies at `address` for as long as the
    /// hardware may follow the reference.
    #[allow(unsafe_code, reason = "the caller promises a placed layout")]
    pub const unsafe fn new_unchecked(address: DmaWindowAddress<L::Window>) -> Self {
        Self {
            address,
            layout: PhantomData,
        }
    }

    /// The referenced address.
    pub const fn address(self) -> DmaWindowAddress<L::Window> {
        self.address
    }

    /// The address as the hardware reads it.
    pub const fn bits(self) -> u32 {
        self.address.address
    }
}

impl<L: DmaLayout> Clone for DmaRef<L> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<L: DmaLayout> Copy for DmaRef<L> {}

impl<L: DmaLayout> PartialEq for DmaRef<L> {
    fn eq(&self, other: &Self) -> bool {
        self.address == other.address
    }
}

impl<L: DmaLayout> Eq for DmaRef<L> {}

impl<L: DmaLayout> core::fmt::Debug for DmaRef<L> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_tuple("DmaRef")
            .field(&self.address)
            .finish()
    }
}
