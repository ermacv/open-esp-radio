//! Zero-copy RX: network ownership of detached DMA buffers.
//!
//! A radio datapath which holds a received Ethernet frame in a stable
//! [`ExternalRxHandoffPool`] slot publishes that slot to Xarxa as a
//! [`PacketBuf`] instead of copying it into the endpoint's RX pool. The packet
//! then retains the physical DMA credit until the stack drops it, so the
//! number of slots the network may hold is capped below the pool size. The
//! reserve left above the cap is what the driver needs to keep staging a
//! complete receive burst while the stack is not draining.
//!
//! Admission happens before the radio rewrites the frame in place, so a
//! refused admission leaves the staged frame untouched for the copying path.

use core::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use embassy_sync::blocking_mutex::raw::RawMutex;
use oer_memory::{ExternalRxAdoption, ExternalRxHandoffPool};
use xarxa_driver::config::{PACKET_BUF_ALIGN, PACKET_BUF_SIZE};
use xarxa_driver::{ExternalPacketOrigin, PacketBuf};

use super::OwnedRxPublisher;
use crate::{ETHERNET_HEADER_LEN, FrameLengthError, RxEnqueueError};

/// Network side of one handoff pool, shared by every endpoint it feeds.
///
/// Place it in a `static` whose own address is passed to [`new`](Self::new).
/// The held count is physical: one DMA slot per adopted packet, whichever
/// endpoint or socket retains it.
pub struct ExternalRxOrigin<const FRAME_CAPACITY: usize, const SLOTS: usize> {
    this: *const Self,
    pool: &'static ExternalRxHandoffPool<FRAME_CAPACITY, SLOTS>,
    packets: ExternalPacketOrigin<SLOTS>,
    cap: usize,
    held: AtomicUsize,
    peak_held: AtomicUsize,
    adopted: AtomicU32,
    copied_over_cap: AtomicU32,
    copied_unfit: AtomicU32,
    dropped: AtomicU32,
}

#[allow(
    unsafe_code,
    reason = "the static origin is shared by RX producers and packet drops"
)]
// SAFETY: `this` is only compared with `self` and passed to the release
// callback, which is callable from any core. Every mutable field is atomic,
// and the Xarxa origin and handoff pool are `Sync`.
unsafe impl<const FRAME_CAPACITY: usize, const SLOTS: usize> Sync
    for ExternalRxOrigin<FRAME_CAPACITY, SLOTS>
{
}

/// Snapshot of one origin's zero-copy RX accounting.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ExternalRxCounters {
    /// Frames published to the stack without a copy.
    pub adopted: u32,
    /// Admissions refused at the cap; the caller copies those frames.
    pub copied_over_cap: u32,
    /// Admitted frames whose storage did not fit a packet and were copied.
    pub copied_unfit: u32,
    /// Admitted frames dropped because the link was down or the queue full.
    pub dropped: u32,
    /// Slots currently held by admissions and adopted packets.
    pub held: usize,
    /// Highest `held` observed since construction.
    pub peak_held: usize,
}

impl ExternalRxCounters {
    /// Counts since `earlier`, with the current `held` and `peak_held`.
    ///
    /// The event counts wrap; `held` and `peak_held` are levels, so a
    /// per-interval peak needs [`ExternalRxOrigin::restart_peak`] at the
    /// start of the interval.
    pub const fn wrapping_delta_since(self, earlier: Self) -> Self {
        Self {
            adopted: self.adopted.wrapping_sub(earlier.adopted),
            copied_over_cap: self.copied_over_cap.wrapping_sub(earlier.copied_over_cap),
            copied_unfit: self.copied_unfit.wrapping_sub(earlier.copied_unfit),
            dropped: self.dropped.wrapping_sub(earlier.dropped),
            held: self.held,
            peak_held: self.peak_held,
        }
    }
}

/// Why [`OwnedRxPublisher::try_admit_external`] refused a zero-copy credit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExternalRxRefusal {
    /// The network already holds the capped number of slots.
    OverCap,
    /// The endpoint's link is down.
    LinkDown,
    /// The endpoint's RX queue has no free entry.
    QueueFull,
}

/// One reserved network credit for a frame not yet published.
///
/// Dropping it unused returns the credit.
#[must_use = "an unused admission only reserves a credit until it is dropped"]
pub struct ExternalRxAdmission<const FRAME_CAPACITY: usize, const SLOTS: usize> {
    origin: &'static ExternalRxOrigin<FRAME_CAPACITY, SLOTS>,
}

impl<const FRAME_CAPACITY: usize, const SLOTS: usize> Drop
    for ExternalRxAdmission<FRAME_CAPACITY, SLOTS>
{
    fn drop(&mut self) {
        self.origin.return_credit();
    }
}

