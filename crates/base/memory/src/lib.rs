#![no_std]
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

//! Audited storage primitives for radio pipelines.
//!
//! This crate deliberately does not know about a chip, executor, network
//! stack, descriptor layout or allocator. [`StableDmaBacking`] proves that a
//! retained TX allocation cannot move, while [`RxHandoffPool`] transfers one
//! final receive buffer through finite state-specific leases.
//! [`StableDmaRange`] carries an unforgeable address/lifetime proof from those
//! audited storage leaves into otherwise safe register APIs. Higher protocol
//! and integration crates use only these safe capabilities. [`zeroed`] gives
//! statics in regions the boot zeroes an initializer that is all zero bytes.

use core::marker::PhantomData;

mod affine_spsc;
mod indexed_custody;
mod pinned_tx;
mod rx_external_handoff;
mod rx_handoff;
pub mod zeroed;

pub use affine_spsc::{
    AffineIndexedSpscQueue, AffineIndexedSpscReceiver, AffineIndexedSpscSender, AffineSpscQueue,
    AffineSpscReceiver, AffineSpscSendPermit, AffineSpscSender, AffineSpscTryReceiveError,
    AffineSpscTrySendError, UniqueQueueIndex,
};
pub use indexed_custody::{CustodyDomain, Drained, IndexedCustody, Quarantine, RetainedName};
pub use pinned_tx::{
    DmaIndexReturn, IndexedStableDmaLease, PinnedDmaTxNetworkLease, PinnedDmaTxPool,
    PinnedDmaTxPoolHandle, PinnedDmaTxRadioLease, ReturningStableDmaBacking,
    TaggedStableDmaBacking,
};
pub use rx_external_handoff::{
    ExternalRxAdoption, ExternalRxBuffer, ExternalRxHandoffPool, ExternalRxNetworkLease,
    ExternalRxRadioLease,
};
pub use rx_handoff::{RxHandoffPool, RxNetworkLease, RxRadioLease};

/// Authority to prepare registers for one retained, software-owned TX chain.
///
/// This trait deliberately exposes only the chain head, not its underlying
/// [`StableDmaRange`]. Register adapters can therefore validate a TX control
/// image without treating the same authority as an RX-walker allocation.
///
/// # Safety
///
/// Implementations must retain a complete, initialized DMA descriptor chain
/// and all of its referenced backing at stable addresses. Hardware must not
/// own the chain while a value implementing this trait exists. Safe code must
/// not be able to construct an implementation independently of that owner.
#[allow(
    unsafe_code,
    reason = "TX prepare authority is implemented only by audited DMA owners"
)]
pub unsafe trait PreparedTxDma {
    fn descriptor_head(&self) -> u32;
}

/// Start authority for one TX chain already recorded as hardware-owned.
///
/// This is intentionally a separate trait rather than a subtype of
/// [`PreparedTxDma`]: a prepare token must not be accepted at the final queue
/// doorbell, and a start token no longer grants CPU mutation of descriptors.
///
/// # Safety
///
/// Before exposing an implementation, the owner must record that hardware
/// owns the complete descriptor chain and backing allocation. It must retain
/// or quarantine that allocation until completion, a proven abort, or a full
/// reset stops the DMA actor. Safe code must not be able to forge the token.
#[allow(
    unsafe_code,
    reason = "TX start authority is implemented only by audited DMA owners"
)]
pub unsafe trait HardwareOwnedTxDma {
    fn descriptor_head(&self) -> u32;
}

/// Non-forgeable proof that one address range remains valid for DMA.
///
/// The value deliberately exposes observation but no safe constructor. A
/// chip storage leaf establishes the allocation, aliasing and hardware
/// lifetime proof once; safe register APIs can then require this capability
/// instead of accepting an unqualified integer address.
pub struct StableDmaRange<'storage> {
    start: u32,
    len: u32,
    _storage: PhantomData<&'storage mut ()>,
}

