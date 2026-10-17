//! Allocation-free HE NDP Announcement parsing and encoding.

use crate::sequence::SequenceNumber;

const NDPA_FRAME_CONTROL: u16 = 0x0054;
const NDPA_HEADER_SIZE: usize = 17;
const HE_STA_INFO_SIZE: usize = 4;
const HE_MARKER: u8 = 1 << 1;
const AID11_MASK: u32 = 0x07ff;
const ACTION_NO_ACK_FRAME_CONTROL: u16 = 0x00e0;
const MANAGEMENT_HEADER_SIZE: usize = 24;
const HE_ACTION_CATEGORY: u8 = 30;
const HE_COMPRESSED_BEAMFORMING_AND_CQI_ACTION: u8 = 0;
const HE_MIMO_CONTROL_SIZE: usize = 5;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HeNdpaError {
    TooShort,
    NotNdpa,
    NotHe,
    MisalignedStationInfo,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HeNdpaEncodingError {
    OutputTooShort,
    NoStations,
    DialogTokenOutOfRange,
    AssociationIdOutOfRange,
    ResourceUnitIndexOutOfRange,
    ReversedResourceUnitRange,
    FeedbackTypeAndNgOutOfRange,
    NcOutOfRange,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HeCompressedBeamformingReportError {
    TooShort,
    NotActionNoAck,
    NotHeCompressedBeamformingAndCqi,
    MissingAverageSnr,
}

/// One FCS-stripped HE NDP Announcement MPDU.
///
/// SOURCE(esp32s31): complete pinned `libpp[wdev.o]::is_ndpa_to_dut`,
/// size `0x7e`. The blob receives an on-air length including the four-byte
/// FCS, subtracts 21 bytes, divides by four and walks station words from
/// offset 17. Removing the FCS produces the equivalent checked geometry
/// `(mpdu.len() - 17) / 4`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HeNdpa<'a> {
    mpdu: &'a [u8],
}

impl<'a> HeNdpa<'a> {
    pub fn parse(mpdu: &'a [u8]) -> Result<Self, HeNdpaError> {
        if mpdu.len() < NDPA_HEADER_SIZE + HE_STA_INFO_SIZE {
            return Err(HeNdpaError::TooShort);
        }
        let frame_control = u16::from_le_bytes([mpdu[0], mpdu[1]]);
        if frame_control & 0x00fc != NDPA_FRAME_CONTROL {
            return Err(HeNdpaError::NotNdpa);
        }
        if mpdu[16] & HE_MARKER == 0 {
            return Err(HeNdpaError::NotHe);
        }
        if !(mpdu.len() - NDPA_HEADER_SIZE).is_multiple_of(HE_STA_INFO_SIZE) {
            return Err(HeNdpaError::MisalignedStationInfo);
        }
        Ok(Self { mpdu })
    }

    pub const fn frame_control(self) -> u16 {
        u16::from_le_bytes([self.mpdu[0], self.mpdu[1]])
    }

    pub const fn duration(self) -> u16 {
        u16::from_le_bytes([self.mpdu[2], self.mpdu[3]])
    }

    pub fn receiver_address(self) -> &'a [u8] {
        &self.mpdu[4..10]
    }

    pub fn transmitter_address(self) -> &'a [u8] {
        &self.mpdu[10..16]
    }

    /// Six-bit sounding-dialog token decoded by the complete blob leaf.
    pub const fn dialog_token(self) -> u8 {
        (self.mpdu[16] >> 2) & 0x3f
    }

    pub fn stations(self) -> HeNdpaStationIterator<'a> {
        HeNdpaStationIterator {
            remaining: &self.mpdu[NDPA_HEADER_SIZE..],
        }
    }

    /// Reproduce the complete blob's local-AID membership test.
    pub fn contains_association_id(self, association_id: u16) -> bool {
        association_id <= AID11_MASK as u16
            && self
                .stations()
                .any(|station| station.association_id() == association_id)
    }
}

/// One raw four-byte HE NDPA STA Info field.
///
/// Only AID11 is named because it is the only subfield consumed by complete
/// `is_ndpa_to_dut`. The remaining 21 bits stay available as a raw word until
/// an independent blob decoder or the 802.11 specification closes them.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HeNdpaStationInfo {
    raw: u32,
}

impl HeNdpaStationInfo {
    pub const fn association_id(self) -> u16 {
        (self.raw & AID11_MASK) as u16
    }

    pub const fn raw(self) -> u32 {
        self.raw
    }

    pub const fn resource_unit_start_index(self) -> u8 {
        ((self.raw >> 11) & 0x7f) as u8
    }

    pub const fn resource_unit_end_index(self) -> u8 {
        ((self.raw >> 18) & 0x7f) as u8
    }

    pub const fn feedback_type_and_ng_encoding(self) -> u8 {
        ((self.raw >> 25) & 0x03) as u8
    }

