//! Side-effect-free A-MPDU slot and backing admission checks.

use super::{
    HtAmpduLengthError, HtAmpduTxError, HtAmpduTxStorage, TX_AMPDU_METADATA_SIZE, TX_FCS_SIZE,
};
use crate::mac::tx::{TxCookie, TxDmaState};

impl<const SLOTS: usize> HtAmpduTxStorage<SLOTS> {
    /// Check a frame against a separately owned DMA allocation.
    ///
    /// The allocation capacity is supplied by the pinned lease; the
    /// aggregate owner holds descriptors only.
    pub(super) fn can_commit_referenced_frame(
        &self,
        cookie: TxCookie,
        frame_length: usize,
        hardware_mic_length: u8,
        dma_capacity: usize,
    ) -> Result<bool, HtAmpduTxError> {
        self.can_commit_frame_in_capacity(
            cookie,
            frame_length,
            hardware_mic_length,
            false,
            dma_capacity,
        )
    }

    fn can_commit_frame_in_capacity(
        &self,
        cookie: TxCookie,
        frame_length: usize,
        hardware_mic_length: u8,
        hardware_he_control: bool,
        buffer_capacity: usize,
    ) -> Result<bool, HtAmpduTxError> {
        if self.state != TxDmaState::Reserved || self.active != cookie {
            return Err(HtAmpduTxError::Stale);
        }
        if usize::from(self.count) >= SLOTS {
            return Ok(false);
        }
        let Some(psdu_length) = frame_length
            .checked_add(usize::from(hardware_mic_length))
            .and_then(|length| length.checked_add(usize::from(TX_FCS_SIZE)))
            .and_then(|length| u16::try_from(length).ok())
            .filter(|length| *length != 0 && *length <= 0x3fff)
        else {
            return Err(HtAmpduTxError::FrameTooLong);
        };
        let Some(descriptor_capacity) = TX_AMPDU_METADATA_SIZE
            .checked_add(usize::from(psdu_length))
            .and_then(|length| length.checked_add(3))
            .map(|length| length & !3)
        else {
            return Err(HtAmpduTxError::FrameTooLong);
        };
        if descriptor_capacity > buffer_capacity {
            return Err(HtAmpduTxError::FrameTooLong);
        }

        match self.length_after_append(psdu_length, hardware_he_control) {
            Ok(_) => Ok(true),
            Err(HtAmpduTxError::Length(HtAmpduLengthError::AggregateTooLong(_))) => Ok(false),
            Err(error) => Err(error),
        }
    }
}