impl<'storage> StableDmaRange<'storage> {
    /// Bind a DMA range to an owner borrow after validating its lifecycle.
    ///
    /// # Safety
    ///
    /// `start..start + len` must name storage governed by `owner`. That
    /// storage must remain at the same address, and all CPU/hardware access
    /// must remain synchronized, for the hardware's complete use of the
    /// range. If a safe owner may be dropped or forgotten while DMA remains
    /// active, the backing allocation must remain valid independently (for
    /// example, static storage).
    #[allow(
        unsafe_code,
        reason = "constructor is the audited DMA-address authority boundary"
    )]
    pub unsafe fn from_owner<T: ?Sized>(_owner: &'storage T, start: u32, len: u32) -> Option<Self> {
        start.checked_add(len)?;
        (len != 0).then_some(Self {
            start,
            len,
            _storage: PhantomData,
        })
    }

    /// Construct synthetic/raw authority for a target validation boundary.
    ///
    /// # Safety
    ///
    /// The complete range must remain valid and exclusively governed by the
    /// caller until the hardware walker is stopped. Native models may use
    /// this only when no asynchronous DMA actor exists.
    #[allow(
        unsafe_code,
        reason = "raw validation harnesses cannot carry a Rust storage owner"
    )]
    pub unsafe fn from_raw_parts(start: u32, len: u32) -> Option<StableDmaRange<'static>> {
        start.checked_add(len)?;
        (len != 0).then_some(StableDmaRange {
            start,
            len,
            _storage: PhantomData,
        })
    }

    pub const fn start(&self) -> u32 {
        self.start
    }

    pub const fn contains(&self, address: u32, len: u32) -> bool {
        if len == 0 {
            return false;
        }
        let Some(offset) = address.checked_sub(self.start) else {
            return false;
        };
        let Some(end) = offset.checked_add(len) else {
            return false;
        };
        end <= self.len
    }
}

/// Exclusive view of one DMA-capable region at its stable address.
///
/// The view itself is ordinary borrowed memory. Address stability beyond this
/// short borrow is supplied by the [`StableDmaBacking`] implementation and is
/// valid while the backing owner remains exclusively retained, including when
/// that owner is leaked. The region's borrow does not itself prove this.
pub struct StableDmaRegion<'a> {
    storage: &'a mut [u8],
}

impl<'a> StableDmaRegion<'a> {
    /// Construct a view after proving that the allocation does not move while
    /// its backing owner is retained.
    ///
    /// # Safety
    ///
    /// `storage` must keep the same address until the `StableDmaBacking`
    /// owner which returned this region is dropped or explicitly released.
    /// Forgetting that owner must leave the allocation live and exclusively
    /// reserved indefinitely, even after any borrows carried by the owner end.
    #[allow(
        unsafe_code,
        reason = "constructor is the single stable-address proof boundary"
    )]
    pub const unsafe fn new(storage: &'a mut [u8]) -> Self {
        Self { storage }
    }

    pub const fn len(&self) -> usize {
        self.storage.len()
    }

    pub const fn is_empty(&self) -> bool {
        self.storage.is_empty()
    }

    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        self.storage
    }

    /// Consume the short region view and retain the original backing borrow.
    ///
    /// This is useful when an owner must return a slice borrowed from its
    /// retained lease instead of from the temporary region wrapper.
    pub fn into_mut_slice(self) -> &'a mut [u8] {
        self.storage
    }
}

/// An owned lease whose DMA region remains at one address until its explicit
/// release or destructor, and remains reserved indefinitely if it is leaked.
///
/// # Safety
///
/// Implementations must return the same non-moving allocation for the entire
/// lifetime of `self`. No safe operation may release, relocate or alias that
/// allocation while the owner is retained by a hardware transaction.
/// This includes `mem::forget` and retention in `ManuallyDrop`: ending a borrow
/// carried by the lease must not free the allocation or allow its reuse. A
/// lease of ordinary stack memory or of another independently droppable owner
/// cannot implement this trait. The allocation must instead be permanently
/// retained storage, or owned by the lease so that leaking it leaks the memory.
///
/// CPU-only metadata and return callbacks may borrow shorter-lived objects.
/// Hardware must not access them, and no leaked-owner path may invoke them or
/// run their destructors. Thus `Self: 'static` is not necessary; the allocation
/// and its exclusion from reuse, rather than every field of `Self`, must
/// survive an abandoned transaction.
#[allow(
    unsafe_code,
    reason = "implementing the stable-address invariant requires an audited pinned pool"
)]
pub unsafe trait StableDmaBacking {
    fn stable_dma_region(&mut self) -> StableDmaRegion<'_>;

    /// Make CPU writes visible before a DMA reader receives ownership.
    ///
    /// Internal uncached SRAM implementations need no action. Cached external
    /// memory implementations must write back their cache-line-isolated region
    /// here. This is deliberately separate from [`Self::stable_dma_region`]:
    /// upper layers mutate that region after borrowing it, so coherency belongs
    /// at the final descriptor-publication edge.
    fn prepare_for_dma_read(&mut self) {}
}