    pub const fn disambiguation(self) -> bool {
        self.raw & (1 << 27) != 0
    }

    pub const fn codebook_size(self) -> bool {
        self.raw & (1 << 28) != 0
    }

    pub const fn nc_encoding(self) -> u8 {
        ((self.raw >> 29) & 0x07) as u8
    }
}

/// One owned HE NDPA STA Info field.
///
/// SOURCE\[HIL_VENDOR_HE20_NDPA_CBF_2026_07_24]: hardware monitor capture.
/// Frame 1374 is a complete vendor AP-to-S31 HE20 NDPA. Its STA Info word
/// `0x0820001d` selects AID 29, RU indices 0..8, feedback/Ng encoding zero,
/// disambiguation one, codebook zero and Nc zero. Frame 1376, 14.39 us later,
/// is the S31's HE Compressed Beamforming and CQI Action-No-Ack response.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HeNdpaStationEncoding {
    pub association_id: u16,
    pub resource_unit_start_index: u8,
    pub resource_unit_end_index: u8,
    pub feedback_type_and_ng_encoding: u8,
    pub disambiguation: bool,
    pub codebook_size: bool,
    pub nc_encoding: u8,
}

impl HeNdpaStationEncoding {
    pub const fn encode(self) -> Result<u32, HeNdpaEncodingError> {
        if self.association_id > AID11_MASK as u16 {
            return Err(HeNdpaEncodingError::AssociationIdOutOfRange);
        }
        if self.resource_unit_start_index > 0x7f || self.resource_unit_end_index > 0x7f {
            return Err(HeNdpaEncodingError::ResourceUnitIndexOutOfRange);
        }
        if self.resource_unit_start_index > self.resource_unit_end_index {
            return Err(HeNdpaEncodingError::ReversedResourceUnitRange);
        }
        if self.feedback_type_and_ng_encoding > 0x03 {
            return Err(HeNdpaEncodingError::FeedbackTypeAndNgOutOfRange);
        }
        if self.nc_encoding > 0x07 {
            return Err(HeNdpaEncodingError::NcOutOfRange);
        }
        Ok(self.association_id as u32
            | ((self.resource_unit_start_index as u32) << 11)
            | ((self.resource_unit_end_index as u32) << 18)
            | ((self.feedback_type_and_ng_encoding as u32) << 25)
            | ((self.disambiguation as u32) << 27)
            | ((self.codebook_size as u32) << 28)
            | ((self.nc_encoding as u32) << 29))
    }
}

/// Owned, allocation-free HE NDPA encoder input.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HeNdpaEncoding<'a> {
    pub duration: u16,
    pub receiver_address: [u8; 6],
    pub transmitter_address: [u8; 6],
    pub dialog_token: u8,
    pub stations: &'a [HeNdpaStationEncoding],
}

impl HeNdpaEncoding<'_> {
    pub const fn encoded_len(self) -> usize {
        NDPA_HEADER_SIZE + self.stations.len() * HE_STA_INFO_SIZE
    }

    pub fn encode(self, output: &mut [u8]) -> Result<usize, HeNdpaEncodingError> {
        if self.stations.is_empty() {
            return Err(HeNdpaEncodingError::NoStations);
        }
        if self.dialog_token > 0x3f {
            return Err(HeNdpaEncodingError::DialogTokenOutOfRange);
        }
        let length = self.encoded_len();
        if output.len() < length {
            return Err(HeNdpaEncodingError::OutputTooShort);
        }

        output[..length].fill(0);
        output[0..2].copy_from_slice(&NDPA_FRAME_CONTROL.to_le_bytes());
        output[2..4].copy_from_slice(&self.duration.to_le_bytes());
        output[4..10].copy_from_slice(&self.receiver_address);
        output[10..16].copy_from_slice(&self.transmitter_address);
        output[16] = (self.dialog_token << 2) | HE_MARKER;
        for (index, station) in self.stations.iter().copied().enumerate() {
            let raw = station.encode()?;
            let offset = NDPA_HEADER_SIZE + index * HE_STA_INFO_SIZE;
            output[offset..offset + HE_STA_INFO_SIZE].copy_from_slice(&raw.to_le_bytes());
        }
        Ok(length)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HeNdpaStationIterator<'a> {
    remaining: &'a [u8],
}

impl Iterator for HeNdpaStationIterator<'_> {
    type Item = HeNdpaStationInfo;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining.len() < HE_STA_INFO_SIZE {
            return None;
        }
        let raw = u32::from_le_bytes([
            self.remaining[0],
            self.remaining[1],
            self.remaining[2],
            self.remaining[3],
        ]);
        self.remaining = &self.remaining[HE_STA_INFO_SIZE..];
        Some(HeNdpaStationInfo { raw })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.remaining.len() / HE_STA_INFO_SIZE;
        (remaining, Some(remaining))
    }
}

