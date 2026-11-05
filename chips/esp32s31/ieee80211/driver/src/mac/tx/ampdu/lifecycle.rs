//! Post-publication A-MPDU completion, timeout and release transitions.

use core::pin::Pin;

use oer_chips_esp32s31_hal::types::{MacHeTriggerTxQueueSnapshot, MacTxAbortCause};
use oer_protocols_ieee80211_wire::sequence::SequenceNumber;

use super::{
    HtAmpduHardware, HtAmpduTxCompletion, HtAmpduTxError, HtAmpduTxStorage, HtBlockAckObservation,
};
use crate::mac::tx::{
    CompletionObservation, TxCompletion, TxCookie, TxDmaState, TxHardware, decode_or_quarantine,
};

impl<const SLOTS: usize> HtAmpduTxStorage<SLOTS> {
    /// Discard a software-owned partial batch.
    pub fn cancel(mut self: Pin<&mut Self>, cookie: TxCookie) -> Result<(), HtAmpduTxError> {
        let storage = self.as_ref().get_ref();
        if storage.state != TxDmaState::Reserved || storage.active != cookie {
            return Err(HtAmpduTxError::Stale);
        }
        self.as_mut().release();
        Ok(())
    }

    /// Sample BlockAck and transfer a completed aggregate back to software.
    pub fn acknowledge_completion<H: HtAmpduHardware>(
        self: Pin<&mut Self>,
        hardware: &mut H,
    ) -> Result<Option<CompletionObservation<HtAmpduTxCompletion>>, HtAmpduTxError> {
        let storage = self.project();
        let Some(registers) = hardware.take_block_ack_completion(storage.queue.hardware_index())
        else {
            return Ok(None);
        };
        if *storage.state != TxDmaState::HardwareOwned {
            *storage.state = TxDmaState::ResetRequired;
            return Err(HtAmpduTxError::Stale);
        }
        match decode_or_quarantine(*storage.active, registers.tx(), || {
            *storage.state = TxDmaState::ResetRequired;
        })
        .map_err(HtAmpduTxError::Completion)?
        {
            CompletionObservation::Observed(tx) => {
                if let Some(reservation) = storage.trigger_reservation.take() {
                    hardware.clear_he_trigger_based_queue(reservation);
                }
                *storage.state = TxDmaState::Completed;
                Ok(Some(CompletionObservation::Observed(HtAmpduTxCompletion {
                    tx,
                    block_ack: HtBlockAckObservation::new(
                        registers.block_ack_control(),
                        SequenceNumber::from_low_bits(registers.block_ack_starting_sequence()),
                        registers.block_ack_bitmap(),
                    ),
                    block_ack_received: registers.block_ack_received(),
                })))
            }
            CompletionObservation::NoEffect => Ok(Some(CompletionObservation::NoEffect)),
        }
    }

    /// Return the Trigger queue readback captured at its publication edge.
    ///
    /// The snapshot is sampled after the complete queue/BSR transaction and
    /// before the HE TX doorbell. Hardware may consume the BSR value and
    /// validity immediately after that doorbell, so a later live read is not
    /// an equivalent publication check.
    pub fn he_trigger_based_snapshot<H: HtAmpduHardware>(
        self: Pin<&Self>,
        _hardware: &H,
        cookie: TxCookie,
    ) -> Result<Option<MacHeTriggerTxQueueSnapshot>, HtAmpduTxError> {
        let storage = self.get_ref();
        if storage.state != TxDmaState::HardwareOwned || storage.active != cookie {
            return Err(HtAmpduTxError::Stale);
        }
        Ok(storage.trigger_publication_snapshot)
    }

    /// Transfer one single-MPDU completion back to software: a legacy, HT
    /// or HE S-MPDU publication of a batch's first frame.
    ///
    /// Unlike A-MPDU, this path intentionally does not sample BlockAck words.
    /// Its success criterion is the ordinary per-queue ACK status, matching
    /// the vendor raw-TX completion callback.
    pub fn acknowledge_single_completion<H: TxHardware>(
        self: Pin<&mut Self>,
        hardware: &mut H,
    ) -> Result<Option<CompletionObservation<TxCompletion>>, HtAmpduTxError> {
        let storage = self.project();
        let Some(registers) = hardware.take_tx_completion(storage.queue.hardware_index()) else {
            return Ok(None);
        };
        if *storage.state != TxDmaState::HardwareOwned || *storage.count == 0 {
            *storage.state = TxDmaState::ResetRequired;
            return Err(HtAmpduTxError::Stale);
        }
        match decode_or_quarantine(*storage.active, registers, || {
            *storage.state = TxDmaState::ResetRequired;
        })
        .map_err(HtAmpduTxError::Completion)?
        {
            CompletionObservation::Observed(completion) => {
                *storage.state = TxDmaState::Completed;
                Ok(Some(CompletionObservation::Observed(completion)))
            }
            CompletionObservation::NoEffect => Ok(Some(CompletionObservation::NoEffect)),
        }
    }

