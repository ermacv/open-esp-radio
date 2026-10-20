//! Allocation-free HE NDP Announcement parsing and encoding.
//!
//! Received bytes reach this module; nothing here panics on them.
#![deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::arithmetic_side_effects
)]

use crate::frame::FrameControlFields;

use crate::sequence::SequenceNumber;

const NDPA_FRAME_CONTROL: u16 = 0x0054;
const NDPA_HEADER_SIZE: usize = 17;
const HE_STA_INFO_SIZE: usize = 4;
#[cfg(test)]
const HE_MARKER: u8 = NdpaToken::from_bytes([0]).with_he(true).to_bytes()[0];
const AID11_MASK: u32 = 0x07ff;
const ACTION_NO_ACK_FRAME_CONTROL: u16 = 0x00e0;
const MANAGEMENT_HEADER_SIZE: usize = 24;
const HE_ACTION_CATEGORY: u8 = 30;
const HE_COMPRESSED_BEAMFORMING_AND_CQI_ACTION: u8 = 0;
const HE_MIMO_CONTROL_SIZE: usize = 5;
const REPORT_FIXED_SIZE: usize = MANAGEMENT_HEADER_SIZE + 2 + HE_MIMO_CONTROL_SIZE;

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
    header: &'a [u8; NDPA_HEADER_SIZE],
    stations: &'a [u8],
}

impl<'a> HeNdpa<'a> {
    pub fn parse(mpdu: &'a [u8]) -> Result<Self, HeNdpaError> {
        let mut reader = oer_wire::Reader::new(mpdu);
        let header = reader
            .array::<NDPA_HEADER_SIZE>()
            .map_err(|_| HeNdpaError::TooShort)?;
        let stations = reader.remaining();
        if stations.len() < HE_STA_INFO_SIZE {
            return Err(HeNdpaError::TooShort);
        }
        let frame_control = u16::from_le_bytes([header[0], header[1]]);
        if FrameControlFields::from_raw(frame_control).type_and_subtype()
            != FrameControlFields::from_raw(NDPA_FRAME_CONTROL).type_and_subtype()
        {
            return Err(HeNdpaError::NotNdpa);
        }
        if !NdpaToken::from_bytes([header[16]]).he() {
            return Err(HeNdpaError::NotHe);
        }
        if !stations.len().is_multiple_of(HE_STA_INFO_SIZE) {
            return Err(HeNdpaError::MisalignedStationInfo);
        }
        Ok(Self { header, stations })
    }

    pub const fn frame_control(self) -> u16 {
        u16::from_le_bytes([self.header[0], self.header[1]])
    }

    pub const fn duration(self) -> u16 {
        u16::from_le_bytes([self.header[2], self.header[3]])
    }