impl ExactSizeIterator for HeNdpaStationIterator<'_> {}

/// One FCS-stripped HE Compressed Beamforming and CQI Action-No-Ack MPDU.
///
/// SOURCE\[HIL_VENDOR_HE20_NDPA_CBF_2026_07_24]: the same complete vendor
/// sounding exchange as [`HeNdpaStationEncoding`]. Frame 1376 carries HE MIMO
/// Control `0x0dc4008208`, one average-SNR byte and the complete borrowed
/// feedback matrix. This parser deliberately does not interpret matrix angles;
/// it owns only the fixed control word needed to qualify the hardware report.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HeCompressedBeamformingReport<'a> {
    mpdu: &'a [u8],
    mimo_control: u64,
    average_snr_end: usize,
}

impl<'a> HeCompressedBeamformingReport<'a> {
    pub fn parse(mpdu: &'a [u8]) -> Result<Self, HeCompressedBeamformingReportError> {
        let fixed_end = MANAGEMENT_HEADER_SIZE + 2 + HE_MIMO_CONTROL_SIZE;
        if mpdu.len() < fixed_end {
            return Err(HeCompressedBeamformingReportError::TooShort);
        }
        let frame_control = u16::from_le_bytes([mpdu[0], mpdu[1]]);
        if frame_control & 0x00fc != ACTION_NO_ACK_FRAME_CONTROL {
            return Err(HeCompressedBeamformingReportError::NotActionNoAck);
        }
        if mpdu[MANAGEMENT_HEADER_SIZE] != HE_ACTION_CATEGORY
            || mpdu[MANAGEMENT_HEADER_SIZE + 1] != HE_COMPRESSED_BEAMFORMING_AND_CQI_ACTION
        {
            return Err(HeCompressedBeamformingReportError::NotHeCompressedBeamformingAndCqi);
        }
        let control = &mpdu[MANAGEMENT_HEADER_SIZE + 2..fixed_end];
        let mimo_control = u64::from_le_bytes([
            control[0], control[1], control[2], control[3], control[4], 0, 0, 0,
        ]);
        let average_snr_end = fixed_end + ((mimo_control & 0x07) as usize + 1);
        if mpdu.len() < average_snr_end {
            return Err(HeCompressedBeamformingReportError::MissingAverageSnr);
        }
        Ok(Self {
            mpdu,
            mimo_control,
            average_snr_end,
        })
    }

    pub fn receiver_address(self) -> &'a [u8] {
        &self.mpdu[4..10]
    }

    pub fn transmitter_address(self) -> &'a [u8] {
        &self.mpdu[10..16]
    }

    pub const fn sequence_number(self) -> SequenceNumber {
        SequenceNumber::from_sequence_control(u16::from_le_bytes([self.mpdu[22], self.mpdu[23]]))
    }

    pub const fn mimo_control(self) -> u64 {
        self.mimo_control
    }

    pub const fn column_count(self) -> u8 {
        (self.mimo_control & 0x07) as u8 + 1
    }

    pub const fn row_count(self) -> u8 {
        ((self.mimo_control >> 3) & 0x07) as u8 + 1
    }

    pub const fn bandwidth_encoding(self) -> u8 {
        ((self.mimo_control >> 6) & 0x03) as u8
    }

    pub const fn grouping(self) -> bool {
        self.mimo_control & (1 << 8) != 0
    }

    pub const fn codebook_information(self) -> bool {
        self.mimo_control & (1 << 9) != 0
    }

    pub const fn feedback_type_encoding(self) -> u8 {
        ((self.mimo_control >> 10) & 0x03) as u8
    }

    pub const fn remaining_feedback_segments(self) -> u8 {
        ((self.mimo_control >> 12) & 0x07) as u8
    }

    pub const fn first_feedback_segment(self) -> bool {
        self.mimo_control & (1 << 15) != 0
    }

    pub const fn resource_unit_start_index(self) -> u8 {
        ((self.mimo_control >> 16) & 0x7f) as u8
    }

    pub const fn resource_unit_end_index(self) -> u8 {
        ((self.mimo_control >> 23) & 0x7f) as u8
    }

    pub const fn sounding_dialog_token(self) -> u8 {
        ((self.mimo_control >> 30) & 0x3f) as u8
    }

    pub const fn reserved(self) -> u8 {
        ((self.mimo_control >> 36) & 0x0f) as u8
    }

    pub fn average_snr(self) -> &'a [u8] {
        &self.mpdu[MANAGEMENT_HEADER_SIZE + 2 + HE_MIMO_CONTROL_SIZE..self.average_snr_end]
    }

    pub fn feedback_matrices(self) -> &'a [u8] {
        &self.mpdu[self.average_snr_end..]
    }
}

#[cfg(test)]
mod tests;
