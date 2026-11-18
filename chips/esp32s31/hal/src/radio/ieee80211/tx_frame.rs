//! The layout of a transmit buffer: the eight-byte entry the MAC reads as
//! its own (`WIFI_TX_METADATA_ENTRY`), the MPDU, then room for the MIC the
//! MAC appends and for the FCS. A buffer without the entry is refused at
//! once by the MAC (SOURCE: HW_WIFIMAC_TX_ENTRY_REQUIRED_20261117); the one
//! constructor here writes it, and a descriptor is reserved only with what
//! that constructor returns.

use crate::layouts::{WifiTxMetadataEntry, WifiTxMetadataEntryControl, WifiTxMetadataEntryMut};

/// The entry's bytes before the MPDU.
pub const TX_ENTRY_BYTES: usize = WifiTxMetadataEntry::SIZE;
/// The FCS the MAC appends after the MPDU and its MIC.
pub const TX_FCS_BYTES: usize = 4;

/// The fields of the entry its writer chooses; the length is the layout's.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TxEntry {
    /// `SINGLE`: the HE single-MPDU container.
    pub single: bool,
    /// `DELI_NUM`: the subframe's delimiters beyond one, in an aggregate.
    pub delimiters: u8,
    /// `HTC`: the MAC inserts an HT/HE Control field.
    pub htc: bool,
}

/// The descriptor capacity a layout takes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxCapacity {
    /// The transfer rounded up to a word.
    Minimal,
    /// This many bytes, at least the transfer and at most the buffer.
    Exactly(u32),
}

/// Why a buffer could not take a frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxFrameRefused {
    /// The MPDU with its MIC and FCS exceeds the entry's fourteen-bit length.
    TooLong,
    /// The buffer, or the asked capacity, does not hold the transfer.
    BufferTooSmall,
}

/// A transmit buffer laid out by [`Self::lay_out`]: its entry written and
/// its MIC and FCS room zeroed. Only that constructor makes one, so a
/// descriptor is never reserved for a buffer without its entry:
///
/// ```compile_fail
/// use oer_chips_esp32s31_hal::radio::ieee80211::tx_frame::TxFrame;
/// let forged = TxFrame { psdu_length: 4, transfer_length: 12, descriptor_capacity: 12 };
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TxFrame {
    psdu_length: u16,
    transfer_length: u32,
    descriptor_capacity: u32,
}

impl TxFrame {
    /// Lay `buffer` out for an MPDU of `mpdu_length` bytes, already written
    /// at [`TX_ENTRY_BYTES`], with `hardware_mic_length` bytes of MIC the MAC
    /// appends: write the entry (`MPDU_LEN` the MPDU with its MIC and FCS,
    /// then `entry`'s fields, the rest zero) and zero the bytes after the
    /// MPDU up to the descriptor's capacity.
    ///
    /// # Errors
    ///
    /// [`TxFrameRefused`], before any write.
    pub fn lay_out(
        buffer: &mut [u8],
        mpdu_length: usize,
        hardware_mic_length: usize,
        entry: TxEntry,
        capacity: TxCapacity,
    ) -> Result<Self, TxFrameRefused> {
        let psdu_length = mpdu_length
            .checked_add(hardware_mic_length)
            .and_then(|length| length.checked_add(TX_FCS_BYTES))
            .and_then(|length| u16::try_from(length).ok())
            .filter(|length| u32::from(*length) >> WifiTxMetadataEntryControl::MPDU_LEN_WIDTH == 0)
            .ok_or(TxFrameRefused::TooLong)?;
        let transfer_length = TX_ENTRY_BYTES
            .checked_add(usize::from(psdu_length))
            .ok_or(TxFrameRefused::TooLong)?;
        let descriptor_capacity = match capacity {
            TxCapacity::Minimal => transfer_length
                .checked_add(3)
                .map(|length| length & !3)
                .ok_or(TxFrameRefused::TooLong)?,
            TxCapacity::Exactly(capacity) => {
                usize::try_from(capacity).map_err(|_| TxFrameRefused::BufferTooSmall)?
            }
        };
        if descriptor_capacity < transfer_length || descriptor_capacity > buffer.len() {
            return Err(TxFrameRefused::BufferTooSmall);
        }
        let (head, rest) = buffer
            .split_first_chunk_mut::<TX_ENTRY_BYTES>()
            .ok_or(TxFrameRefused::BufferTooSmall)?;
        // Every bit the entry does not name is zero, as the vendor's prepend
        // leaves it.
        head.fill(0);
        let mut written = WifiTxMetadataEntryMut::new(head);
        let control = written
            .control()
            .with_mpdu_len(psdu_length)
            .ok_or(TxFrameRefused::TooLong)?
            .with_single(entry.single);
        let delimiters = written
            .delimiters()
            .with_deli_num(entry.delimiters)
            .with_htc(entry.htc);
        written.set_control(control);
        written.set_delimiters(delimiters);
        rest.get_mut(mpdu_length..descriptor_capacity.saturating_sub(TX_ENTRY_BYTES))
            .ok_or(TxFrameRefused::BufferTooSmall)?
            .fill(0);
        Ok(Self {
            psdu_length,
            transfer_length: u32::try_from(transfer_length).map_err(|_| TxFrameRefused::TooLong)?,
            descriptor_capacity: u32::try_from(descriptor_capacity)
                .map_err(|_| TxFrameRefused::BufferTooSmall)?,
        })
    }

    /// The MPDU with its MIC and FCS: the entry's `MPDU_LEN`.
    pub const fn psdu_length(self) -> u16 {
        self.psdu_length
    }

    /// The bytes the descriptor's LENGTH holds: the entry and the PSDU.
    pub const fn transfer_length(self) -> u32 {
        self.transfer_length
    }

    /// The bytes the descriptor's SIZE holds.
    pub const fn descriptor_capacity(self) -> u32 {
        self.descriptor_capacity
    }
}

#[cfg(test)]
mod tests;
