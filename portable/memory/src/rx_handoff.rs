//! Bounded RX storage whose state-specific leases hand frames between tasks.

use core::{
    cell::UnsafeCell,
    sync::atomic::{AtomicU8, AtomicUsize, Ordering},
};

const SLOT_FREE: u8 = 0;
const SLOT_NETWORK: u8 = 1;
const SLOT_READY: u8 = 2;
const SLOT_RADIO: u8 = 3;

#[repr(C, align(16))]
struct RxHandoffSlot<const FRAME_CAPACITY: usize> {
    bytes: UnsafeCell<[u8; FRAME_CAPACITY]>,
    offset: AtomicUsize,
    length: AtomicUsize,
    state: AtomicU8,
}

impl<const FRAME_CAPACITY: usize> RxHandoffSlot<FRAME_CAPACITY> {
    const fn new() -> Self {
        Self {
            bytes: UnsafeCell::new([0; FRAME_CAPACITY]),
            offset: AtomicUsize::new(0),
            length: AtomicUsize::new(0),
            state: AtomicU8::new(SLOT_FREE),
        }
    }

    fn claim(&self, from: u8, to: u8, message: &str) {
        assert_eq!(
            self.state
                .compare_exchange(from, to, Ordering::AcqRel, Ordering::Acquire),
            Ok(from),
            "{message}"
        );
    }

    fn try_claim(&self, from: u8, to: u8) -> bool {
        self.state
            .compare_exchange(from, to, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    fn publish_ready(&self, owner: u8, offset: usize, length: usize, message: &str) {
        self.offset.store(offset, Ordering::Relaxed);
        self.length.store(length, Ordering::Relaxed);
        assert_eq!(
            self.state
                .compare_exchange(owner, SLOT_READY, Ordering::Release, Ordering::Acquire),
            Ok(owner),
            "{message}"
        );
    }

    fn release(&self, owner: u8, message: &str) {
        self.offset.store(0, Ordering::Relaxed);
        self.length.store(0, Ordering::Relaxed);
        self.claim(owner, SLOT_FREE, message);
    }

    fn offset(&self) -> usize {
        self.offset.load(Ordering::Acquire)
    }

    fn length(&self) -> usize {
        // A network lease exists only after `claim` acquires the slot's
        // Release publication. The offset load retains the explicit range
        // observation boundary; a second Acquire for the adjacent length
        // would add no synchronization and emits a duplicate fence on RV32.
        self.length.load(Ordering::Relaxed)
    }

    fn state(&self) -> u8 {
        self.state.load(Ordering::Acquire)
    }

    fn storage_mut_ptr(&self) -> *mut u8 {
        self.bytes.get().cast::<u8>()
    }
}

// SAFETY: the atomic state machine and non-Clone leases allow exactly one
// stage to access the UnsafeCell. Release/acquire transitions publish bytes
// and length before ownership is handed to the next stage.
#[allow(unsafe_code, reason = "RX handoff state machine is its Sync boundary")]
unsafe impl<const FRAME_CAPACITY: usize> Sync for RxHandoffSlot<FRAME_CAPACITY> {}

/// Fixed storage for copy-minimal radio-to-network frame handoff.
///
/// The pool itself need not be exposed to either task. They receive
/// state-specific leases that cannot perform another stage's transition.
pub struct RxHandoffPool<const FRAME_CAPACITY: usize, const QUEUE_DEPTH: usize> {
    slots: [RxHandoffSlot<FRAME_CAPACITY>; QUEUE_DEPTH],
}

impl<const FRAME_CAPACITY: usize, const QUEUE_DEPTH: usize>
    RxHandoffPool<FRAME_CAPACITY, QUEUE_DEPTH>
{
    pub const fn new() -> Self {
        Self {
            slots: [const { RxHandoffSlot::new() }; QUEUE_DEPTH],
        }
    }

    pub fn claim_radio(&self, index: u8) -> RxRadioLease<'_, FRAME_CAPACITY> {
        let slot = self
            .slots
            .get(usize::from(index))
            .expect("RX handoff index belongs to this pool");
        slot.claim(
            SLOT_FREE,
            SLOT_RADIO,
            "free-channel entry did not name a free RX handoff slot",
        );
        RxRadioLease {
            slot,
            index,
            live: true,
        }
    }

    /// Claim the first free slot without exposing its storage.
    pub fn try_claim_radio(&self) -> Option<RxRadioLease<'_, FRAME_CAPACITY>> {
        self.try_claim_radio_from(0)
    }

