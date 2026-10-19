//! ESP32-S31 BlockAck policy and fixed-slot completion adapter.
//!
//! Frame parsing and one generic agreement state machine live in
//! [`oer_ieee80211_mac::block_ack`], the station's originator in
//! `oer_ieee80211_mac::block_ack::originator` and the vendor TIDs and Dialog Tokens in
//! `oer_espressif_ieee80211_policy::block_ack`. This module retains only
//! the S31 window bound, the three-register completion snapshot and the
//! fixed hardware-slot batch. It still owns no DMA address or register access.

use oer_ieee80211_lower_mac::BlockAckReport;
pub use oer_ieee80211_mac::block_ack::{
    ADDBA_ACTION_BODY_LEN, ADDBA_REQUEST_ACTION, ADDBA_RESPONSE_ACTION, AddbaRequest,
    BLOCK_ACK_CATEGORY, BlockAckAction, DELBA_ACTION, OperationalTxBlockAck, TxBlockAckAlarm,
    TxBlockAckConfig, TxBlockAckDialogToken, TxBlockAckError, TxBlockAckResponse,
    TxBlockAckSession, parse_block_ack_action,
};
use oer_ieee80211_mac::sequence::SequenceNumber;

/// Strict S31 TX window recovered from the fixed vendor queue geometry.
pub const TX_BLOCK_ACK_MAX_WINDOW: u16 = 32;
pub const TX_AMPDU_SLOT_CAPACITY: usize = TX_BLOCK_ACK_MAX_WINDOW as usize;

/// Opaque index of one statically owned TX frame.
///
/// The strict S31 data path has exactly 32 fixed TX slots. Keeping only their
/// indices here prevents the BlockAck state machine from owning raw pointers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TxAmpduSlot(u8);

impl TxAmpduSlot {
    pub const fn new(index: u8) -> Option<Self> {
        if (index as usize) < TX_AMPDU_SLOT_CAPACITY {
            Some(Self(index))
        } else {
            None
        }
    }

    pub const fn index(self) -> u8 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TxAmpduMpdu {
    pub slot: TxAmpduSlot,
    pub sequence: SequenceNumber,
}

/// Semantic BlockAck information decoded by the PAC completion owner.
///
/// Bit zero acknowledges `starting_sequence`, bit one the following sequence,
/// and so on. The S31 completion block exposes 64 bits even though strict mode
/// deliberately negotiates a window of at most 32 frames.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TxBlockAckBitmap {
    pub starting_sequence: SequenceNumber,
    pub bitmap: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HtBlockAckObservation {
    pub control: u8,
    pub block_ack: TxBlockAckBitmap,
}

impl HtBlockAckObservation {
    pub const fn new(control: u8, starting_sequence: SequenceNumber, bitmap: u64) -> Self {
        Self {
            control,
            block_ack: TxBlockAckBitmap::new(starting_sequence, bitmap),
        }
    }
}

impl TxBlockAckBitmap {
    #[inline(always)]
    pub const fn new(starting_sequence: SequenceNumber, bitmap: u64) -> Self {
        Self {
            starting_sequence,
            bitmap,
        }
    }

    /// Return whether the transmitter must consider `sequence` complete.
    ///
    /// Peers use both standard-compliant SSN conventions: some retain the
    /// oldest possible sequence and describe it with the bitmap, while others
    /// advance SSN to the first not-yet-acknowledged sequence. In the latter
    /// form an MPDU immediately left of the new window is already complete
    /// even though it no longer has a bitmap bit. Only a bounded predecessor
    /// is admitted; a sequence beyond either side of the 64-entry BA window
    /// remains unacknowledged so a stale result cannot release new traffic.
    pub const fn acknowledges(self, sequence: SequenceNumber) -> bool {
        self.report().acknowledges(sequence)
    }

