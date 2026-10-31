//! Affine completion of a bounded 32-slot MPDU batch.
use crate::sequence::SequenceSpace;
use oer_ieee80211_contracts_port::BlockAckReport;
use oer_ieee80211_wire::sequence::SequenceNumber;
const TX_AMPDU_SLOT_CAPACITY: usize = 32;
/// Opaque index of one statically owned TX frame.
///
/// The bounded transmit pool has exactly 32 fixed TX slots. Keeping only their
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
    /// A completing owner has no MPDU at its next submitted position.
    MissingCompletion {
        index: u8,
        count: u8,
    },
}

/// A batch with no outstanding frame ownership.
#[derive(Debug)]
pub struct BatchIdle;

/// A batch whose frame list is exclusively owned by its builder.
#[derive(Debug)]
pub struct BatchBuilding;

/// A batch draining a single hardware completion observation.
#[derive(Debug)]
pub struct BatchCompleting(Option<BlockAckReport>);

/// One fixed TX A-MPDU batch owned by the Rust radio task.
///
/// `begin` consumes the idle owner. Only a building owner can append frames
/// or accept completion; only the last completion returns an idle owner.
/// Dropping a phase loses its authority rather than allowing a second begin.
/// Each completion step returns exactly one frame without a drain loop.
///
/// ```compile_fail,E0382
/// use oer_ieee80211_core_upper_mac::block_ack::batch::TxAmpduBatch;
/// use oer_ieee80211_wire::sequence::SequenceNumber;
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
        for entry in self.entries.iter().take(usize::from(self.count)) {
            if entry.is_some_and(|entry| entry.sequence == sequence) {
                return Err(TxAmpduBatchError::DuplicateSequence(sequence));
            }
        }
        let mpdu = TxAmpduMpdu { slot, sequence };
        let entry = self
            .entries
            .get_mut(usize::from(self.count))
            .ok_or(TxAmpduBatchError::Full)?;
        // The entry proves count < 32, so the increment cannot wrap.
        let count = self.count.wrapping_add(1);
        *entry = Some(mpdu);
        self.count = count;
        self.slot_mask |= slot_bit;
        Ok(mpdu)
    }

    pub fn complete_with_block_ack(
        self,
        block_ack: BlockAckReport,
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
        block_ack: Option<BlockAckReport>,
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
    /// Refuse a missing completion without releasing this batch's authority.
    pub fn next_completion(mut self) -> Result<TxAmpduBatchStep, (Self, TxAmpduBatchError)> {
        let index = self.completion_index;
        let Some(entry) = self.entries.get_mut(usize::from(index)) else {
            let count = self.count;
            return Err((self, TxAmpduBatchError::MissingCompletion { index, count }));
        };
        let Some(mpdu) = entry.take() else {
            let count = self.count;
            return Err((self, TxAmpduBatchError::MissingCompletion { index, count }));
        };
        // The array access proved index < 32, so advancing cannot wrap.
        self.completion_index = index.wrapping_add(1);
        self.slot_mask &= !(1_u32 << mpdu.slot.index());
        let disposition = if self
            .phase
            .0
            .is_some_and(|ack| crate::block_ack::acknowledges(ack, mpdu.sequence))
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
            Ok(TxAmpduBatchStep::Last {
                completion,
                batch: self.with_phase(BatchIdle),
            })
        } else {
            Ok(TxAmpduBatchStep::More {
                completion,
                batch: self,
            })
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

#[cfg(test)]
mod tests {
    use super::*;
    fn assert_batch_completions<const N: usize>(
        batch: TxAmpduBatch<BatchCompleting>,
        expected: [TxAmpduCompletion; N],
    ) -> TxAmpduBatch {
        let mut remaining = batch;
        let mut expected = expected.into_iter();
        loop {
            match remaining.next_completion().unwrap() {
                TxAmpduBatchStep::More { completion, batch } => {
                    assert_eq!(Some(completion), expected.next());
                    assert!(expected.len() > 0);
                    remaining = batch;
                }
                TxAmpduBatchStep::Last { completion, batch } => {
                    assert_eq!(Some(completion), expected.next());
                    assert_eq!(expected.next(), None);
                    assert!(batch.is_empty());
                    return batch;
                }
            }
        }
    }

    #[test]
    fn batch_returns_one_block_ack_result_per_step_and_idle_authority_last() {
        let mut batch = TxAmpduBatch::new()
            .begin(SequenceNumber::new(0x0ffe).unwrap(), 4)
            .unwrap();
        for slot in 3..7 {
            batch.push(slot).unwrap();
        }
        let batch = batch
            .complete_with_block_ack(BlockAckReport::new(
                SequenceNumber::new(0x0ffe).unwrap(),
                0b1101,
            ))
            .unwrap();
        let idle = assert_batch_completions(
            batch,
            [
                (3, 0x0ffe, TxAmpduDisposition::Acknowledged),
                (4, 0x0fff, TxAmpduDisposition::Retry),
                (5, 0, TxAmpduDisposition::Acknowledged),
                (6, 1, TxAmpduDisposition::Acknowledged),
            ]
            .map(|(slot, sequence, disposition)| TxAmpduCompletion {
                mpdu: TxAmpduMpdu {
                    slot: TxAmpduSlot::new(slot).unwrap(),
                    sequence: SequenceNumber::new(sequence).unwrap(),
                },
                disposition,
            }),
        );
        let next = idle.begin(SequenceNumber::new(17).unwrap(), 1).unwrap();
        assert!(next.is_empty());
    }

    #[test]
    fn missing_block_ack_retries_every_mpdu_without_a_drain() {
        let mut batch = TxAmpduBatch::new()
            .begin(SequenceNumber::new(9).unwrap(), 2)
            .unwrap();
        let first = batch.push(0).unwrap();
        let last = batch.push(31).unwrap();
        let batch = batch.complete_without_block_ack().unwrap();
        assert_batch_completions(
            batch,
            [first, last].map(|mpdu| TxAmpduCompletion {
                mpdu,
                disposition: TxAmpduDisposition::Retry,
            }),
        );
    }

    #[test]
    fn batch_rejects_duplicate_static_slot_ownership() {
        let mut batch = TxAmpduBatch::new().begin(SequenceNumber::ZERO, 32).unwrap();
        batch.push(17).unwrap();
        assert_eq!(batch.push(17), Err(TxAmpduBatchError::DuplicateSlot(17)));
    }

    #[test]
    fn batch_preserves_nonconsecutive_hardware_sequences() {
        let mut batch = TxAmpduBatch::new()
            .begin(SequenceNumber::new(0x120).unwrap(), 4)
            .unwrap();
        let first = batch
            .push_sequence(3, SequenceNumber::new(0x120).unwrap())
            .unwrap();
        let last = batch
            .push_sequence(4, SequenceNumber::new(0x123).unwrap())
            .unwrap();
        assert_eq!(
            batch.push_sequence(5, last.sequence),
            Err(TxAmpduBatchError::DuplicateSequence(last.sequence))
        );
        let batch = batch
            .complete_with_block_ack(BlockAckReport::new(first.sequence, 0b1001))
            .unwrap();
        assert_batch_completions(
            batch,
            [first, last].map(|mpdu| TxAmpduCompletion {
                mpdu,
                disposition: TxAmpduDisposition::Acknowledged,
            }),
        );
    }

    #[test]
    fn batch_never_exceeds_negotiated_or_static_window() {
        let (idle, error) = TxAmpduBatch::new()
            .begin(SequenceNumber::ZERO, 0)
            .unwrap_err();
        assert_eq!(error, TxAmpduBatchError::InvalidWindow(0));
        let (idle, error) = idle.begin(SequenceNumber::ZERO, 33).unwrap_err();
        assert_eq!(error, TxAmpduBatchError::InvalidWindow(33));
        let batch = idle.begin(SequenceNumber::ZERO, 1).unwrap();
        let (mut batch, error) = batch.complete_without_block_ack().unwrap_err();
        assert_eq!(error, TxAmpduBatchError::Empty);
        batch.push(0).unwrap();
        assert_eq!(batch.push(1), Err(TxAmpduBatchError::Full));
    }

    #[test]
    fn missing_completion_returns_the_owner_and_all_remaining_frames() {
        let mut building = TxAmpduBatch::new().begin(SequenceNumber::ZERO, 2).unwrap();
        let first = building.push(3).unwrap();
        let second = building.push(7).unwrap();
        let original = building.complete_without_block_ack().unwrap();
        for index in [2, 32, u8::MAX] {
            let mut batch = TxAmpduBatch {
                entries: original.entries,
                phase: BatchCompleting(original.phase.0),
                starting_sequence: original.starting_sequence,
                window: original.window,
                count: original.count,
                completion_index: index,
                slot_mask: original.slot_mask,
            };
            let entries = batch.entries;
            let (returned, error) = batch.next_completion().unwrap_err();
            assert_eq!(
                error,
                TxAmpduBatchError::MissingCompletion { index, count: 2 }
            );
            assert_eq!(returned.entries, entries);
            assert_eq!(returned.completion_index, index);
            assert_eq!(returned.slot_mask, (1 << 3) | (1 << 7));
            batch = returned;
            batch.completion_index = 0;
            assert_batch_completions(
                batch,
                [first, second].map(|mpdu| TxAmpduCompletion {
                    mpdu,
                    disposition: TxAmpduDisposition::Retry,
                }),
            );
        }
    }

    #[test]
    fn an_out_of_storage_builder_count_is_refused_before_mutation() {
        let mut batch = TxAmpduBatch::new().begin(SequenceNumber::ZERO, 32).unwrap();
        batch.count = 32;
        batch.window = u8::MAX;
        let entries = batch.entries;
        assert_eq!(batch.push(0), Err(TxAmpduBatchError::Full));
        assert_eq!(batch.entries, entries);
        assert_eq!(batch.count, 32);
        assert_eq!(batch.slot_mask, 0);
    }
}
