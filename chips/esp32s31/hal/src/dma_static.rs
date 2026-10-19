//! Statics placed in the DMA window (`docs/hardware-memory.md`).
//!
//! [`dma_static!`](crate::dma_static!) declares a zero-initialised static in
//! a `.dma.bss.<block>` section. The runtime's linker script gathers every
//! such section into its `.dma.bss` output and asserts that the whole DMA
//! range lies inside the DMA window and holds no address whose low DMA
//! address bits are zero (`dma-window.x`, generated from the register model).
//! A placed static is claimed once as a [`Placed`]: the proof a storage needs
//! before it publishes its own addresses to DMA hardware.

use oer_base_memory::{
    DmaIndexReturn, IndexedStableDmaLease, PinnedDmaTxNetworkLease, PinnedDmaTxPool,
    PinnedDmaTxPoolHandle, PinnedDmaTxRadioLease, ReturningStableDmaBacking, StableDmaBacking,
    StableDmaRegion, TaggedStableDmaBacking,
    zeroed::{Zeroable, ZeroedStatic},
};

/// The section prefix every DMA static uses.
pub const DMA_BSS_PREFIX: &str = ".dma.bss.";

/// Whether `section` is a `.dma.bss.<block>` input section.
pub const fn is_dma_section(section: &str) -> bool {
    let (section, prefix) = (section.as_bytes(), DMA_BSS_PREFIX.as_bytes());
    if section.len() <= prefix.len() {
        return false;
    }
    let mut index = 0;
    while index < prefix.len() {
        if section[index] != prefix[index] {
            return false;
        }
        index += 1;
    }
    true
}

/// A zero-initialised static placed in the DMA window, claimed once.
pub struct DmaStatic<T>(ZeroedStatic<T>);

impl<T: Zeroable> DmaStatic<T> {
    /// The unclaimed static.
    ///
    /// # Safety
    ///
    /// The value is a `static` in a `.dma.bss.<block>` section of a runtime
    /// image linked with the runtime's linker script: only
    /// [`dma_static!`](crate::dma_static!) calls this.
    #[doc(hidden)]
    #[allow(unsafe_code, reason = "the caller promises the placement")]
    pub const unsafe fn placed() -> Self {
        Self(ZeroedStatic::new())
    }
}

impl<T> DmaStatic<T> {
    /// The placed value, the first time; `None` once claimed.
    pub fn take(&'static self) -> Option<Placed<T>> {
        self.0.try_take().map(|value| Placed { value })
    }
}

/// A value placed in the DMA window, claimed once from its [`DmaStatic`]:
/// every address inside it lies in the window, off an address with zero low
/// DMA address bits.
pub struct Placed<T: 'static> {
    value: &'static mut T,
}

impl<T: 'static> Placed<T> {
    /// The placed value.
    pub fn into_inner(self) -> &'static mut T {
        self.value
    }

    /// A host model's value, as if placed: host builds have no DMA window,
    /// and their storages address DMA through their synthetic space.
    #[cfg(any(test, feature = "test-support"))]
    pub fn host(value: &'static mut T) -> Self {
        Self { value }
    }
}

impl<A: 'static, B: 'static> Placed<(A, B)> {
    /// The two values of a pair placed together: each lies inside the
    /// placed static, so each is placed.
    pub fn split(self) -> (Placed<A>, Placed<B>) {
        let (first, second) = self.value;
        (Placed { value: first }, Placed { value: second })
    }
}

/// A transmit pool placed in the DMA window ([`dma_static!`](crate::dma_static!)):
/// the radio leases it issues are [`PlacedTxLease`]s, buffers the DMA
/// hardware may read without a further address check.
pub struct PlacedTxPool<
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
> {
    handle: PinnedDmaTxPoolHandle<FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
}

impl<
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
> PlacedTxPool<FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>
{
    /// Pin the `placed` pool (coherent memory).
    pub fn pin(
        placed: Placed<PinnedDmaTxPool<FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>>,
    ) -> Self {
        Self {
            handle: PinnedDmaTxPool::pin_static(placed.into_inner()),
        }
    }

    /// Pin the `placed` pool, which needs `prepare_for_dma_read` before a
    /// DMA reader receives a buffer.
    pub fn pin_with_dma_read_prepare(
        placed: Placed<PinnedDmaTxPool<FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>>,
        prepare_for_dma_read: fn(&mut [u8]),
    ) -> Self {
        Self {
            handle: PinnedDmaTxPool::pin_static_with_dma_read_prepare(
                placed.into_inner(),
                prepare_for_dma_read,
            ),
        }
    }

    /// A free slot for a unique network writer
    /// ([`PinnedDmaTxPoolHandle::claim_network`]).
    pub fn claim_network(
        &self,
        index: u8,
    ) -> PinnedDmaTxNetworkLease<'static, FRAME_CAPACITY, HEADROOM, TRAILER> {
        self.handle.claim_network(index)
    }

    /// A published slot for the radio, as a placed buffer
    /// ([`PinnedDmaTxPoolHandle::claim_radio`]).
    pub fn claim_radio(
        &self,
        index: u8,
    ) -> PlacedTxLease<PinnedDmaTxRadioLease<'static, FRAME_CAPACITY, HEADROOM, TRAILER>> {
        PlacedTxLease(self.handle.claim_radio(index))
    }

    /// Slots held by a network, ready or radio stage.
    pub fn claimed_slots(&self) -> usize {
        self.handle.claimed_slots()
    }
}

