//! Side-effect-free A-MPDU slot, backing, APEP and TXOP admission checks.

use super::{
    AmpduFrameSize, HeAmpduPolicy, HtAmpduLengthError, HtAmpduTxError, HtAmpduTxStorage,
    TX_AMPDU_METADATA_SIZE, TX_FCS_SIZE,
};
#[cfg(any(test, feature = "test-support"))]
use crate::tx::{HeEdcaTxopLimit, HeRate, HtAmpduDensity};
use crate::tx::{HtRate, TxCookie, TxSlotState};

impl<const SLOTS: usize, const BUFFER_SIZE: usize> HtAmpduTxStorage<SLOTS, BUFFER_SIZE> {
    /// Check whether one more encoded MPDU fits both its static slot and the
    /// peer-advertised aggregate-byte ceiling.
    ///
    /// This does not reserve a slot or mutate the batch. A scheduler can
    /// therefore stop at the byte limit without consuming a CCMP PN or
    /// Sequence Control value for a frame that belongs in the next PPDU.
    #[cfg(any(test, feature = "test-support"))]
    pub fn can_commit_frame(
        &self,
        cookie: TxCookie,
        frame_length: usize,
        hardware_mic_length: u8,
        _empty_delimiters: u8,
    ) -> Result<bool, HtAmpduTxError> {
        self.can_commit_frame_in_capacity(
            cookie,
            frame_length,
            hardware_mic_length,
            false,
            BUFFER_SIZE,
        )
    }

    /// Check a frame against a separately owned DMA allocation.
    ///
    /// A descriptor-only storage uses `BUFFER_SIZE == 0`; its external
    /// allocation capacity is supplied by the pinned lease instead of being
    /// charged twice to the aggregate owner.
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

    /// Check one HT MPDU against both negotiated storage and the exact
    /// rate-dependent `rx11NRate2AMPDULimit` ceiling.
    #[cfg(any(test, feature = "test-support"))]
    pub fn can_commit_ht_frame(
        &self,
        cookie: TxCookie,
        frame_length: usize,
        hardware_mic_length: u8,
        empty_delimiters: u8,
        rate: HtRate,
    ) -> Result<bool, HtAmpduTxError> {
        if !self.can_commit_frame(cookie, frame_length, hardware_mic_length, empty_delimiters)? {
            return Ok(false);
        }
        let psdu_length = frame_length
            .checked_add(usize::from(hardware_mic_length))
            .and_then(|length| length.checked_add(usize::from(TX_FCS_SIZE)))
            .and_then(|length| u16::try_from(length).ok())
            .filter(|length| *length != 0 && *length <= 0x3fff)
            .ok_or(HtAmpduTxError::FrameTooLong)?;
        match rate.vendor_ampdu_byte_limit() {
            Some(limit) => Ok(self.length_after_append(psdu_length, false)? <= limit),
            None => Ok(true),
        }
    }

    /// Descriptor-only counterpart of the host-side `can_commit_ht_frame` check.
    pub fn can_commit_referenced_ht_frame(
        &self,
        cookie: TxCookie,
        frame_length: usize,
        hardware_mic_length: u8,
        _empty_delimiters: u8,
        rate: HtRate,
        dma_capacity: usize,
    ) -> Result<bool, HtAmpduTxError> {
        if !self.can_commit_referenced_frame(
            cookie,
            frame_length,
            hardware_mic_length,
            dma_capacity,
        )? {
            return Ok(false);
        }
        let psdu_length = frame_length
            .checked_add(usize::from(hardware_mic_length))
            .and_then(|length| length.checked_add(usize::from(TX_FCS_SIZE)))
            .and_then(|length| u16::try_from(length).ok())
            .filter(|length| *length != 0 && *length <= 0x3fff)
            .ok_or(HtAmpduTxError::FrameTooLong)?;
        match rate.vendor_ampdu_byte_limit() {
            Some(limit) => Ok(self.length_after_append(psdu_length, false)? <= limit),
            None => Ok(true),
        }
    }