    /// Claim a free slot starting at a caller-owned round-robin hint.
    ///
    /// The atomic slot transition remains the ownership proof; `start` is
    /// only an efficiency hint and therefore need not be synchronized with a
    /// consumer which releases an earlier slot concurrently.
    pub fn try_claim_radio_from(&self, start: usize) -> Option<RxRadioLease<'_, FRAME_CAPACITY>> {
        if QUEUE_DEPTH > usize::from(u8::MAX) + 1 {
            return None;
        }
        if QUEUE_DEPTH == 0 {
            return None;
        }
        (0..QUEUE_DEPTH).find_map(|distance| {
            let index = (start + distance) % QUEUE_DEPTH;
            let slot = &self.slots[index];
            slot.try_claim(SLOT_FREE, SLOT_RADIO).then(|| RxRadioLease {
                slot,
                index: index as u8,
                live: true,
            })
        })
    }

    pub fn claim_network(&self, index: u8) -> RxNetworkLease<'_, FRAME_CAPACITY> {
        let slot = self
            .slots
            .get(usize::from(index))
            .expect("RX handoff index belongs to this pool");
        slot.claim(
            SLOT_READY,
            SLOT_NETWORK,
            "ready-channel entry did not name a ready RX handoff slot",
        );
        RxNetworkLease {
            slot,
            index,
            live: true,
        }
    }

    /// Number of slots retained by any pipeline stage.
    pub fn claimed_slots(&self) -> usize {
        self.slots
            .iter()
            .filter(|slot| slot.state() != SLOT_FREE)
            .count()
    }

    /// Number of slots currently exposed to the network stage.
    pub fn network_slots(&self) -> usize {
        self.slots
            .iter()
            .filter(|slot| slot.state() == SLOT_NETWORK)
            .count()
    }
}

impl<const FRAME_CAPACITY: usize, const QUEUE_DEPTH: usize> Default
    for RxHandoffPool<FRAME_CAPACITY, QUEUE_DEPTH>
{
    fn default() -> Self {
        Self::new()
    }
}

/// Unique radio writer for one free RX handoff slot.
pub struct RxRadioLease<'pool, const FRAME_CAPACITY: usize> {
    slot: &'pool RxHandoffSlot<FRAME_CAPACITY>,
    index: u8,
    live: bool,
}

impl<'pool, const FRAME_CAPACITY: usize> RxRadioLease<'pool, FRAME_CAPACITY> {
    pub const fn index(&self) -> usize {
        self.index as usize
    }

    pub fn frame_prefix_mut(&mut self, length: usize) -> &mut [u8] {
        assert!(length <= FRAME_CAPACITY, "RX frame exceeds slot capacity");
        // SAFETY: `&mut self` borrows the unique non-Clone SLOT_RADIO lease.
        #[allow(unsafe_code, reason = "radio lease uniquely owns its RX slot")]
        unsafe {
            core::slice::from_raw_parts_mut(self.slot.storage_mut_ptr(), length)
        }
    }

    pub fn publish<R>(mut self, length: usize, write: impl FnOnce(&mut [u8]) -> R) -> (u8, R) {
        assert!(length <= FRAME_CAPACITY, "RX frame exceeds slot capacity");
        let result = write(self.frame_prefix_mut(length));
        self.slot.publish_ready(
            SLOT_RADIO,
            0,
            length,
            "only the radio lease may publish an RX handoff slot",
        );
        self.live = false;
        (self.index, result)
    }

