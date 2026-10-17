//! Post-publication A-MPDU completion, timeout and release transitions.

use core::pin::Pin;

use oer_esp32s31_hal::types::MacHeTriggerTxQueueSnapshot;
use oer_ieee80211_mac::sequence::SequenceNumber;

use super::{
    HtAmpduHardware, HtAmpduTxCompletion, HtAmpduTxError, HtAmpduTxStorage, HtBlockAckObservation,
};
use crate::tx::{
    CompletionObservation, TxCompletion, TxCookie, TxHardware, TxSlotState, decode_or_quarantine,
};

impl<const SLOTS: usize, const BUFFER_SIZE: usize> HtAmpduTxStorage<SLOTS, BUFFER_SIZE> {
    /// Discard a software-owned partial batch.
    pub fn cancel(mut self: Pin<&mut Self>, cookie: TxCookie) -> Result<(), HtAmpduTxError> {
        let storage = self.as_ref().get_ref();
        if storage.state != TxSlotState::Reserved || storage.active != cookie {
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
        let Some(registers) = hardware.take_block_ack_completion(storage.queue.index()) else {
            return Ok(None);
        };
        if *storage.state != TxSlotState::HardwareOwned {
            *storage.state = TxSlotState::ResetRequired;
            return Err(HtAmpduTxError::Stale);
        }
        match decode_or_quarantine(*storage.active, registers.tx(), || {
            *storage.state = TxSlotState::ResetRequired;
        })
        .map_err(HtAmpduTxError::Completion)?
        {
            CompletionObservation::Observed(tx) => {
                if let Some(reservation) = storage.trigger_reservation.take() {
                    hardware.clear_he_trigger_based_queue(reservation);
                }
                *storage.state = TxSlotState::Completed;
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
        if storage.state != TxSlotState::HardwareOwned || storage.active != cookie {
            return Err(HtAmpduTxError::Stale);
        }
        Ok(storage.trigger_publication_snapshot)
    }

    /// Transfer one HE S-MPDU ACK completion back to software.
    ///
    /// Unlike A-MPDU, this path intentionally does not sample BlockAck words.
    /// Its success criterion is the ordinary per-queue ACK status, matching
    /// the vendor raw-TX completion callback.
    pub fn acknowledge_he_smpdu_completion<H: TxHardware>(
        self: Pin<&mut Self>,
        hardware: &mut H,
    ) -> Result<Option<CompletionObservation<TxCompletion>>, HtAmpduTxError> {
        let storage = self.project();
        let Some(registers) = hardware.take_tx_completion(storage.queue.index()) else {
            return Ok(None);
        };
        if *storage.state != TxSlotState::HardwareOwned || *storage.count != 1 {
            *storage.state = TxSlotState::ResetRequired;
            return Err(HtAmpduTxError::Stale);
        }
        match decode_or_quarantine(*storage.active, registers, || {
            *storage.state = TxSlotState::ResetRequired;
        })
        .map_err(HtAmpduTxError::Completion)?
        {
            CompletionObservation::Observed(completion) => {
                *storage.state = TxSlotState::Completed;
                Ok(Some(CompletionObservation::Observed(completion)))
            }
            CompletionObservation::NoEffect => Ok(Some(CompletionObservation::NoEffect)),
        }
    }

    pub fn begin_timeout_abort<H: HtAmpduHardware>(
        self: Pin<&mut Self>,
        hardware: &mut H,
        cookie: TxCookie,
    ) -> Result<bool, HtAmpduTxError> {
        let storage = self.project();
        if *storage.state != TxSlotState::HardwareOwned || *storage.active != cookie {
            return Err(HtAmpduTxError::Stale);
        }
        Ok(hardware.begin_tx_timeout_abort(storage.queue.index()))
    }

    /// Permanently quarantine an aggregate whose executor deadline expired
    /// without a qualified hardware completion/timeout edge.
    ///
    /// The referenced network leases must remain retained until the unique
    /// radio lifecycle owner resets the MAC; returning them would make
    /// potentially DMA-visible allocations writable by `embassy-net`.
    pub fn require_reset(self: Pin<&mut Self>, cookie: TxCookie) -> Result<(), HtAmpduTxError> {
        let storage = self.project();
        if *storage.state != TxSlotState::HardwareOwned || *storage.active != cookie {
            return Err(HtAmpduTxError::Stale);
        }
        *storage.state = TxSlotState::ResetRequired;
        Ok(())
    }

    /// Release a detached completed batch after BlockAck processing and any
    /// individual retries have copied the retained MPDUs.
    pub fn release_completed(
        mut self: Pin<&mut Self>,
        cookie: TxCookie,
    ) -> Result<(), HtAmpduTxError> {
        let storage = self.as_ref().get_ref();
        if storage.state != TxSlotState::Completed || storage.active != cookie || !storage.detached
        {
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
        *storage.state = TxSlotState::Free;
    }
}