    /// Hand a batch the queue left without a completion (a collision or a
    /// settled timeout abort) to software as a detached one: its frames stay
    /// for a retention or a release.
    pub(super) fn mark_aborted_detached(
        self: Pin<&mut Self>,
        cookie: TxCookie,
    ) -> Result<(), HtAmpduTxError> {
        let storage = self.project();
        if *storage.state != TxDmaState::HardwareOwned || *storage.active != cookie {
            return Err(HtAmpduTxError::Stale);
        }
        *storage.trigger_reservation = None;
        *storage.state = TxDmaState::Completed;
        *storage.detached = true;
        Ok(())
    }

    /// Record a single-MPDU publication of the batch's first frame on
    /// `queue`; the frames behind it stay unpublished.
    pub(super) fn record_single_publication(
        self: Pin<&mut Self>,
        cookie: TxCookie,
        queue: crate::mac::tx::LegacyTxQueue,
    ) -> Result<(), HtAmpduTxError> {
        let storage = self.project();
        if *storage.state != TxDmaState::Reserved || *storage.active != cookie {
            return Err(HtAmpduTxError::Stale);
        }
        *storage.queue = queue;
        *storage.aggregate_length = storage.psdu_lengths.first().copied().unwrap_or(0);
        *storage.detached = false;
        *storage.state = TxDmaState::HardwareOwned;
        Ok(())
    }

    /// Set each frame's empty delimiters to `delimiters(psdu)` (zero for
    /// HT), handing each changed one to `write` with its position before it
    /// is recorded, and return the batch's length.
    pub(super) fn set_empty_delimiters(
        mut self: Pin<&mut Self>,
        cookie: TxCookie,
        delimiters: impl Fn(u16) -> Option<u8>,
        mut write: impl FnMut(usize, u8) -> Result<(), HtAmpduTxError>,
    ) -> Result<super::HtAmpduLength, HtAmpduTxError> {
        let storage = self.as_ref().get_ref();
        if storage.state != TxDmaState::Reserved || storage.active != cookie {
            return Err(HtAmpduTxError::Stale);
        }
        let count = usize::from(storage.count);
        let mut changed = false;
        let storage = self.as_mut().project();
        for (index, (psdu, current)) in storage
            .psdu_lengths
            .iter()
            .zip(storage.empty_delimiters.iter_mut())
            .take(count)
            .enumerate()
        {
            let wanted = delimiters(*psdu).ok_or(HtAmpduTxError::FrameTooLong)?;
            if wanted != *current {
                write(index, wanted)?;
                *current = wanted;
                changed = true;
            }
        }
        if changed {
            self.as_mut().recalculate_prepared_length()?;
        }
        self.as_ref().get_ref().calculate_aggregate()
    }

    /// The MPDU length and hardware MIC length of the frame at `index`.
    pub(super) fn frame_size(&self, index: usize) -> Option<(u16, u8)> {
        (index < usize::from(self.count))
            .then(|| self.frame_lengths.get(index).copied())
            .flatten()
            .zip(self.hardware_mic_lengths.get(index).copied())
    }

    pub fn begin_timeout_abort<H: HtAmpduHardware>(
        self: Pin<&mut Self>,
        hardware: &mut H,
        cookie: TxCookie,
        cause: MacTxAbortCause,
    ) -> Result<bool, HtAmpduTxError> {
        let storage = self.project();
        if *storage.state != TxDmaState::HardwareOwned || *storage.active != cookie {
            return Err(HtAmpduTxError::Stale);
        }
        Ok(hardware.begin_tx_timeout_abort(storage.queue.hardware_index(), cause))
    }

    /// Permanently quarantine an aggregate an abort left on the air, whose
    /// completion did not come.
    ///
    /// The referenced network leases must remain retained until the unique
    /// radio lifecycle owner resets the MAC; returning them would make
    /// potentially DMA-visible allocations writable by `embassy-net`.
    pub fn require_reset(self: Pin<&mut Self>, cookie: TxCookie) -> Result<(), HtAmpduTxError> {
        let storage = self.project();
        if *storage.state != TxDmaState::HardwareOwned || *storage.active != cookie {
            return Err(HtAmpduTxError::Stale);
        }
        *storage.state = TxDmaState::ResetRequired;
        Ok(())
    }

    /// Release a detached completed batch after BlockAck processing and any
    /// individual retries have copied the retained MPDUs.
    pub fn release_completed(
        mut self: Pin<&mut Self>,
        cookie: TxCookie,
    ) -> Result<(), HtAmpduTxError> {
        let storage = self.as_ref().get_ref();
        if storage.state != TxDmaState::Completed || storage.active != cookie || !storage.detached {
            return Err(HtAmpduTxError::Stale);
        }
        self.as_mut().release();
        Ok(())
    }

    pub(super) fn release(self: Pin<&mut Self>) {
        let storage = self.project();
        *storage.active = TxCookie(0);
        *storage.count = 0;
        *storage.prepared_length = 0;
        *storage.aggregate_length = 0;
        *storage.trigger_reservation = None;
        *storage.trigger_publication_snapshot = None;
        *storage.detached = false;
        *storage.state = TxDmaState::Free;
    }
}
