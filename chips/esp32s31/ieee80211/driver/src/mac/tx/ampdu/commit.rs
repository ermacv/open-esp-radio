//! Safe S31 A-MPDU metadata encoding and slot-state commit operations.

use core::pin::Pin;

use super::{
    AmpduFrameLayout, HeAmpduFrameRequest, HtAmpduFrameRequest, HtAmpduTxError, HtAmpduTxStorage,
    TX_AMPDU_METADATA_SIZE, TX_FCS_SIZE,
};
use crate::mac::tx::{TxCookie, TxDmaState};

/// One referenced MPDU checked against its batch, its metadata prefix
/// written, not yet counted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ReferencedFrame {
    buffer_address: usize,
    frame_length: u16,
    hardware_mic_length: u8,
    psdu_length: u16,
    empty_delimiters: u8,
    descriptor_capacity: u16,
    prepared_length: u16,
}

impl ReferencedFrame {
    /// The descriptor's buffer capacity.
    pub(super) const fn descriptor_capacity(&self) -> u32 {
        self.descriptor_capacity as u32
    }

    /// The octets the DMA reads: the prefix and the PSDU.
    pub(super) const fn transfer_length(&self) -> u32 {
        (TX_AMPDU_METADATA_SIZE as u32).saturating_add(self.psdu_length as u32)
    }
}

impl<const SLOTS: usize> HtAmpduTxStorage<SLOTS> {
    /// Commit an MPDU already encoded in a separately pinned allocation.
    ///
    /// `dma_storage` begins with the private eight-byte S31 metadata prefix;
    /// the encoded 802.11 frame must already occupy
    /// `dma_storage[8..8 + frame_length]`. This is the descriptor half of the
    /// vendor cache-TX/type-nine ESF path and performs no payload copy.
    ///
    /// The private entry is reachable only through [`super::RetainedDmaAmpduTx`],
    /// which retains exclusive ownership of the same stable allocation until
    /// this batch has completed, detached and released or cancelled.
    ///
    /// SOURCE: complete `libnet80211[ieee80211_output.o]::ieee80211_alloc_tx_buf` cache-TX/type-nine branch retains the netstack
    /// buffer through `s_netstack_ref`; complete `libpp[pp.o]::ppAssembleAMPDU` links the existing ESF descriptors without copying
    /// their payloads.
    pub(super) fn commit_referenced_frame(
        self: Pin<&mut Self>,
        cookie: TxCookie,
        dma_storage: &mut [u8],
        layout: AmpduFrameLayout,
        empty_delimiters: u8,
    ) -> Result<(), HtAmpduTxError> {
        let frame = self.as_ref().get_ref().prepare_referenced_frame(
            cookie,
            dma_storage,
            layout,
            empty_delimiters,
        )?;
        self.record_referenced_frame(cookie, frame)
    }

    /// Check an MPDU encoded in a separately pinned allocation, as
    /// [`Self::commit_referenced_frame`] does, and write its private
    /// metadata prefix and zeroed trailer into `dma_storage`; nothing is
    /// counted until [`Self::record_referenced_frame`] takes the result.
    pub(super) fn prepare_referenced_frame(
        &self,
        cookie: TxCookie,
        dma_storage: &mut [u8],
        layout: AmpduFrameLayout,
        empty_delimiters: u8,
    ) -> Result<ReferencedFrame, HtAmpduTxError> {
        let frame_length = layout.mpdu_length();
        let hardware_mic_length = layout.hardware_mic_length();
        if self.state != TxDmaState::Reserved || self.active != cookie {
            return Err(HtAmpduTxError::Stale);
        }
        if !self.can_commit_referenced_frame(
            cookie,
            frame_length,
            hardware_mic_length,
            dma_storage.len(),
        )? {
            return Err(HtAmpduTxError::AggregateFull);
        }
        if usize::from(self.count) >= SLOTS {
            return Err(HtAmpduTxError::SlotCapacity);
        }
        let psdu_length = frame_length
            .checked_add(usize::from(hardware_mic_length))
            .and_then(|length| length.checked_add(usize::from(TX_FCS_SIZE)))
            .and_then(|length| u16::try_from(length).ok())
            .filter(|length| *length != 0 && *length <= 0x3fff)
            .ok_or(HtAmpduTxError::FrameTooLong)?;
        let transfer_length = TX_AMPDU_METADATA_SIZE
            .checked_add(usize::from(psdu_length))
            .ok_or(HtAmpduTxError::FrameTooLong)?;
        let descriptor_capacity = transfer_length
            .checked_add(3)
            .map(|length| length & !3)
            .filter(|length| *length <= dma_storage.len())
            .and_then(|length| u16::try_from(length).ok())
            .ok_or(HtAmpduTxError::FrameTooLong)?;
        let prepared_length = self.length_after_append(psdu_length, false)?;
        let trailer_start = TX_AMPDU_METADATA_SIZE
            .checked_add(frame_length)
            .ok_or(HtAmpduTxError::FrameTooLong)?;
        let (prefix, rest) = dma_storage
            .split_first_chunk_mut::<TX_AMPDU_METADATA_SIZE>()
            .ok_or(HtAmpduTxError::FrameTooLong)?;
        let (length, flags) = prefix.split_at_mut(4);
        length.copy_from_slice(&u32::from(psdu_length).to_le_bytes());
        if let Some((delimiters, reserved)) = flags.split_first_mut() {
            *delimiters = empty_delimiters;
            reserved.fill(0);
        }
        let trailer_length = usize::from(descriptor_capacity)
            .checked_sub(trailer_start)
            .ok_or(HtAmpduTxError::FrameTooLong)?;
        rest.get_mut(frame_length..)
            .and_then(|trailer| trailer.get_mut(..trailer_length))
            .ok_or(HtAmpduTxError::FrameTooLong)?
            .fill(0);
        Ok(ReferencedFrame {
            buffer_address: dma_storage.as_mut_ptr().addr(),
            frame_length: u16::try_from(frame_length).map_err(|_| HtAmpduTxError::FrameTooLong)?,
            hardware_mic_length,
            psdu_length,
            empty_delimiters,
            descriptor_capacity,
            prepared_length,
        })
    }