/// A buffer lease of a pool placed in the DMA window: only
/// [`PlacedTxPool::claim_radio`] makes one on the chip.
pub struct PlacedTxLease<L>(L);

impl<L> PlacedTxLease<L> {
    /// A host model's lease, as if placed: host builds have no DMA window.
    #[cfg(any(test, feature = "test-support"))]
    pub fn host(lease: L) -> Self {
        Self(lease)
    }

    /// The lease.
    pub fn into_inner(self) -> L {
        self.0
    }
}

impl<L> core::ops::Deref for PlacedTxLease<L> {
    type Target = L;

    fn deref(&self) -> &L {
        &self.0
    }
}

impl<L> core::ops::DerefMut for PlacedTxLease<L> {
    fn deref_mut(&mut self) -> &mut L {
        &mut self.0
    }
}

#[allow(unsafe_code, reason = "the stability is the wrapped lease's")]
// SAFETY: the wrapper adds no state; the wrapped lease's stability proof is
// unchanged.
unsafe impl<L: StableDmaBacking> StableDmaBacking for PlacedTxLease<L> {
    fn stable_dma_region(&mut self) -> StableDmaRegion<'_> {
        self.0.stable_dma_region()
    }

    fn prepare_for_dma_read(&mut self) {
        self.0.prepare_for_dma_read();
    }
}

impl<L: IndexedStableDmaLease> IndexedStableDmaLease for PlacedTxLease<L> {
    fn release_index(self) -> u8 {
        self.0.release_index()
    }
}

mod sealed {
    pub trait Sealed {}
}

/// A stable DMA backing whose bytes lie in the DMA window: a
/// [`PlacedTxLease`], or a pool wrapper around one. Sealed: placement is the
/// HAL's to prove, so DMA engines may publish its address unchecked.
pub trait PlacedBacking: StableDmaBacking + sealed::Sealed {}

impl<L> sealed::Sealed for PlacedTxLease<L> {}
impl<L: StableDmaBacking> PlacedBacking for PlacedTxLease<L> {}
impl<T, B: PlacedBacking> sealed::Sealed for TaggedStableDmaBacking<T, B> {}
impl<T, B: PlacedBacking> PlacedBacking for TaggedStableDmaBacking<T, B> {}
impl<B: IndexedStableDmaLease + PlacedBacking, R: DmaIndexReturn> sealed::Sealed
    for ReturningStableDmaBacking<B, R>
{
}
impl<B: IndexedStableDmaLease + PlacedBacking, R: DmaIndexReturn> PlacedBacking
    for ReturningStableDmaBacking<B, R>
{
}

/// Declare a zero-initialised static in the DMA window: a
/// [`DmaStatic`](crate::dma_static::DmaStatic) in the `.dma.bss.<block>`
/// section named.
///
/// ```ignore
/// oer_esp32s31_hal::dma_static! {
///     /// The receive epoch's arena.
///     static RX_ARENA: Arena = zeroed in ".dma.bss.wifi_rx";
/// }
/// ```
#[macro_export]
macro_rules! dma_static {
    () => {};
    (
        $(#[$meta:meta])*
        $vis:vis static $name:ident: $ty:ty = zeroed in $section:literal;
        $($rest:tt)*
    ) => {
        const _: () = assert!(
            $crate::dma_static::is_dma_section($section),
            "a DMA static lives in a `.dma.bss.<block>` section"
        );
        $(#[$meta])*
        #[unsafe(link_section = $section)]
        $vis static $name: $crate::dma_static::DmaStatic<$ty> =
            // SAFETY: the static is in the `.dma.bss.<block>` section checked
            // above, which the runtime linker script places in `.dma.bss` and
            // asserts inside the DMA window.
            unsafe { $crate::dma_static::DmaStatic::placed() };
        $crate::dma_static! { $($rest)* }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_placed_pair_splits_into_its_own_two_values() {
        let pair: &'static mut (u32, [u8; 4]) =
            std::boxed::Box::leak(std::boxed::Box::new((7, [1; 4])));
        let (first_address, second_address) = (
            core::ptr::addr_of!(pair.0).addr(),
            core::ptr::addr_of!(pair.1).addr(),
        );
        let (first, second) = Placed::host(pair).split();
        let (first, second) = (first.into_inner(), second.into_inner());
        assert_eq!((*first, *second), (7, [1; 4]));
        assert_eq!(
            (
                core::ptr::from_mut(first).addr(),
                core::ptr::from_mut(second).addr()
            ),
            (first_address, second_address)
        );
    }

    #[test]
    fn only_a_named_dma_bss_section_is_a_dma_section() {
        assert!(is_dma_section(".dma.bss.wifi_rx"));
        for section in [
            ".dma.bss.",
            ".dma.bss",
            ".dma.data.wifi",
            ".bss.wifi",
            ".dma.bsss.x",
        ] {
            assert!(!is_dma_section(section), "{section}");
        }
    }

    #[test]
    // The macro's placement is unsafe code here, in its own crate; a foreign
    // crate's expansion of it is not linted.
    #[allow(unsafe_code, reason = "a DMA static's section placement")]
    fn a_placed_static_is_claimed_once() {
        crate::dma_static! {
            static CELL: [u32; 4] = zeroed in ".dma.bss.test";
        }
        let placed = CELL.take().unwrap();
        assert_eq!(*placed.into_inner(), [0; 4]);
        assert!(CELL.take().is_none());
    }
}