    /// Check one referenced HT frame against a fresh aggregate without
    /// reserving or cancelling the descriptor pool.
    ///
    /// A scheduler can use this to route an individually valid frame to the
    /// ordinary queue when it exceeds a peer/rate aggregate ceiling. Keeping
    /// the probe value-only avoids consuming a generation or mutating the
    /// caller-supplied association byte limit before the real batch begins.
    pub fn can_fit_fresh_referenced_ht_frame(
        &self,
        frame_length: usize,
        hardware_mic_length: u8,
        rate: HtRate,
        maximum_aggregate_bytes: u16,
        dma_capacity: usize,
    ) -> Result<bool, HtAmpduTxError> {
        let psdu_length =
            Self::fresh_referenced_psdu_length(frame_length, hardware_mic_length, dma_capacity)?;
        let aggregate_length = 4_u32 + u32::from(psdu_length);
        if maximum_aggregate_bytes == 0 || aggregate_length > u32::from(maximum_aggregate_bytes) {
            return Ok(false);
        }
        Ok(rate
            .vendor_ampdu_byte_limit()
            .is_none_or(|limit| aggregate_length <= u32::from(limit)))
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(super) fn can_commit_frame_with_hardware_he_control(
        &self,
        cookie: TxCookie,
        frame_length: usize,
        hardware_mic_length: u8,
        hardware_he_control: bool,
    ) -> Result<bool, HtAmpduTxError> {
        self.can_commit_frame_in_capacity(
            cookie,
            frame_length,
            hardware_mic_length,
            hardware_he_control,
            BUFFER_SIZE,
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
        if self.state != TxSlotState::Reserved || self.active != cookie {
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

    /// Check one HE frame using the blob-derived delimiter padding policy.
    ///
    /// Unlike [`Self::can_commit_frame`], the caller cannot accidentally
    /// publish an HE short subframe with a guessed metadata-byte-four value.
    #[cfg(any(test, feature = "test-support"))]
    pub fn can_commit_he_frame(
        &self,
        cookie: TxCookie,
        frame_length: usize,
        hardware_mic_length: u8,
        rate: HeRate,
        density: HtAmpduDensity,
    ) -> Result<bool, HtAmpduTxError> {
        self.can_commit_he_frame_with_txop(
            cookie,
            frame_length,
            hardware_mic_length,
            rate,
            density,
            HeEdcaTxopLimit::DEFAULT,
        )
    }

    /// Check one HE frame against both peer and rate/TXOP APEP ceilings.
    #[cfg(any(test, feature = "test-support"))]
    pub fn can_commit_he_frame_with_txop(
        &self,
        cookie: TxCookie,
        frame_length: usize,
        hardware_mic_length: u8,
        rate: HeRate,
        density: HtAmpduDensity,
        txop_limit: HeEdcaTxopLimit,
    ) -> Result<bool, HtAmpduTxError> {
        let psdu_length = frame_length
            .checked_add(usize::from(hardware_mic_length))
            .and_then(|length| length.checked_add(usize::from(TX_FCS_SIZE)))
            .and_then(|length| u16::try_from(length).ok())
            .filter(|length| *length != 0 && *length <= 0x3fff)
            .ok_or(HtAmpduTxError::FrameTooLong)?;
        let empty_delimiters = rate
            .ampdu_empty_delimiters(psdu_length, density)
            .ok_or(HtAmpduTxError::FrameTooLong)?;
        if !self.can_commit_frame(cookie, frame_length, hardware_mic_length, empty_delimiters)? {
            return Ok(false);
        }
        let aggregate_length = u32::from(self.length_after_append(psdu_length, false)?);
        Ok(rate
            .checked_maximum_apep_bytes(txop_limit)
            .is_some_and(|limit| aggregate_length <= limit))
    }

    /// Check one referenced HE frame against allocation, APEP and TXOP limits.
    pub fn can_commit_referenced_he_frame(
        &self,
        cookie: TxCookie,
        frame_size: AmpduFrameSize,
        policy: HeAmpduPolicy,
        dma_capacity: usize,
    ) -> Result<bool, HtAmpduTxError> {
        let frame_length = frame_size.mpdu_length();
        let hardware_mic_length = frame_size.hardware_mic_length();
        let psdu_length = frame_length
            .checked_add(usize::from(hardware_mic_length))
            .and_then(|length| length.checked_add(usize::from(TX_FCS_SIZE)))
            .and_then(|length| u16::try_from(length).ok())
            .filter(|length| *length != 0 && *length <= 0x3fff)
            .ok_or(HtAmpduTxError::FrameTooLong)?;
        let _empty_delimiters = policy
            .rate()
            .ampdu_empty_delimiters(psdu_length, policy.density())
            .ok_or(HtAmpduTxError::FrameTooLong)?;
        if !self.can_commit_frame_in_capacity(
            cookie,
            frame_length,
            hardware_mic_length,
            false,
            dma_capacity,
        )? {
            return Ok(false);
        }
        let aggregate_length = u32::from(self.length_after_append(psdu_length, false)?);
        Ok(policy
            .rate()
            .checked_maximum_apep_bytes(policy.txop_limit())
            .is_some_and(|limit| aggregate_length <= limit))
    }

    /// Check one referenced HE frame against a fresh aggregate without
    /// changing descriptor ownership.
    pub fn can_fit_fresh_referenced_he_frame(
        &self,
        frame_size: AmpduFrameSize,
        policy: HeAmpduPolicy,
        maximum_aggregate_bytes: u16,
        dma_capacity: usize,
    ) -> Result<bool, HtAmpduTxError> {
        let psdu_length = Self::fresh_referenced_psdu_length(
            frame_size.mpdu_length(),
            frame_size.hardware_mic_length(),
            dma_capacity,
        )?;
        let _empty_delimiters = policy
            .rate()
            .ampdu_empty_delimiters(psdu_length, policy.density())
            .ok_or(HtAmpduTxError::FrameTooLong)?;
        let aggregate_length = 4_u32 + u32::from(psdu_length);
        Ok(maximum_aggregate_bytes != 0
            && aggregate_length <= u32::from(maximum_aggregate_bytes)
            && policy
                .rate()
                .checked_maximum_apep_bytes(policy.txop_limit())
                .is_some_and(|limit| aggregate_length <= limit))
    }

    fn fresh_referenced_psdu_length(
        frame_length: usize,
        hardware_mic_length: u8,
        dma_capacity: usize,
    ) -> Result<u16, HtAmpduTxError> {
        let psdu_length = frame_length
            .checked_add(usize::from(hardware_mic_length))
            .and_then(|length| length.checked_add(usize::from(TX_FCS_SIZE)))
            .and_then(|length| u16::try_from(length).ok())
            .filter(|length| *length != 0 && *length <= 0x3fff)
            .ok_or(HtAmpduTxError::FrameTooLong)?;
        let descriptor_capacity = TX_AMPDU_METADATA_SIZE
            .checked_add(usize::from(psdu_length))
            .and_then(|length| length.checked_add(3))
            .map(|length| length & !3)
            .ok_or(HtAmpduTxError::FrameTooLong)?;
        if descriptor_capacity > dma_capacity {
            return Err(HtAmpduTxError::FrameTooLong);
        }
        Ok(psdu_length)
    }

    /// Check one HE frame whose four-byte HE-Control is inserted by hardware.
    ///
    /// The DMA-resident frame length remains unchanged. Only APEP accounting
    /// gains four bytes through metadata byte seven bit zero.
    #[cfg(any(test, feature = "test-support"))]
    pub fn can_commit_hardware_he_control_frame(
        &self,
        cookie: TxCookie,
        frame_length: usize,
        hardware_mic_length: u8,
        rate: HeRate,
        density: HtAmpduDensity,
    ) -> Result<bool, HtAmpduTxError> {
        self.can_commit_hardware_he_control_frame_with_txop(
            cookie,
            frame_length,
            hardware_mic_length,
            rate,
            density,
            HeEdcaTxopLimit::DEFAULT,
        )
    }

    /// Check hardware HE-Control insertion under the same complete APEP gate.
    #[cfg(any(test, feature = "test-support"))]
    pub fn can_commit_hardware_he_control_frame_with_txop(
        &self,
        cookie: TxCookie,
        frame_length: usize,
        hardware_mic_length: u8,
        rate: HeRate,
        density: HtAmpduDensity,
        txop_limit: HeEdcaTxopLimit,
    ) -> Result<bool, HtAmpduTxError> {
        let psdu_length = frame_length
            .checked_add(usize::from(hardware_mic_length))
            .and_then(|length| length.checked_add(usize::from(TX_FCS_SIZE)))
            .and_then(|length| u16::try_from(length).ok())
            .filter(|length| *length != 0 && *length <= 0x3fff)
            .ok_or(HtAmpduTxError::FrameTooLong)?;
        let _empty_delimiters = rate
            .ampdu_empty_delimiters(psdu_length, density)
            .ok_or(HtAmpduTxError::FrameTooLong)?;
        if !self.can_commit_frame_with_hardware_he_control(
            cookie,
            frame_length,
            hardware_mic_length,
            true,
        )? {
            return Ok(false);
        }
        let aggregate_length = u32::from(self.length_after_append(psdu_length, true)?);
        Ok(rate
            .checked_maximum_apep_bytes(txop_limit)
            .is_some_and(|limit| aggregate_length <= limit))
    }
}