    /// Count a frame [`Self::prepare_referenced_frame`] checked against
    /// this same batch, at its next position.
    pub(super) fn record_referenced_frame(
        self: Pin<&mut Self>,
        cookie: TxCookie,
        frame: ReferencedFrame,
    ) -> Result<(), HtAmpduTxError> {
        let storage = self.project();
        if *storage.state != TxDmaState::Reserved || *storage.active != cookie {
            return Err(HtAmpduTxError::Stale);
        }
        let index = usize::from(*storage.count);
        let count = storage
            .count
            .checked_add(1)
            .filter(|count| usize::from(*count) <= SLOTS)
            .ok_or(HtAmpduTxError::SlotCapacity)?;
        let (
            Some(buffer_address),
            Some(frame_length),
            Some(hardware_mic_length),
            Some(msdu_length),
            Some(psdu_length),
            Some(hardware_he_control),
            Some(empty_delimiters),
            Some(descriptor_capacity),
        ) = (
            storage.buffer_addresses.get_mut(index),
            storage.frame_lengths.get_mut(index),
            storage.hardware_mic_lengths.get_mut(index),
            storage.msdu_lengths.get_mut(index),
            storage.psdu_lengths.get_mut(index),
            storage.hardware_he_control.get_mut(index),
            storage.empty_delimiters.get_mut(index),
            storage.descriptor_capacities.get_mut(index),
        )
        else {
            return Err(HtAmpduTxError::SlotCapacity);
        };
        *buffer_address = frame.buffer_address;
        *frame_length = frame.frame_length;
        *hardware_mic_length = frame.hardware_mic_length;
        *msdu_length = 0;
        *psdu_length = frame.psdu_length;
        *hardware_he_control = false;
        *empty_delimiters = frame.empty_delimiters;
        *descriptor_capacity = frame.descriptor_capacity;
        *storage.prepared_length = frame.prepared_length;
        *storage.count = count;
        Ok(())
    }

    /// Commit one referenced HT MPDU under the exact rate-dependent ceiling.
    ///
    /// The retained owner upholds the same stable-allocation invariant as
    /// [`Self::commit_referenced_frame`].
    pub(super) fn commit_referenced_ht_frame(
        mut self: Pin<&mut Self>,
        cookie: TxCookie,
        dma_storage: &mut [u8],
        request: HtAmpduFrameRequest,
    ) -> Result<(), HtAmpduTxError> {
        let layout = request.layout();
        if !self.as_ref().can_commit_referenced_ht_frame(
            cookie,
            layout.mpdu_length(),
            layout.hardware_mic_length(),
            request.empty_delimiters(),
            request.rate(),
            dma_storage.len(),
        )? {
            return Err(HtAmpduTxError::AggregateFull);
        }
        self.as_mut().commit_referenced_frame(
            cookie,
            dma_storage,
            layout,
            request.empty_delimiters(),
        )
    }

    /// Commit one referenced HE MPDU under the complete rate/TXOP policy.
    ///
    /// SOURCE: complete `libpp[pp_he.o]::ppCalDeliNum,
    /// libpp[pp_he.o]::ppCalTxHEAMPDULength,libpp[pp_he.o]::ppCheckTxHEAMPDUlength` supplies delimiter and
    /// duration accounting. Complete
    /// `libnet80211[ieee80211_output.o]::ieee80211_alloc_tx_buf`
    /// and `libpp[pp.o]::ppAssembleAMPDU` retain and link the
    /// cache-TX allocation instead of copying it into aggregate storage.
    ///
    /// The retained owner keeps `dma_storage` at its current address until the
    /// batch is detached and released or cancelled.
    pub(super) fn commit_referenced_he_frame(
        mut self: Pin<&mut Self>,
        cookie: TxCookie,
        dma_storage: &mut [u8],
        request: HeAmpduFrameRequest,
    ) -> Result<(), HtAmpduTxError> {
        let layout = request.layout();
        if !self.as_ref().can_commit_referenced_he_frame(
            cookie,
            layout.frame_size(),
            request.policy(),
            dma_storage.len(),
        )? {
            return Err(HtAmpduTxError::AggregateFull);
        }
        let psdu_length = layout
            .mpdu_length()
            .checked_add(usize::from(layout.hardware_mic_length()))
            .and_then(|length| length.checked_add(usize::from(TX_FCS_SIZE)))
            .and_then(|length| u16::try_from(length).ok())
            .filter(|length| *length != 0 && *length <= 0x3fff)
            .ok_or(HtAmpduTxError::FrameTooLong)?;
        let policy = request.policy();
        let empty_delimiters = policy
            .rate()
            .ampdu_empty_delimiters(psdu_length, policy.density())
            .ok_or(HtAmpduTxError::FrameTooLong)?;
        self.as_mut()
            .commit_referenced_frame(cookie, dma_storage, layout, empty_delimiters)
    }
}
