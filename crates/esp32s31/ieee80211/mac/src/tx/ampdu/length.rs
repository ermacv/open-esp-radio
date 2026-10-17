//! Allocation-free A-MPDU byte accounting recovered from the PP formatter.

use core::pin::Pin;

use super::{HtAmpduTxError, HtAmpduTxStorage, block_ack::TX_AMPDU_SLOT_CAPACITY};

pub(super) const HARDWARE_HE_CONTROL_LENGTH: u16 = 4;
const HT_MPDU_LENGTH_MASK: u32 = 0x3fff;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HtAmpduLengthError {
    InvalidLimits,
    Empty,
    ZeroMpduLength,
    WindowFull,
    AggregateTooLong(u32),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HtAmpduLength {
    pub bytes: u16,
    pub subframes: u8,
}

/// Exact basic-HT A-MPDU byte accounting recovered from the pinned PP blob.
///
/// Bits 0..13 of `payload_word` carry the MPDU length. `empty_delimiters` is
/// the byte immediately following that word in the PP metadata prefix. Every
/// non-final MPDU contributes its four-byte delimiter, its length rounded to
/// four bytes, and the requested empty delimiters. `finish` removes the final
/// padding and empty delimiters, leaving the last MPDU's mandatory delimiter.
///
/// The accumulator has a caller-selected fixed window and byte ceiling. It
/// never allocates, accesses pointers, reads time, retries, waits or invokes a
/// callback.
pub struct HtAmpduLengthAccumulator {
    bytes_with_tail: u32,
    tail_bytes: u16,
    count: u8,
    max_subframes: u8,
    max_bytes: u16,
}

impl HtAmpduLengthAccumulator {
    /// Intersect a prospective prefix with an additional byte ceiling.
    /// Zero closes an empty budget. An already admitted prefix cannot be
    /// shortened: refusal leaves both the prefix and its limits unchanged.
    pub fn cap_bytes(&mut self, maximum_bytes: u16) -> Result<(), HtAmpduLengthError> {
        let maximum_bytes = self.max_bytes.min(maximum_bytes);
        let bytes = self.bytes_with_tail - u32::from(self.tail_bytes);
        if bytes > u32::from(maximum_bytes) {
            return Err(HtAmpduLengthError::AggregateTooLong(bytes));
        }
        self.max_bytes = maximum_bytes;
        Ok(())
    }

    pub const fn new(max_subframes: u8, max_bytes: u16) -> Result<Self, HtAmpduLengthError> {
        if max_subframes == 0 || max_subframes as usize > TX_AMPDU_SLOT_CAPACITY || max_bytes == 0 {
            return Err(HtAmpduLengthError::InvalidLimits);
        }
        Ok(Self {
            bytes_with_tail: 0,
            tail_bytes: 0,
            count: 0,
            max_subframes,
            max_bytes,
        })
    }

    pub const fn push(
        &mut self,
        payload_word: u32,
        empty_delimiters: u8,
    ) -> Result<(), HtAmpduLengthError> {
        self.push_with_hardware_he_control(payload_word, empty_delimiters, false)
    }

    /// Add one MPDU whose HE-Control field may be inserted by MAC hardware.
    ///
    /// SOURCE: complete `libpp[pp_he.o]::ppCalSubFrameLength` reads `metadata[7] & 1`, multiplies it by four,
    /// and adds it after the delimiter and rounded metadata length. The low
    /// fourteen-bit MPDU length remains unchanged.
    pub const fn push_with_hardware_he_control(
        &mut self,
        payload_word: u32,
        empty_delimiters: u8,
        hardware_he_control: bool,
    ) -> Result<(), HtAmpduLengthError> {
        if self.count >= self.max_subframes {
            return Err(HtAmpduLengthError::WindowFull);
        }
        let mpdu_bytes = payload_word & HT_MPDU_LENGTH_MASK;
        if mpdu_bytes == 0 {
            return Err(HtAmpduLengthError::ZeroMpduLength);
        }
        let padding = (4 - (mpdu_bytes & 3)) & 3;
        let empty_bytes = (empty_delimiters as u32) * 4;
        let inserted_bytes = if hardware_he_control {
            HARDWARE_HE_CONTROL_LENGTH as u32
        } else {
            0
        };
        let contribution = mpdu_bytes + padding + empty_bytes + 4 + inserted_bytes;
        let next = self.bytes_with_tail + contribution;
        let final_bytes = next - padding - empty_bytes;
        if final_bytes > self.max_bytes as u32 {
            return Err(HtAmpduLengthError::AggregateTooLong(final_bytes));
        }
        self.bytes_with_tail = next;
        self.tail_bytes = (padding + empty_bytes) as u16;
        self.count += 1;
        Ok(())
    }

    pub const fn finish(&self) -> Result<HtAmpduLength, HtAmpduLengthError> {
        if self.count == 0 {
            return Err(HtAmpduLengthError::Empty);
        }
        let bytes = self.bytes_with_tail - self.tail_bytes as u32;
        if bytes > self.max_bytes as u32 {
            return Err(HtAmpduLengthError::AggregateTooLong(bytes));
        }
        Ok(HtAmpduLength {
            bytes: bytes as u16,
            subframes: self.count,
        })
    }
}

impl<const SLOTS: usize, const BUFFER_SIZE: usize> HtAmpduTxStorage<SLOTS, BUFFER_SIZE> {
    /// Snapshot the software-owned prefix for prospective HT burst admission.
    /// Appending to this value consumes no descriptor, sequence number or PN.
    /// It checks length only; publication still validates backing and ownership.
    /// Discard the snapshot after changing the real prefix or its limits.
    pub fn ht_length_budget(
        &self,
        rate: crate::tx::HtRate,
    ) -> Result<HtAmpduLengthAccumulator, HtAmpduTxError> {
        let max_bytes = rate
            .vendor_ampdu_byte_limit()
            .map_or(self.max_aggregate_bytes, |limit| {
                limit.min(self.max_aggregate_bytes)
            });
        let max_subframes = u8::try_from(SLOTS)
            .map_err(|_| HtAmpduTxError::Length(HtAmpduLengthError::InvalidLimits))?;
        self.length_budget(max_subframes, max_bytes)
    }

    /// The HE counterpart of [`Self::ht_length_budget`]: the aggregate is
    /// bounded by the configured ceiling, by the A-PSDU the rate carries
    /// within the policy's TXOP limit, and by the 32 subframes an HE
    /// aggregate publication holds.
    pub fn he_length_budget(
        &self,
        policy: super::HeAmpduPolicy,
    ) -> Result<HtAmpduLengthAccumulator, HtAmpduTxError> {
        let apep = policy
            .rate()
            .checked_maximum_apep_bytes(policy.txop_limit())
            .ok_or(HtAmpduTxError::Length(HtAmpduLengthError::InvalidLimits))?;
        let max_bytes = u16::try_from(apep)
            .unwrap_or(u16::MAX)
            .min(self.max_aggregate_bytes);
        let max_subframes = u8::try_from(SLOTS)
            .map_err(|_| HtAmpduTxError::Length(HtAmpduLengthError::InvalidLimits))?
            .min(32);
        self.length_budget(max_subframes, max_bytes)
    }

    fn length_budget(
        &self,
        max_subframes: u8,
        max_bytes: u16,
    ) -> Result<HtAmpduLengthAccumulator, HtAmpduTxError> {
        if !matches!(
            self.state,
            crate::tx::TxSlotState::Free | crate::tx::TxSlotState::Reserved
        ) {
            return Err(HtAmpduTxError::Stale);
        }
        let mut budget = HtAmpduLengthAccumulator::new(max_subframes, max_bytes)
            .map_err(HtAmpduTxError::Length)?;
        if self.count != 0 {
            let last = usize::from(self.count - 1);
            let tail = (4 - (self.psdu_lengths[last] & 3)) & 3;
            budget.tail_bytes = tail + u16::from(self.empty_delimiters[last]) * 4;
            budget.bytes_with_tail = u32::from(self.prepared_length) + u32::from(budget.tail_bytes);
            budget.count = self.count;
        }
        Ok(budget)
    }

    /// Return the final A-MPDU length after appending one PSDU.
    ///
    /// The previous final MPDU gains its four-byte alignment and requested
    /// empty delimiters only when another MPDU follows it. The new final MPDU
    /// contributes one delimiter and its exact PSDU length. Keeping this
    /// prefix total makes each append O(1); the former validation replayed all
    /// preceding lengths on every `commit_frame`, making a 32-MPDU build
    /// O(n²).
    ///
    /// SOURCE: complete `libpp[pp.o]::ppCalTxAMPDULength`, complete
    /// `libpp[pp_he.o]::ppCalSubFrameLength`, and the equivalent finite rules in
    /// [`HtAmpduLengthAccumulator`].
    pub(super) fn length_after_append(
        &self,
        psdu_length: u16,
        hardware_he_control: bool,
    ) -> Result<u16, HtAmpduTxError> {
        if psdu_length == 0 {
            return Err(HtAmpduTxError::Length(HtAmpduLengthError::ZeroMpduLength));
        }
        let previous = if self.count != 0 {
            let previous = usize::from(self.count - 1);
            Some((
                u32::from(self.psdu_lengths[previous]),
                u32::from(self.empty_delimiters[previous]),
            ))
        } else {
            None
        };
        let next_psdu = u32::from(psdu_length)
            + if hardware_he_control {
                u32::from(HARDWARE_HE_CONTROL_LENGTH)
            } else {
                0
            };
        let next = oer_ieee80211_mac::ampdu::length_after_append(
            u32::from(self.prepared_length),
            previous,
            next_psdu,
        )
        .ok_or(HtAmpduTxError::Length(
            HtAmpduLengthError::AggregateTooLong(u32::MAX),
        ))?;
        if next > u32::from(self.max_aggregate_bytes) {
            return Err(HtAmpduTxError::Length(
                HtAmpduLengthError::AggregateTooLong(next),
            ));
        }
        u16::try_from(next)
            .map_err(|_| HtAmpduTxError::Length(HtAmpduLengthError::AggregateTooLong(next)))
    }

    pub(super) fn recalculate_prepared_length(
        self: Pin<&mut Self>,
    ) -> Result<HtAmpduLength, HtAmpduTxError> {
        let storage = self.as_ref().get_ref();
        let mut length = HtAmpduLengthAccumulator::new(storage.count, storage.max_aggregate_bytes)
            .map_err(HtAmpduTxError::Length)?;
        for index in 0..usize::from(storage.count) {
            length
                .push_with_hardware_he_control(
                    u32::from(storage.psdu_lengths[index]),
                    storage.empty_delimiters[index],
                    storage.hardware_he_control[index],
                )
                .map_err(HtAmpduTxError::Length)?;
        }
        let aggregate = length.finish().map_err(HtAmpduTxError::Length)?;
        *self.project().prepared_length = aggregate.bytes;
        Ok(aggregate)
    }

    pub(super) fn calculate_aggregate(&self) -> Result<HtAmpduLength, HtAmpduTxError> {
        if self.count == 0 {
            return Err(HtAmpduTxError::Length(HtAmpduLengthError::Empty));
        }
        if self.prepared_length == 0 {
            return Err(HtAmpduTxError::Length(HtAmpduLengthError::ZeroMpduLength));
        }
        Ok(HtAmpduLength {
            bytes: self.prepared_length,
            subframes: self.count,
        })
    }
}