    pub fn receiver_address(self) -> &'a [u8] {
        &self.header[4..10]
    }

    pub fn transmitter_address(self) -> &'a [u8] {
        &self.header[10..16]
    }

    /// Six-bit sounding-dialog token decoded by the complete blob leaf.
    pub const fn dialog_token(self) -> u8 {
        NdpaToken::from_bytes([self.header[16]]).token()
    }

    pub fn stations(self) -> HeNdpaStationIterator<'a> {
        HeNdpaStationIterator {
            remaining: self.stations,
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
        NdpaStationFields::from_bytes(self.raw.to_le_bytes()).association_id()
    }

    pub const fn raw(self) -> u32 {
        self.raw
    }

    pub const fn resource_unit_start_index(self) -> u8 {
        NdpaStationFields::from_bytes(self.raw.to_le_bytes()).resource_unit_start_index()
    }

    pub const fn resource_unit_end_index(self) -> u8 {
        NdpaStationFields::from_bytes(self.raw.to_le_bytes()).resource_unit_end_index()
    }

    pub const fn feedback_type_and_ng_encoding(self) -> u8 {
        NdpaStationFields::from_bytes(self.raw.to_le_bytes()).feedback_type_and_ng_encoding()
    }

    pub const fn disambiguation(self) -> bool {
        NdpaStationFields::from_bytes(self.raw.to_le_bytes()).disambiguation()
    }

    pub const fn codebook_size(self) -> bool {
        NdpaStationFields::from_bytes(self.raw.to_le_bytes()).codebook_size()
    }

    pub const fn nc_encoding(self) -> u8 {
        NdpaStationFields::from_bytes(self.raw.to_le_bytes()).nc_encoding()
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
        let mut fields = NdpaStationFields::from_bytes([0; 4]);
        fields = crate::encoding::validated(fields.with_association_id(self.association_id));
        fields = crate::encoding::validated(
            fields.with_resource_unit_start_index(self.resource_unit_start_index),
        );
        fields = crate::encoding::validated(
            fields.with_resource_unit_end_index(self.resource_unit_end_index),
        );
        fields = crate::encoding::validated(
            fields.with_feedback_type_and_ng_encoding(self.feedback_type_and_ng_encoding),
        );
        fields = fields.with_disambiguation(self.disambiguation);
        fields = fields.with_codebook_size(self.codebook_size);
        fields = crate::encoding::validated(fields.with_nc_encoding(self.nc_encoding));
        Ok(u32::from_le_bytes(fields.to_bytes()))
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
        NDPA_HEADER_SIZE.saturating_add(self.stations.len().saturating_mul(HE_STA_INFO_SIZE))
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

        let (head, body) = output
            .split_at_mut_checked(NDPA_HEADER_SIZE)
            .ok_or(HeNdpaEncodingError::OutputTooShort)?;
        let header: [&[u8]; 5] = [
            &NDPA_FRAME_CONTROL.to_le_bytes(),
            &self.duration.to_le_bytes(),
            &self.receiver_address,
            &self.transmitter_address,
            &[crate::encoding::validated(
                NdpaToken::from_bytes([0])
                    .with_he(true)
                    .with_token(self.dialog_token),
            )
            .to_bytes()[0]],
        ];
        for (out, byte) in head.iter_mut().zip(header.into_iter().flatten()) {
            *out = *byte;
        }
        for (chunk, station) in body.chunks_exact_mut(HE_STA_INFO_SIZE).zip(self.stations) {
            chunk.copy_from_slice(&station.encode()?.to_le_bytes());
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
        let mut reader = oer_wire::Reader::new(self.remaining);
        let word = reader.array::<HE_STA_INFO_SIZE>().ok()?;
        let rest = reader.remaining();
        self.remaining = rest;
        Some(HeNdpaStationInfo {
            raw: u32::from_le_bytes(*word),
        })
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
    header: &'a [u8; MANAGEMENT_HEADER_SIZE],
    mimo_control: u64,
    average_snr: &'a [u8],
    feedback_matrices: &'a [u8],
}

impl<'a> HeCompressedBeamformingReport<'a> {
    const fn control_fields(self) -> MimoControlFields {
        let [c0, c1, c2, c3, c4, _, _, _] = self.mimo_control.to_le_bytes();
        MimoControlFields::from_bytes([c0, c1, c2, c3, c4])
    }

    pub fn parse(mpdu: &'a [u8]) -> Result<Self, HeCompressedBeamformingReportError> {
        let mut reader = oer_wire::Reader::new(mpdu);
        let fixed = reader
            .array::<REPORT_FIXED_SIZE>()
            .map_err(|_| HeCompressedBeamformingReportError::TooShort)?;
        let rest = reader.remaining();
        let mut reader = oer_wire::Reader::new(fixed);
        let header = reader
            .array::<MANAGEMENT_HEADER_SIZE>()
            .map_err(|_| HeCompressedBeamformingReportError::TooShort)?;
        let action = reader.remaining();
        let frame_control = u16::from_le_bytes([header[0], header[1]]);
        if FrameControlFields::from_raw(frame_control).type_and_subtype()
            != FrameControlFields::from_raw(ACTION_NO_ACK_FRAME_CONTROL).type_and_subtype()
        {
            return Err(HeCompressedBeamformingReportError::NotActionNoAck);
        }
        let mut reader = oer_wire::Reader::new(action);
        let &[category, action] = reader
            .array::<2>()
            .map_err(|_| HeCompressedBeamformingReportError::TooShort)?;
        let control = reader.remaining();
        let [c0, c1, c2, c3, c4] = *control
            .first_chunk::<HE_MIMO_CONTROL_SIZE>()
            .ok_or(HeCompressedBeamformingReportError::TooShort)?;
        if category != HE_ACTION_CATEGORY || action != HE_COMPRESSED_BEAMFORMING_AND_CQI_ACTION {
            return Err(HeCompressedBeamformingReportError::NotHeCompressedBeamformingAndCqi);
        }
        let mimo_control = u64::from_le_bytes([c0, c1, c2, c3, c4, 0, 0, 0]);
        let average_snr_len = (MimoControlFields::from_bytes([c0, c1, c2, c3, c4]).columns()
            as usize)
            .saturating_add(1);
        let mut reader = oer_wire::Reader::new(rest);
        let average_snr = reader
            .take(average_snr_len)
            .map_err(|_| HeCompressedBeamformingReportError::MissingAverageSnr)?;
        let feedback_matrices = reader.remaining();
        Ok(Self {
            header,
            mimo_control,
            average_snr,
            feedback_matrices,
        })
    }

    pub fn receiver_address(self) -> &'a [u8] {
        &self.header[4..10]
    }

    pub fn transmitter_address(self) -> &'a [u8] {
        &self.header[10..16]
    }

    pub const fn sequence_number(self) -> SequenceNumber {
        SequenceNumber::from_sequence_control(u16::from_le_bytes([
            self.header[22],
            self.header[23],
        ]))
    }

    pub const fn mimo_control(self) -> u64 {
        self.mimo_control
    }

    pub const fn column_count(self) -> u8 {
        (self.control_fields().columns()).saturating_add(1)
    }

    pub const fn row_count(self) -> u8 {
        (self.control_fields().rows()).saturating_add(1)
    }

    pub const fn bandwidth_encoding(self) -> u8 {
        self.control_fields().bandwidth_encoding()
    }

    pub const fn grouping(self) -> bool {
        self.control_fields().grouping()
    }

    pub const fn codebook_information(self) -> bool {
        self.control_fields().codebook_information()
    }

    pub const fn feedback_type_encoding(self) -> u8 {
        self.control_fields().feedback_type_encoding()
    }

    pub const fn remaining_feedback_segments(self) -> u8 {
        self.control_fields().remaining_feedback_segments()
    }

    pub const fn first_feedback_segment(self) -> bool {
        self.control_fields().first_feedback_segment()
    }

    pub const fn resource_unit_start_index(self) -> u8 {
        self.control_fields().resource_unit_start_index()
    }

    pub const fn resource_unit_end_index(self) -> u8 {
        self.control_fields().resource_unit_end_index()
    }

    pub const fn sounding_dialog_token(self) -> u8 {
        self.control_fields().sounding_dialog_token()
    }

    pub const fn reserved(self) -> u8 {
        self.control_fields().reserved()
    }

    pub fn average_snr(self) -> &'a [u8] {
        self.average_snr
    }

    pub fn feedback_matrices(self) -> &'a [u8] {
        self.feedback_matrices
    }
}

