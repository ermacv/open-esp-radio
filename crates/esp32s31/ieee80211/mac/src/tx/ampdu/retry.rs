//! Detached-frame inspection and bounded A-MPDU retry compaction.

use core::pin::Pin;

#[cfg(any(test, feature = "test-support"))]
use oer_ieee80211_upper_mac::ampdu::set_retry_bit;

use super::{
    HtAmpduLength, HtAmpduTxError, HtAmpduTxStorage, TX_AMPDU_METADATA_SIZE, TxCookie, TxSlotState,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct CompletedFrameLayout {
    pub(super) index: usize,
    pub(super) buffer_address: usize,
    pub(super) capacity: usize,
    pub(super) frame_start: usize,
    pub(super) frame_end: usize,
    pub(super) hardware_mic_length: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct RetryFrameLocation {
    pub(super) index: usize,
    pub(super) buffer_address: usize,
    pub(super) capacity: usize,
}

impl<const SLOTS: usize, const BUFFER_SIZE: usize> HtAmpduTxStorage<SLOTS, BUFFER_SIZE> {
    /// Borrow one detached, completed encoded MPDU for an individual retry.
    ///
    /// The returned slice excludes the private eight-byte DMA metadata prefix
    /// and the hardware-generated MIC/FCS trailer. Sequence Control and CCMP
    /// header are retained exactly as originally submitted.
    #[cfg(any(test, feature = "test-support"))]
    pub fn completed_frame(
        &self,
        cookie: TxCookie,
        index: u8,
    ) -> Result<(&[u8], u8), HtAmpduTxError> {
        let layout = self.completed_frame_layout(cookie, index)?;
        let buffer = &self.buffers[layout.index].0;
        if layout.buffer_address != buffer.as_ptr().addr() || layout.capacity > buffer.len() {
            return Err(HtAmpduTxError::BackingUnavailable { index });
        }
        Ok((
            &buffer[layout.frame_start..layout.frame_end],
            layout.hardware_mic_length,
        ))
    }

    pub(super) fn completed_frame_layout(
        &self,
        cookie: TxCookie,
        index: u8,
    ) -> Result<CompletedFrameLayout, HtAmpduTxError> {
        if self.state != TxSlotState::Completed || self.active != cookie || !self.detached {
            return Err(HtAmpduTxError::Stale);
        }
        let slot = usize::from(index);
        if slot >= usize::from(self.count) {
            return Err(HtAmpduTxError::FrameIndexOutOfRange {
                index,
                count: self.count,
            });
        }
        let frame_length = usize::from(self.frame_lengths[slot]);
        let capacity = usize::from(self.descriptor_capacities[slot]);
        let frame_end = TX_AMPDU_METADATA_SIZE
            .checked_add(frame_length)
            .filter(|end| *end <= capacity)
            .ok_or(HtAmpduTxError::BackingUnavailable { index })?;
        Ok(CompletedFrameLayout {
            index: slot,
            buffer_address: self.buffer_addresses[slot],
            capacity,
            frame_start: TX_AMPDU_METADATA_SIZE,
            frame_end,
            hardware_mic_length: self.hardware_mic_lengths[slot],
        })
    }

    /// Retain only selected completed MPDUs for another A-MPDU attempt.
    ///
    /// Bit `n` of `retry_mask` selects completed slot `n`. Selected frames are
    /// compacted toward slot zero, keep their Sequence Control and CCMP PN,
    /// and gain only the IEEE 802.11 Retry bit. This is the owned-array form
    /// of the finite mutation observed in
    /// `libpp.a[pp.o]::ppResortTxAMPDU`: its partial-BlockAck path
    /// detached the old links, preserved the encoded missing MPDU, set
    /// Frame Control.Retry, and placed it at the head of a new aggregate.
    ///
    /// The queue must already be detached, so hardware cannot observe the
    /// compaction. A one-frame mask is valid because HE keeps a failed
    /// one-member A-MPDU under this same owner; converting it to the ordinary
    /// queue first requires the distinct `ppHEAMPDU2Normal` metadata
    /// transition.
    ///
    /// The retained aggregate also keeps its original PHY-rate image.
    /// Complete `libpp.a[lmac.o]::lmacRetryTxFrame` compares state
    /// byte `+0x12` with four and skips `rcGetRate` in that state; complete
    /// `lmacProcessLongRetryFail` writes four immediately before entering the
    /// A-MPDU retry leaf. Only the compacted byte/subframe count and the next
    /// EDCA backoff are expected to change before republication.
    #[cfg(any(test, feature = "test-support"))]
    pub fn retain_for_ampdu_retry(
        mut self: Pin<&mut Self>,
        cookie: TxCookie,
        retry_mask: u32,
        republication: super::AmpduRepublication,
    ) -> Result<HtAmpduLength, HtAmpduTxError> {
        let locations = self
            .as_ref()
            .retry_frame_locations(cookie, retry_mask, republication)?;
        for location in locations.iter().flatten() {
            if !self.as_ref().get_ref().internal_frame_matches(*location) {
                return Err(HtAmpduTxError::BackingUnavailable {
                    index: location.index as u8,
                });
            }
        }
        if republication == super::AmpduRepublication::Retransmission {
            self.as_mut().mark_internal_retry_frames(&locations);
        }
        self.compact_retry_metadata(locations)
    }

    /// Resolve the retained frames. A republication after a protection
    /// failure must retain the complete aggregate.
    pub(super) fn retry_frame_locations(
        self: Pin<&Self>,
        cookie: TxCookie,
        retry_mask: u32,
        republication: super::AmpduRepublication,
    ) -> Result<[Option<RetryFrameLocation>; SLOTS], HtAmpduTxError> {
        let storage = self.get_ref();
        if storage.state != TxSlotState::Completed || storage.active != cookie || !storage.detached
        {
            return Err(HtAmpduTxError::Stale);
        }
        let old_count = usize::from(storage.count);
        let valid_mask = if old_count == 32 {
            u32::MAX
        } else {
            (1_u32 << old_count) - 1
        };
        let incomplete_republication = republication
            == super::AmpduRepublication::AfterProtectionFailure
            && retry_mask != valid_mask;
        if retry_mask == 0 || retry_mask & !valid_mask != 0 || incomplete_republication {
            return Err(HtAmpduTxError::InvalidRetryMask {
                mask: retry_mask,
                count: storage.count,
            });
        }
        for source in 0..old_count {
            if retry_mask & (1_u32 << source) != 0 && storage.frame_lengths[source] < 2 {
                return Err(HtAmpduTxError::RetainedFrameTooShort {
                    index: source as u8,
                    length: storage.frame_lengths[source],
                });
            }
        }

        let mut locations = [None; SLOTS];
        for (source, location) in locations.iter_mut().enumerate().take(old_count) {
            if retry_mask & (1_u32 << source) == 0 {
                continue;
            }
            let capacity = usize::from(storage.descriptor_capacities[source]);
            if TX_AMPDU_METADATA_SIZE + 2 > capacity {
                return Err(HtAmpduTxError::BackingUnavailable {
                    index: source as u8,
                });
            }
            *location = Some(RetryFrameLocation {
                index: source,
                buffer_address: storage.buffer_addresses[source],
                capacity,
            });
        }
        Ok(locations)
    }

    #[cfg(any(test, feature = "test-support"))]
    fn internal_frame_matches(&self, location: RetryFrameLocation) -> bool {
        let buffer = &self.buffers[location.index].0;
        location.buffer_address == buffer.as_ptr().addr() && location.capacity <= buffer.len()
    }

    #[cfg(any(test, feature = "test-support"))]
    fn mark_internal_retry_frames(
        mut self: Pin<&mut Self>,
        locations: &[Option<RetryFrameLocation>; SLOTS],
    ) {
        let storage = self.as_mut().project();
        for location in locations.iter().flatten() {
            if location.buffer_address == storage.buffers[location.index].0.as_ptr().addr()
                && location.capacity <= storage.buffers[location.index].0.len()
            {
                set_retry_bit(&mut storage.buffers[location.index].0[TX_AMPDU_METADATA_SIZE..]);
            }
        }
    }

    pub(super) fn compact_retry_metadata(
        mut self: Pin<&mut Self>,
        locations: [Option<RetryFrameLocation>; SLOTS],
    ) -> Result<HtAmpduLength, HtAmpduTxError> {
        let storage = self.as_mut().project();
        let mut destination = 0_usize;
        for location in locations.iter().flatten() {
            let source = location.index;
            if destination != source {
                storage.buffer_addresses[destination] = storage.buffer_addresses[source];
                storage.frame_lengths[destination] = storage.frame_lengths[source];
                storage.hardware_mic_lengths[destination] = storage.hardware_mic_lengths[source];
                storage.msdu_lengths[destination] = storage.msdu_lengths[source];
                storage.psdu_lengths[destination] = storage.psdu_lengths[source];
                storage.hardware_he_control[destination] = storage.hardware_he_control[source];
                storage.empty_delimiters[destination] = storage.empty_delimiters[source];
                storage.descriptor_capacities[destination] = storage.descriptor_capacities[source];
            }
            destination += 1;
        }
        *storage.count = u8::try_from(destination)
            .map_err(|_| HtAmpduTxError::SlotCountOverflow { count: destination })?;
        *storage.prepared_length = 0;
        *storage.aggregate_length = 0;
        *storage.trigger_reservation = None;
        *storage.trigger_publication_snapshot = None;
        *storage.detached = false;
        *storage.state = TxSlotState::Reserved;
        self.recalculate_prepared_length()
    }
}
