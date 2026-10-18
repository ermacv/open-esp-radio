//! Typed control-plane handoff for receive BlockAck reordering.
//!
//! ADDBA/DELBA processing owns protocol and PAC state in the connected
//! control task. Staged frame leases belong exclusively to the RX protocol
//! task, which moves only genuinely out-of-order MPDUs into an independent
//! cold backing. The bounded mailbox carries only the semantic agreement edge
//! between those owners; it never carries a frame pointer or C context.

#![forbid(unsafe_code)]

use core::sync::atomic::{AtomicUsize, Ordering};

use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_sync::{
    blocking_mutex::raw::CriticalSectionRawMutex,
    channel::{Channel, Receiver, Sender, TrySendError},
    mutex::{Mutex, MutexGuard},
};
use oer_esp32s31_ieee80211_mac::rx::RxSegment;
pub use oer_esp32s31_ieee80211_mac::rx::ampdu::{
    RxBlockAckIdentity, RxBlockAckSnapshot, RxReorderCommand, RxReorderCommandError,
};

/// One command per possible RX agreement plus replacement/teardown slack.
///
/// The command path is not a packet queue. Capacity only has to cover finite
/// control-plane progress while the RX protocol task is scheduled elsewhere.
pub const RX_REORDER_COMMAND_CAPACITY: usize = 16;

/// One logical slot for every sequence position in the vendor maximum window.
pub const RX_REORDER_BACKING_SLOT_COUNT: usize = 64;
/// Ephemeral owner identity used only while an in-order frame crosses
/// `ingest`. It is never retained in the reorder state or mapped to PSRAM.
pub(crate) const RX_REORDER_CURRENT_SLOT: usize = RX_REORDER_BACKING_SLOT_COUNT;
pub(crate) const RX_REORDER_SLOT_DOMAIN: usize = RX_REORDER_BACKING_SLOT_COUNT + 1;
const RX_REORDER_BACKING_BITMAP_WORDS: usize = 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxReorderStorageError {
    Exhausted,
    TooLong(usize),
}

#[repr(C, align(4))]
struct RxReorderSlot<const CAPACITY: usize>(Mutex<CriticalSectionRawMutex, [u8; CAPACITY]>);

impl<const CAPACITY: usize> RxReorderSlot<CAPACITY> {
    const fn new() -> Self {
        Self(Mutex::new([0; CAPACITY]))
    }
}

/// CPU-only backing for MPDUs that actually cross a BlockAck sequence gap.
///
/// The ordinary SRAM staging lease remains the zero-extra-copy fast path for
/// in-order frames. Only a retained frame is copied here, after which its hot
/// staging credit is returned immediately to the DMA/protocol handoff.
pub struct RxReorderFrameStorage<
    const CAPACITY: usize,
    const SLOTS: usize = RX_REORDER_BACKING_SLOT_COUNT,
> {
    slots: [RxReorderSlot<CAPACITY>; SLOTS],
    claimed: [AtomicUsize; RX_REORDER_BACKING_BITMAP_WORDS],
}

impl<const CAPACITY: usize, const SLOTS: usize> RxReorderFrameStorage<CAPACITY, SLOTS> {
    pub const fn new() -> Self {
        assert!(SLOTS != 0, "RX reorder storage must not be empty");
        assert!(
            SLOTS <= RX_REORDER_BACKING_SLOT_COUNT,
            "RX reorder storage exceeds the MAC slot domain"
        );
        Self {
            slots: [const { RxReorderSlot::new() }; SLOTS],
            claimed: [const { AtomicUsize::new(0) }; RX_REORDER_BACKING_BITMAP_WORDS],
        }
    }

    pub fn try_reserve(
        &self,
    ) -> Result<RxReorderReservation<'_, CAPACITY, SLOTS>, RxReorderStorageError> {
        let slot = (0..SLOTS)
            .find(|&slot| self.try_claim(slot))
            .ok_or(RxReorderStorageError::Exhausted)?;
        Ok(RxReorderReservation {
            storage: self,
            slot,
            live: true,
        })
    }

    pub fn available_slots(&self) -> usize {
        SLOTS.saturating_sub(
            self.claimed
                .iter()
                .map(|word| word.load(Ordering::Acquire).count_ones() as usize)
                .sum(),
        )
    }

    fn try_claim(&self, slot: usize) -> bool {
        let (word, bit) = reorder_bitmap_word_and_bit(slot);
        self.claimed[word].fetch_or(bit, Ordering::AcqRel) & bit == 0
    }

    fn release(&self, slot: usize) -> bool {
        let (word, bit) = reorder_bitmap_word_and_bit(slot);
        self.claimed[word].fetch_and(!bit, Ordering::AcqRel) & bit != 0
    }
}

impl<const CAPACITY: usize, const SLOTS: usize> Default for RxReorderFrameStorage<CAPACITY, SLOTS> {
    fn default() -> Self {
        Self::new()
    }
}

/// Claimed logical sequence slot before the reorder state decides whether the
/// current SRAM staging frame must be retained.
pub struct RxReorderReservation<'storage, const CAPACITY: usize, const SLOTS: usize> {
    storage: &'storage RxReorderFrameStorage<CAPACITY, SLOTS>,
    slot: usize,
    live: bool,
}

