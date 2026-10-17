//! Safe S31 A-MPDU metadata encoding and slot-state commit operations.

use core::pin::Pin;

use super::{
    AmpduFrameLayout, HeAmpduFrameRequest, HtAmpduFrameRequest, HtAmpduTxError, HtAmpduTxStorage,
    TX_AMPDU_METADATA_SIZE, TX_FCS_SIZE,
};
#[cfg(any(test, feature = "test-support"))]
use super::{AmpduFrameSize, HeAmpduPolicy};
#[cfg(any(test, feature = "test-support"))]
use crate::tx::{HeEdcaTxopLimit, HeRate, HtAmpduDensity};
use crate::tx::{TxCookie, TxSlotState};

impl<const SLOTS: usize, const BUFFER_SIZE: usize> HtAmpduTxStorage<SLOTS, BUFFER_SIZE> {
    /// Commit the next encoded MPDU and its hardware-generated trailer.
    ///
    /// `frame_length` is the encoded 802.11 MPDU before the hardware MIC and
    /// FCS. The resulting metadata length is the complete PSDU length used by
    /// the recovered `ppCalTxAMPDULength` accounting.
    #[cfg(any(test, feature = "test-support"))]
    pub fn commit_frame(
        self: Pin<&mut Self>,
        cookie: TxCookie,
        frame_length: usize,
        hardware_mic_length: u8,
        empty_delimiters: u8,
    ) -> Result<(), HtAmpduTxError> {
        self.commit_frame_with_hardware_he_control(
            cookie,
            frame_length,
            hardware_mic_length,
            empty_delimiters,
            false,
        )
    }

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
        let frame_length = layout.mpdu_length();
        let hardware_mic_length = layout.hardware_mic_length();
        let storage = self.as_ref().get_ref();
        if storage.state != TxSlotState::Reserved || storage.active != cookie {
            return Err(HtAmpduTxError::Stale);
        }
        if !storage.can_commit_referenced_frame(
            cookie,
            frame_length,
            hardware_mic_length,
            dma_storage.len(),
        )? {
            return Err(HtAmpduTxError::AggregateFull);
        }
        let index = usize::from(storage.count);
        if index >= SLOTS {
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
        let prepared_length = storage.length_after_append(psdu_length, false)?;

        let storage = self.project();
        storage.buffer_addresses[index] = dma_storage.as_mut_ptr().addr();
        dma_storage[..4].copy_from_slice(&u32::from(psdu_length).to_le_bytes());
        dma_storage[4] = empty_delimiters;
        dma_storage[5..TX_AMPDU_METADATA_SIZE].fill(0);
        let trailer_start = TX_AMPDU_METADATA_SIZE + frame_length;
        dma_storage[trailer_start..transfer_length].fill(0);
        dma_storage[transfer_length..usize::from(descriptor_capacity)].fill(0);
        storage.frame_lengths[index] =
            u16::try_from(frame_length).map_err(|_| HtAmpduTxError::FrameTooLong)?;
        storage.hardware_mic_lengths[index] = hardware_mic_length;
        storage.msdu_lengths[index] = 0;
        storage.psdu_lengths[index] = psdu_length;
        storage.hardware_he_control[index] = false;
        storage.empty_delimiters[index] = empty_delimiters;
        storage.descriptor_capacities[index] = descriptor_capacity;
        *storage.prepared_length = prepared_length;
        *storage.count += 1;
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

    #[cfg(any(test, feature = "test-support"))]
    fn commit_frame_with_hardware_he_control(
        self: Pin<&mut Self>,
        cookie: TxCookie,
        frame_length: usize,
        hardware_mic_length: u8,
        empty_delimiters: u8,
        hardware_he_control: bool,
    ) -> Result<(), HtAmpduTxError> {
        let storage = self.as_ref().get_ref();
        if storage.state != TxSlotState::Reserved || storage.active != cookie {
            return Err(HtAmpduTxError::Stale);
        }
        if !storage.can_commit_frame_with_hardware_he_control(
            cookie,
            frame_length,
            hardware_mic_length,
            hardware_he_control,
        )? {
            return Err(HtAmpduTxError::AggregateFull);
        }
        let index = usize::from(storage.count);
        if index >= SLOTS {
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
            .filter(|length| *length <= BUFFER_SIZE)
            .and_then(|length| u16::try_from(length).ok())
            .ok_or(HtAmpduTxError::FrameTooLong)?;
        let prepared_length = storage.length_after_append(psdu_length, hardware_he_control)?;

        let storage = self.project();
        let buffer = &mut storage.buffers[index].0;
        storage.buffer_addresses[index] = buffer.as_mut_ptr().addr();
        buffer[..4].copy_from_slice(&u32::from(psdu_length).to_le_bytes());
        buffer[4] = empty_delimiters;
        buffer[5..TX_AMPDU_METADATA_SIZE].fill(0);
        if hardware_he_control {
            // SOURCE: complete `libpp[pp_he.o]::ppCalSubFrameLength` uses byte seven bit zero as the four-byte
            // on-air insertion term. HIL_VENDOR_HE_CONTROL_INSERTION_2026_07_30
            // captured vendor metadata word one as 0x0100_0000 while CCMP
            // remained immediately after the QoS header in DMA.
            buffer[7] = 1;
        }
        let trailer_start = TX_AMPDU_METADATA_SIZE + frame_length;
        buffer[trailer_start..transfer_length].fill(0);
        buffer[transfer_length..usize::from(descriptor_capacity)].fill(0);
        storage.frame_lengths[index] =
            u16::try_from(frame_length).map_err(|_| HtAmpduTxError::FrameTooLong)?;
        storage.hardware_mic_lengths[index] = hardware_mic_length;
        // The generic MPDU API does not know the pre-encapsulation MSDU
        // length. Clear a possibly retained value from the previous
        // generation so Trigger publication fails closed.
        storage.msdu_lengths[index] = 0;
        storage.psdu_lengths[index] = psdu_length;
        storage.hardware_he_control[index] = hardware_he_control;
        storage.empty_delimiters[index] = empty_delimiters;
        storage.descriptor_capacities[index] = descriptor_capacity;
        *storage.prepared_length = prepared_length;
        *storage.count += 1;
        Ok(())
    }

    /// Commit one HE MPDU with exact PP-blob delimiter padding.
    #[cfg(any(test, feature = "test-support"))]
    pub fn commit_he_frame(
        self: Pin<&mut Self>,
        cookie: TxCookie,
        frame_length: usize,
        hardware_mic_length: u8,
        rate: HeRate,
        density: HtAmpduDensity,
    ) -> Result<(), HtAmpduTxError> {
        self.commit_he_frame_with_txop(
            cookie,
            frame_length,
            hardware_mic_length,
            rate,
            density,
            HeEdcaTxopLimit::DEFAULT,
        )
    }

    /// Commit one HE MPDU under the complete rate/TXOP duration policy.
    #[cfg(any(test, feature = "test-support"))]
    pub fn commit_he_frame_with_txop(
        self: Pin<&mut Self>,
        cookie: TxCookie,
        frame_length: usize,
        hardware_mic_length: u8,
        rate: HeRate,
        density: HtAmpduDensity,
        txop_limit: HeEdcaTxopLimit,
    ) -> Result<(), HtAmpduTxError> {
        if !self.as_ref().can_commit_he_frame_with_txop(
            cookie,
            frame_length,
            hardware_mic_length,
            rate,
            density,
            txop_limit,
        )? {
            return Err(HtAmpduTxError::AggregateFull);
        }
        let psdu_length = frame_length
            .checked_add(usize::from(hardware_mic_length))
            .and_then(|length| length.checked_add(usize::from(TX_FCS_SIZE)))
            .and_then(|length| u16::try_from(length).ok())
            .filter(|length| *length != 0 && *length <= 0x3fff)
            .ok_or(HtAmpduTxError::FrameTooLong)?;
        let empty_delimiters = rate
            .ampdu_empty_delimiters(psdu_length, density)
            .ok_or(HtAmpduTxError::FrameTooLong)?;
        self.commit_frame(cookie, frame_length, hardware_mic_length, empty_delimiters)
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

    /// Commit one HE MPDU with a hardware-inserted HE-Control field.
    ///
    /// Unlike the former placeholder workaround, this does not move CCMP or
    /// increase the low fourteen-bit DMA MPDU length. It publishes the exact
    /// vendor metadata byte-seven bit and adds four only to assembled APEP
    /// accounting.
    #[cfg(any(test, feature = "test-support"))]
    pub fn commit_hardware_he_control_frame(
        self: Pin<&mut Self>,
        cookie: TxCookie,
        frame_length: usize,
        hardware_mic_length: u8,
        rate: HeRate,
        density: HtAmpduDensity,
    ) -> Result<(), HtAmpduTxError> {
        self.commit_hardware_he_control_frame_with_txop(
            cookie,
            frame_length,
            hardware_mic_length,
            rate,
            density,
            HeEdcaTxopLimit::DEFAULT,
        )
    }

    /// Commit hardware HE-Control insertion under the complete duration gate.
    #[cfg(any(test, feature = "test-support"))]
    pub fn commit_hardware_he_control_frame_with_txop(
        self: Pin<&mut Self>,
        cookie: TxCookie,
        frame_length: usize,
        hardware_mic_length: u8,
        rate: HeRate,
        density: HtAmpduDensity,
        txop_limit: HeEdcaTxopLimit,
    ) -> Result<(), HtAmpduTxError> {
        if !self
            .as_ref()
            .can_commit_hardware_he_control_frame_with_txop(
                cookie,
                frame_length,
                hardware_mic_length,
                rate,
                density,
                txop_limit,
            )?
        {
            return Err(HtAmpduTxError::AggregateFull);
        }
        let psdu_length = frame_length
            .checked_add(usize::from(hardware_mic_length))
            .and_then(|length| length.checked_add(usize::from(TX_FCS_SIZE)))
            .and_then(|length| u16::try_from(length).ok())
            .filter(|length| *length != 0 && *length <= 0x3fff)
            .ok_or(HtAmpduTxError::FrameTooLong)?;
        let empty_delimiters = rate
            .ampdu_empty_delimiters(psdu_length, density)
            .ok_or(HtAmpduTxError::FrameTooLong)?;
        self.commit_frame_with_hardware_he_control(
            cookie,
            frame_length,
            hardware_mic_length,
            empty_delimiters,
            true,
        )
    }

    /// Commit one HE MPDU together with its original MSDU byte count.
    ///
    /// SOURCE: complete `libpp[hal_debug.o]::dbg_read_tx_ppdu`
    /// names frame-state halfword `+0x22` `msdu_len`; complete
    /// `libpp[hal_mac_tx.o]::mac_tx_set_tb` sums that halfword
    /// across the linked aggregate and publishes it to `WDEVTXQBSR_SW`.
    /// Keeping the value beside the owned encoded MPDU prevents the Trigger
    /// path from substituting the larger 802.11 frame or PSDU length.
    #[cfg(any(test, feature = "test-support"))]
    pub fn commit_he_msdu_frame(
        mut self: Pin<&mut Self>,
        cookie: TxCookie,
        frame_length: usize,
        hardware_mic_length: u8,
        msdu_length: usize,
        rate: HeRate,
        density: HtAmpduDensity,
    ) -> Result<(), HtAmpduTxError> {
        let msdu_length = u16::try_from(msdu_length)
            .ok()
            .filter(|length| *length != 0)
            .ok_or(HtAmpduTxError::TriggerMsduLengthUnavailable)?;
        let index = usize::from(self.count);
        self.as_mut()
            .commit_he_frame(cookie, frame_length, hardware_mic_length, rate, density)?;
        self.project().msdu_lengths[index] = msdu_length;
        Ok(())
    }

    /// Commit a Trigger-eligible HE MPDU with hardware HE-Control insertion.
    #[cfg(any(test, feature = "test-support"))]
    pub fn commit_hardware_he_control_msdu_frame(
        self: Pin<&mut Self>,
        cookie: TxCookie,
        frame_size: AmpduFrameSize,
        msdu_length: usize,
        rate: HeRate,
        density: HtAmpduDensity,
    ) -> Result<(), HtAmpduTxError> {
        self.commit_hardware_he_control_msdu_frame_with_policy(
            cookie,
            frame_size,
            msdu_length,
            HeAmpduPolicy::new(rate, density, HeEdcaTxopLimit::DEFAULT),
        )
    }

    /// Commit Trigger metadata and HE-Control under one HE duration policy.
    #[cfg(any(test, feature = "test-support"))]
    pub fn commit_hardware_he_control_msdu_frame_with_policy(
        mut self: Pin<&mut Self>,
        cookie: TxCookie,
        frame_size: AmpduFrameSize,
        msdu_length: usize,
        policy: HeAmpduPolicy,
    ) -> Result<(), HtAmpduTxError> {
        let msdu_length = u16::try_from(msdu_length)
            .ok()
            .filter(|length| *length != 0)
            .ok_or(HtAmpduTxError::TriggerMsduLengthUnavailable)?;
        let index = usize::from(self.count);
        self.as_mut().commit_hardware_he_control_frame_with_txop(
            cookie,
            frame_size.mpdu_length(),
            frame_size.hardware_mic_length(),
            policy.rate(),
            policy.density(),
            policy.txop_limit(),
        )?;
        self.project().msdu_lengths[index] = msdu_length;
        Ok(())
    }
}