impl<const FRAME_CAPACITY: usize, const SLOTS: usize> ExternalRxOrigin<FRAME_CAPACITY, SLOTS> {
    /// Bind the network side of `pool`, leaving `reserve` slots to the driver.
    ///
    /// `this` must be the address of the static being initialized; every
    /// publication checks it, so an origin moved out of its static cannot
    /// adopt.
    ///
    /// # Panics
    ///
    /// Panics unless `reserve < SLOTS`.
    pub const fn new(
        this: &'static Self,
        pool: &'static ExternalRxHandoffPool<FRAME_CAPACITY, SLOTS>,
        reserve: usize,
    ) -> Self {
        assert!(
            reserve < SLOTS,
            "the zero-copy cap must leave at least one slot"
        );
        let this: *const Self = this;
        Self {
            this,
            pool,
            #[allow(unsafe_code, reason = "the origin binds its own static release edge")]
            // SAFETY: `release_adopted_slot` only reaches `this`'s atomics and
            // its `'static` pool, both callable from any core. The packet's
            // slot is the handoff index, which is adopted again only after
            // the pool has released it: the radio must first claim the FREE
            // slot, which `release_adopted` publishes last.
            packets: unsafe {
                ExternalPacketOrigin::new(
                    this.cast(),
                    release_adopted_slot::<FRAME_CAPACITY, SLOTS>,
                )
            },
            cap: SLOTS - reserve,
            held: AtomicUsize::new(0),
            peak_held: AtomicUsize::new(0),
            adopted: AtomicU32::new(0),
            copied_over_cap: AtomicU32::new(0),
            copied_unfit: AtomicU32::new(0),
            dropped: AtomicU32::new(0),
        }
    }

    /// Maximum number of slots the network may hold.
    pub const fn cap(&self) -> usize {
        self.cap
    }

    /// Whether `packet` retains a slot of this origin.
    pub fn owns(&self, packet: &PacketBuf) -> bool {
        self.packets.owns(packet)
    }

    /// Current accounting snapshot.
    pub fn counters(&self) -> ExternalRxCounters {
        ExternalRxCounters {
            adopted: self.adopted.load(Ordering::Relaxed),
            copied_over_cap: self.copied_over_cap.load(Ordering::Relaxed),
            copied_unfit: self.copied_unfit.load(Ordering::Relaxed),
            dropped: self.dropped.load(Ordering::Relaxed),
            held: self.held.load(Ordering::Acquire),
            peak_held: self.peak_held.load(Ordering::Relaxed),
        }
    }

    /// Start a new peak interval at the current held count.
    pub fn restart_peak(&self) {
        self.peak_held
            .store(self.held.load(Ordering::Acquire), Ordering::Relaxed);
    }

    fn try_take_credit(&self) -> bool {
        let mut held = self.held.load(Ordering::Acquire);
        loop {
            if held >= self.cap {
                return false;
            }
            match self.held.compare_exchange_weak(
                held,
                held + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    self.peak_held.fetch_max(held + 1, Ordering::Relaxed);
                    return true;
                }
                Err(observed) => held = observed,
            }
        }
    }

    fn return_credit(&self) {
        let previous = self.held.fetch_sub(1, Ordering::AcqRel);
        debug_assert_ne!(previous, 0, "a zero-copy RX credit was returned twice");
    }

    /// Return one adopted slot's storage to the pool, then its credit.
    ///
    /// # Safety
    ///
    /// `index` came from `into_adoption` on this origin's pool and nothing
    /// accesses its bytes any more.
    #[allow(unsafe_code, reason = "forwards the adoption release contract")]
    unsafe fn release_slot(&self, index: u8) {
        // SAFETY: forwarded from this function's contract.
        unsafe { self.pool.release_adopted(index) };
        // The credit returns only after the DMA buffer did, so the network
        // never appears to hold fewer slots than the pool has retained.
        self.return_credit();
    }
}

/// Xarxa release callback of an [`ExternalRxOrigin`].
///
/// # Safety
///
/// `context` is the origin's own static address and `slot` the handoff index
/// of a packet this origin adopted, released once.
#[allow(unsafe_code, reason = "type-erased Xarxa release edge")]
unsafe fn release_adopted_slot<const FRAME_CAPACITY: usize, const SLOTS: usize>(
    context: *const (),
    slot: usize,
) {
    // SAFETY: `new` bound `context` to the origin's static address, checked
    // against `self` before every adoption.
    let origin = unsafe { &*context.cast::<ExternalRxOrigin<FRAME_CAPACITY, SLOTS>>() };
    let index = u8::try_from(slot).expect("adopted slot is a handoff index");
    // SAFETY: Xarxa calls this once per adoption after the packet stopped
    // accessing its storage; the slot came from `into_adoption`.
    unsafe { origin.release_slot(index) };
}