    /// The portable BlockAck of the lower-MAC port.
    pub const fn report(self) -> BlockAckReport {
        BlockAckReport {
            start_sequence: self.starting_sequence,
            bitmap: self.bitmap,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxAmpduDisposition {
    Acknowledged,
    Retry,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TxAmpduCompletion {
    pub mpdu: TxAmpduMpdu,
    pub disposition: TxAmpduDisposition,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxAmpduBatchError {
    Empty,
    InvalidWindow(u8),
    InvalidSlot(u8),
    DuplicateSlot(u8),
    DuplicateSequence(SequenceNumber),
    Full,
}

/// A batch with no outstanding frame ownership.
#[derive(Debug)]
pub struct BatchIdle;

/// A batch whose frame list is exclusively owned by its builder.
#[derive(Debug)]
pub struct BatchBuilding;

/// A batch draining a single hardware completion observation.
#[derive(Debug)]
pub struct BatchCompleting(Option<TxBlockAckBitmap>);

/// One fixed TX A-MPDU batch owned by the Rust radio task.
///
/// `begin` consumes the idle owner. Only a building owner can append frames
/// or accept completion; only the last completion returns an idle owner.
/// Dropping a phase loses its authority rather than allowing a second begin.
/// Each completion step returns exactly one frame without a drain loop.
///
/// ```compile_fail,E0382
/// use oer_esp32s31_ieee80211_mac::tx::ampdu::TxAmpduBatch;
/// use oer_ieee80211_mac::sequence::SequenceNumber;
/// let idle = TxAmpduBatch::new();
/// let building = idle.begin(SequenceNumber::ZERO, 2);
/// let duplicate = idle.begin(SequenceNumber::ZERO, 2);
/// ```
#[derive(Debug)]
pub struct TxAmpduBatch<Phase = BatchIdle> {
    entries: [Option<TxAmpduMpdu>; TX_AMPDU_SLOT_CAPACITY],
    phase: Phase,
    starting_sequence: SequenceNumber,
    window: u8,
    count: u8,
    completion_index: u8,
    slot_mask: u32,
}

impl TxAmpduBatch {
    pub const fn new() -> Self {
        Self {
            entries: [None; TX_AMPDU_SLOT_CAPACITY],
            phase: BatchIdle,
            starting_sequence: SequenceNumber::ZERO,
            window: 0,
            count: 0,
            completion_index: 0,
            slot_mask: 0,
        }
    }

    pub fn begin(
        mut self,
        starting_sequence: SequenceNumber,
        window: u8,
    ) -> Result<TxAmpduBatch<BatchBuilding>, (Self, TxAmpduBatchError)> {
        if window == 0 || usize::from(window) > TX_AMPDU_SLOT_CAPACITY {
            return Err((self, TxAmpduBatchError::InvalidWindow(window)));
        }
        self.starting_sequence = starting_sequence;
        self.window = window;
        Ok(self.with_phase(BatchBuilding))
    }
}

impl TxAmpduBatch<BatchBuilding> {
    /// Append one statically owned frame with a consecutive QoS sequence.
    pub fn push(&mut self, slot: u8) -> Result<TxAmpduMpdu, TxAmpduBatchError> {
        let sequence = self.starting_sequence.wrapping_add(u16::from(self.count));
        self.push_sequence(slot, sequence)
    }

    /// Preserve a prepared MPDU's exact sequence, including retry holes.
    pub fn push_sequence(
        &mut self,
        slot: u8,
        sequence: SequenceNumber,
    ) -> Result<TxAmpduMpdu, TxAmpduBatchError> {
        let slot = TxAmpduSlot::new(slot).ok_or(TxAmpduBatchError::InvalidSlot(slot))?;
        let slot_bit = 1_u32 << slot.index();
        if self.slot_mask & slot_bit != 0 {
            return Err(TxAmpduBatchError::DuplicateSlot(slot.index()));
        }
        if self.count >= self.window {
            return Err(TxAmpduBatchError::Full);
        }
        for entry in &self.entries[..usize::from(self.count)] {
            if entry.is_some_and(|entry| entry.sequence == sequence) {
                return Err(TxAmpduBatchError::DuplicateSequence(sequence));
            }
        }
        let mpdu = TxAmpduMpdu { slot, sequence };
        self.entries[usize::from(self.count)] = Some(mpdu);
        self.count += 1;
        self.slot_mask |= slot_bit;
        Ok(mpdu)
    }

    pub fn complete_with_block_ack(
        self,
        block_ack: TxBlockAckBitmap,
    ) -> Result<TxAmpduBatch<BatchCompleting>, (Self, TxAmpduBatchError)> {
        self.begin_completion(Some(block_ack))
    }

    /// On a timeout/error edge every MPDU returns as `Retry`.
    pub fn complete_without_block_ack(
        self,
    ) -> Result<TxAmpduBatch<BatchCompleting>, (Self, TxAmpduBatchError)> {
        self.begin_completion(None)
    }

    fn begin_completion(
        self,
        block_ack: Option<TxBlockAckBitmap>,
    ) -> Result<TxAmpduBatch<BatchCompleting>, (Self, TxAmpduBatchError)> {
        if self.count == 0 {
            return Err((self, TxAmpduBatchError::Empty));
        }
        Ok(self.with_phase(BatchCompleting(block_ack)))
    }
}

/// Returning the last MPDU returns the unique permission to begin again.
#[derive(Debug)]
#[must_use = "retain the remaining batch or the returned idle owner"]
pub enum TxAmpduBatchStep {
    More {
        completion: TxAmpduCompletion,
        batch: TxAmpduBatch<BatchCompleting>,
    },
    Last {
        completion: TxAmpduCompletion,
        batch: TxAmpduBatch,
    },
}

impl TxAmpduBatch<BatchCompleting> {
    pub fn next_completion(mut self) -> TxAmpduBatchStep {
        let index = usize::from(self.completion_index);
        let mpdu = self.entries[index]
            .take()
            .expect("completing batch retains every submitted MPDU");
        self.completion_index += 1;
        self.slot_mask &= !(1_u32 << mpdu.slot.index());
        let disposition = if self
            .phase
            .0
            .is_some_and(|ack| ack.acknowledges(mpdu.sequence))
        {
            TxAmpduDisposition::Acknowledged
        } else {
            TxAmpduDisposition::Retry
        };
        let completion = TxAmpduCompletion { mpdu, disposition };
        if self.completion_index == self.count {
            self.window = 0;
            self.count = 0;
            self.completion_index = 0;
            TxAmpduBatchStep::Last {
                completion,
                batch: self.with_phase(BatchIdle),
            }
        } else {
            TxAmpduBatchStep::More {
                completion,
                batch: self,
            }
        }
    }
}

impl<Phase> TxAmpduBatch<Phase> {
    pub const fn len(&self) -> usize {
        self.count as usize
    }
    pub const fn is_empty(&self) -> bool {
        self.count == 0
    }

    fn with_phase<Next>(self, phase: Next) -> TxAmpduBatch<Next> {
        TxAmpduBatch {
            entries: self.entries,
            phase,
            starting_sequence: self.starting_sequence,
            window: self.window,
            count: self.count,
            completion_index: self.completion_index,
            slot_mask: self.slot_mask,
        }
    }
}

impl Default for TxAmpduBatch {
    fn default() -> Self {
        Self::new()
    }
}