oer_wire::bitfield! {
    /// SOURCE: IEEE Std 802.11ax-2021, 9.3.1.19 (NDP Announcement frame).
    pub struct NdpaStationFields[4] (little) {
        pub association_id, with_association_id: u16 = 0..11;
        pub resource_unit_start_index, with_resource_unit_start_index: u8 = 11..18;
        pub resource_unit_end_index, with_resource_unit_end_index: u8 = 18..25;
        pub feedback_type_and_ng_encoding, with_feedback_type_and_ng_encoding: u8 = 25..27;
        pub disambiguation, with_disambiguation: bool = 27..28;
        pub codebook_size, with_codebook_size: bool = 28..29;
        pub nc_encoding, with_nc_encoding: u8 = 29..32;
    }
}
oer_wire::bitfield! {
    /// SOURCE: IEEE Std 802.11ax-2021, 9.3.1.19 (Sounding Dialog Token field).
    pub struct NdpaToken[1] (little) {
        pub he, with_he: bool = 1..2;
        pub token, with_token: u8 = 2..8;
    }
}
oer_wire::bitfield! {
    /// SOURCE: IEEE Std 802.11ax-2021, 9.4.1.64 (HE MIMO Control field).
    pub struct MimoControlFields[5] (little) {
        pub columns, with_columns: u8 = 0..3;
        pub rows, with_rows: u8 = 3..6;
        pub bandwidth_encoding, with_bandwidth_encoding: u8 = 6..8;
        pub grouping, with_grouping: bool = 8..9;
        pub codebook_information, with_codebook_information: bool = 9..10;
        pub feedback_type_encoding, with_feedback_type_encoding: u8 = 10..12;
        pub remaining_feedback_segments, with_remaining_feedback_segments: u8 = 12..15;
        pub first_feedback_segment, with_first_feedback_segment: bool = 15..16;
        pub resource_unit_start_index, with_resource_unit_start_index: u8 = 16..23;
        pub resource_unit_end_index, with_resource_unit_end_index: u8 = 23..30;
        pub sounding_dialog_token, with_sounding_dialog_token: u8 = 30..36;
        pub reserved, with_reserved: u8 = 36..40;
    }
}

#[cfg(test)]
mod tests;