    /// Publish already initialized bytes directly to a network lease.
    pub fn into_network(mut self, length: usize) -> RxNetworkLease<'pool, FRAME_CAPACITY> {
        assert!(length <= FRAME_CAPACITY, "RX frame exceeds slot capacity");
        self.slot.publish_ready(
            SLOT_RADIO,
            0,
            length,
            "only the radio lease may publish an RX handoff slot",
        );
        self.slot.claim(
            SLOT_READY,
            SLOT_NETWORK,
            "only the just-published RX slot may enter network ownership",
        );
        self.live = false;
        RxNetworkLease {
            slot: self.slot,
            index: self.index,
            live: true,
        }
    }

    pub fn release(mut self) -> u8 {
        self.slot.release(
            SLOT_RADIO,
            "only an unpublished radio lease may return an RX handoff slot",
        );
        self.live = false;
        self.index
    }
}

impl<const FRAME_CAPACITY: usize> Drop for RxRadioLease<'_, FRAME_CAPACITY> {
    fn drop(&mut self) {
        if self.live {
            self.slot.release(
                SLOT_RADIO,
                "only the live radio lease may return an RX handoff slot",
            );
        }
    }
}

/// Unique network reader for one ready RX handoff slot.
pub struct RxNetworkLease<'pool, const FRAME_CAPACITY: usize> {
    slot: &'pool RxHandoffSlot<FRAME_CAPACITY>,
    index: u8,
    live: bool,
}

impl<const FRAME_CAPACITY: usize> RxNetworkLease<'_, FRAME_CAPACITY> {
    pub const fn index(&self) -> usize {
        self.index as usize
    }

    pub fn frame(&self) -> &[u8] {
        let offset = self.slot.offset();
        let length = self.slot.length();
        // SAFETY: this non-Clone token uniquely retains SLOT_NETWORK; safe
        // pool operations cannot mutate or reclaim the matching storage.
        #[allow(unsafe_code, reason = "network lease uniquely retains its RX slot")]
        unsafe {
            core::slice::from_raw_parts(self.slot.storage_mut_ptr().add(offset), length)
        }
    }

    #[inline(always)]
    pub fn with_frame<R>(&mut self, read: impl FnOnce(&mut [u8]) -> R) -> R {
        let offset = self.slot.offset();
        let length = self.slot.length();
        // SAFETY: `&mut self` borrows the unique non-Clone SLOT_NETWORK lease.
        #[allow(unsafe_code, reason = "network lease uniquely owns its RX slot")]
        let frame = unsafe {
            core::slice::from_raw_parts_mut(self.slot.storage_mut_ptr().add(offset), length)
        };
        read(frame)
    }

    /// Re-publish an initialized subrange for a second consumer stage.
    ///
    /// This supports a protocol owner that formats its staged 802.11 storage
    /// as an Ethernet frame in place. The returned index is the only value
    /// that crosses the ready queue; this lease no longer owns the slot.
    #[inline(always)]
    pub fn republish(mut self, offset: usize, length: usize) -> u8 {
        let current_offset = self.slot.offset();
        let current_end = current_offset
            .checked_add(self.slot.length())
            .expect("live RX range fits its slot");
        let end = offset
            .checked_add(length)
            .expect("republished RX range length cannot overflow");
        assert!(
            offset >= current_offset && end <= current_end && end <= FRAME_CAPACITY,
            "republished RX range must stay inside initialized storage"
        );
        self.slot.publish_ready(
            SLOT_NETWORK,
            offset,
            length,
            "only the protocol/network lease may republish an RX handoff slot",
        );
        self.live = false;
        self.index
    }

    pub fn release(mut self) -> u8 {
        self.slot.release(
            SLOT_NETWORK,
            "only the network lease may return an RX handoff slot",
        );
        self.live = false;
        self.index
    }
}

impl<const FRAME_CAPACITY: usize> Drop for RxNetworkLease<'_, FRAME_CAPACITY> {
    fn drop(&mut self) {
        if self.live {
            self.slot.release(
                SLOT_NETWORK,
                "only the live network lease may return an RX handoff slot",
            );
        }
    }
}

#[cfg(test)]
mod tests;