impl<'storage, const CAPACITY: usize, const SLOTS: usize>
    RxReorderReservation<'storage, CAPACITY, SLOTS>
{
    pub const fn slot(&self) -> usize {
        self.slot
    }

    pub fn copy_from(
        mut self,
        segment: RxSegment<'_>,
    ) -> Result<RxReorderFrame<'storage, CAPACITY, SLOTS>, (RxReorderStorageError, Self)> {
        if segment.buffer.len() > CAPACITY {
            return Err((RxReorderStorageError::TooLong(segment.buffer.len()), self));
        }
        let mut bytes = self.storage.slots[self.slot]
            .0
            .try_lock()
            .expect("a reserved RX reorder slot has no competing byte owner");
        bytes[..segment.buffer.len()].copy_from_slice(segment.buffer);
        drop(bytes);
        self.live = false;
        Ok(RxReorderFrame {
            storage: self.storage,
            slot: self.slot,
            descriptor_address: segment.descriptor_address,
            descriptor_word0: segment.descriptor_word0,
            next_descriptor_address: segment.next_descriptor_address,
            length: segment.buffer.len(),
        })
    }
}

impl<const CAPACITY: usize, const SLOTS: usize> Drop for RxReorderReservation<'_, CAPACITY, SLOTS> {
    fn drop(&mut self) {
        if self.live {
            let released = self.storage.release(self.slot);
            debug_assert!(released);
        }
    }
}

/// Unique retained MPDU owner in the cold PSRAM reorder backing.
pub struct RxReorderFrame<'storage, const CAPACITY: usize, const SLOTS: usize> {
    storage: &'storage RxReorderFrameStorage<CAPACITY, SLOTS>,
    slot: usize,
    descriptor_address: u32,
    descriptor_word0: u32,
    next_descriptor_address: u32,
    length: usize,
}

/// Locked read view of one retained reorder slot.
///
/// The guard may live across the async protocol dispatch. The logical frame
/// token keeps the same slot claimed, so no writer can reserve it until the
/// frame and this view have both been dropped.
pub struct RxReorderSegment<'frame, const CAPACITY: usize> {
    bytes: MutexGuard<'frame, CriticalSectionRawMutex, [u8; CAPACITY]>,
    descriptor_address: u32,
    descriptor_word0: u32,
    next_descriptor_address: u32,
    length: usize,
}

impl<const CAPACITY: usize> RxReorderSegment<'_, CAPACITY> {
    pub fn as_segment(&self) -> RxSegment<'_> {
        RxSegment {
            descriptor_address: self.descriptor_address,
            descriptor_word0: self.descriptor_word0,
            buffer: &self.bytes[..self.length],
            next_descriptor_address: self.next_descriptor_address,
        }
    }
}

impl<const CAPACITY: usize, const SLOTS: usize> RxReorderFrame<'_, CAPACITY, SLOTS> {
    pub const fn slot(&self) -> usize {
        self.slot
    }

    pub fn segment(&self) -> RxReorderSegment<'_, CAPACITY> {
        let bytes = self.storage.slots[self.slot]
            .0
            .try_lock()
            .expect("a retained RX reorder frame has unique byte ownership");
        RxReorderSegment {
            bytes,
            descriptor_address: self.descriptor_address,
            descriptor_word0: self.descriptor_word0,
            next_descriptor_address: self.next_descriptor_address,
            length: self.length,
        }
    }
}

impl<const CAPACITY: usize, const SLOTS: usize> Drop for RxReorderFrame<'_, CAPACITY, SLOTS> {
    fn drop(&mut self) {
        let released = self.storage.release(self.slot);
        debug_assert!(released);
    }
}

const fn reorder_bitmap_word_and_bit(slot: usize) -> (usize, usize) {
    let bits = usize::BITS as usize;
    (slot / bits, 1_usize << (slot % bits))
}

pub type RxReorderCommandSender<'resources, M> =
    Sender<'resources, M, RxReorderCommand, RX_REORDER_COMMAND_CAPACITY>;
pub type RxReorderCommandReceiver<'resources, M> =
    Receiver<'resources, M, RxReorderCommand, RX_REORDER_COMMAND_CAPACITY>;

/// Static storage shared by the connected control and RX protocol tasks.
pub struct RxReorderCommandResources<M: RawMutex> {
    commands: Channel<M, RxReorderCommand, RX_REORDER_COMMAND_CAPACITY>,
}

impl<M: RawMutex> RxReorderCommandResources<M> {
    pub const fn new() -> Self {
        Self {
            commands: Channel::new(),
        }
    }

    pub fn split(
        &self,
    ) -> (
        RxReorderCommandSender<'_, M>,
        RxReorderCommandReceiver<'_, M>,
    ) {
        (self.commands.sender(), self.commands.receiver())
    }
}

impl<M: RawMutex> Default for RxReorderCommandResources<M> {
    fn default() -> Self {
        Self::new()
    }
}

pub fn try_send_rx_reorder_command<M: RawMutex>(
    sender: &RxReorderCommandSender<'_, M>,
    command: RxReorderCommand,
) -> Result<(), RxReorderCommandError> {
    sender.try_send(command).map_err(|error| match error {
        TrySendError::Full(command) => RxReorderCommandError::Full(command),
    })
}

pub fn try_receive_rx_reorder_command<M: RawMutex>(
    receiver: &RxReorderCommandReceiver<'_, M>,
) -> Option<RxReorderCommand> {
    receiver.try_receive().ok()
}

#[cfg(test)]
mod tests;