/// Whether adopted storage satisfies Xarxa's packet layout.
///
/// The packet starts at the Ethernet header with no headroom and must own a
/// full, aligned `PACKET_BUF_SIZE` view from there.
fn fits_packet(adoption: &ExternalRxAdoption) -> bool {
    adoption.length >= ETHERNET_HEADER_LEN
        && adoption.length <= PACKET_BUF_SIZE
        && adoption.available >= PACKET_BUF_SIZE
        && adoption
            .data
            .as_ptr()
            .addr()
            .is_multiple_of(PACKET_BUF_ALIGN)
}

impl<M: RawMutex, const RX_QUEUE_DEPTH: usize> OwnedRxPublisher<'_, M, RX_QUEUE_DEPTH> {
    /// Reserve one zero-copy credit before the radio rewrites a frame.
    ///
    /// A refusal leaves the frame to the copying [`try_send_parts`] path;
    /// [`ExternalRxRefusal::OverCap`] is counted as such.
    ///
    /// [`try_send_parts`]: Self::try_send_parts
    pub fn try_admit_external<const FRAME_CAPACITY: usize, const SLOTS: usize>(
        &self,
        origin: &'static ExternalRxOrigin<FRAME_CAPACITY, SLOTS>,
    ) -> Result<ExternalRxAdmission<FRAME_CAPACITY, SLOTS>, ExternalRxRefusal> {
        if !self.link.snapshot().up {
            return Err(ExternalRxRefusal::LinkDown);
        }
        if self.rx.is_full() {
            return Err(ExternalRxRefusal::QueueFull);
        }
        if !origin.try_take_credit() {
            origin.copied_over_cap.fetch_add(1, Ordering::Relaxed);
            return Err(ExternalRxRefusal::OverCap);
        }
        Ok(ExternalRxAdmission { origin })
    }

    /// Publish the ready handoff slot `index` under `admission`.
    ///
    /// The slot must hold a complete Ethernet frame published for the network.
    /// Whatever the outcome, the call consumes the slot: it is retained by an
    /// adopted packet, or its bytes were copied or dropped and the slot was
    /// released. Storage which does not fit a packet is copied into this
    /// endpoint's RX pool.
    pub fn publish_external<const FRAME_CAPACITY: usize, const SLOTS: usize>(
        &self,
        admission: ExternalRxAdmission<FRAME_CAPACITY, SLOTS>,
        index: u8,
    ) -> Result<(), RxEnqueueError> {
        let origin = admission.origin;
        assert!(
            core::ptr::eq(origin.this, origin),
            "an external RX origin publishes only from its own static"
        );
        // The admission's credit now belongs to the slot, returned by its
        // release below or by the adopted packet's drop.
        core::mem::forget(admission);
        let adoption = origin.pool.claim_network(index).into_adoption();
        if !fits_packet(&adoption) {
            origin.copied_unfit.fetch_add(1, Ordering::Relaxed);
            let result = self.copy_adopted(&adoption);
            #[allow(unsafe_code, reason = "returns a slot which was not adopted")]
            // SAFETY: the adoption came from this origin's pool and the copy
            // above no longer borrows its bytes.
            unsafe {
                origin.release_slot(adoption.index)
            };
            return result;
        }
        #[allow(unsafe_code, reason = "adopts a detached DMA buffer as a packet")]
        // SAFETY: `fits_packet` proved an aligned `PACKET_BUF_SIZE` view at
        // the Ethernet start inside the slot's stable, initialized allocation,
        // which the consumed network lease hands exclusively to this packet
        // until its release callback runs. The origin lives in its static.
        let packet = unsafe {
            origin.packets.adopt(
                usize::from(adoption.index),
                adoption.data,
                0,
                adoption.length,
            )
        };
        match self.try_publish(packet) {
            Ok(()) => {
                origin.adopted.fetch_add(1, Ordering::Relaxed);
                Ok(())
            }
            Err(error) => {
                // The rejected packet was dropped inside `try_publish`,
                // releasing its slot through the origin.
                origin.dropped.fetch_add(1, Ordering::Relaxed);
                Err(error)
            }
        }
    }

    fn copy_adopted(&self, adoption: &ExternalRxAdoption) -> Result<(), RxEnqueueError> {
        if adoption.length > adoption.available {
            return Err(RxEnqueueError::InvalidLength(FrameLengthError::TooLong));
        }
        #[allow(unsafe_code, reason = "reads the adopted frame before release")]
        // SAFETY: the adoption owns `available >= length` initialized bytes
        // from `data` until its slot is released by the caller.
        let frame = unsafe { core::slice::from_raw_parts(adoption.data.as_ptr(), adoption.length) };
        self.try_send(frame)
    }
}
