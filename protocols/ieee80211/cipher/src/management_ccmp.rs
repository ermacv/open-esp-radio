//! Stateless validation and decryption of protected management MPDUs.

#![deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::arithmetic_side_effects
)]

use crate::ccm::MIC_LEN;
use crate::ccmp_mpdu::{CcmpAssociation, CcmpMpduError};
use oer_ieee80211_wire::{
    ccmp::{CCMP_HEADER_LEN, CcmpHeader, CcmpHeaderError},
    frame::DecodedHeader,
    security::Spp,
};

const HEADER_LEN: usize = 24;
/// Why a protected management frame did not open.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ManagementCcmpError {
    /// The MAC header is malformed or does not describe management traffic.
    MalformedHeader,
    /// The CCMP header has reserved bits or bytes set.
    InvalidCcmpHeader,
    /// Shorter than a header, CCMP header and MIC.
    Truncated,
    /// Beyond the two-byte CCM length field.
    TooLong,
    /// The Protected Frame bit is clear.
    NotProtected,
    /// The CCMP header lacks its Extended IV bit.
    MissingExtendedIv,
    /// The packet number does not exceed the last accepted one.
    Replay,
    /// The MIC does not verify.
    InvalidMic,
}

/// One validated management MPDU; the caller owns its replay decision.
pub struct ManagementCcmpFrame<'a> {
    frame: &'a mut [u8],
    packet_number: u64,
}

impl<'a> ManagementCcmpFrame<'a> {
    pub fn parse(frame: &'a mut [u8]) -> Result<Self, ManagementCcmpError> {
        if frame.len() < HEADER_LEN + CCMP_HEADER_LEN + MIC_LEN {
            return Err(ManagementCcmpError::Truncated);
        }
        let header = DecodedHeader::decode(frame).ok_or(ManagementCcmpError::MalformedHeader)?;
        if header.control().is_data() {
            return Err(ManagementCcmpError::MalformedHeader);
        }
        if !header.control().protected() {
            return Err(ManagementCcmpError::NotProtected);
        }
        let ccmp = header
            .body()
            .first_chunk::<CCMP_HEADER_LEN>()
            .ok_or(ManagementCcmpError::Truncated)?;
        let packet_number = CcmpHeader::parse(*ccmp)
            .map_err(|error| match error {
                CcmpHeaderError::ExtIvMissing => ManagementCcmpError::MissingExtendedIv,
                CcmpHeaderError::ReservedOctet | CcmpHeaderError::ReservedKeyBits => {
                    ManagementCcmpError::InvalidCcmpHeader
                }
            })?
            .packet_number()
            .value();
        Ok(Self {
            frame,
            packet_number,
        })
    }

    pub const fn packet_number(&self) -> u64 {
        self.packet_number
    }

    pub fn open(self, temporal_key: &[u8; 16]) -> Result<&'a [u8], ManagementCcmpError> {
        let frame = self.frame;
        // SPP only affects QoS data; management uses the same transform in
        // either association mode. The receiver owns its separate PN frontier.
        let body = CcmpAssociation::new(Spp::NotNegotiated)
            .open(temporal_key, frame)
            .map_err(ManagementCcmpError::from)?;
        Ok(body)
    }
}

impl From<CcmpMpduError> for ManagementCcmpError {
    fn from(error: CcmpMpduError) -> Self {
        match error {
            CcmpMpduError::MalformedHeader => Self::MalformedHeader,
            CcmpMpduError::NotProtected => Self::NotProtected,
            CcmpMpduError::InvalidCcmpHeader => Self::InvalidCcmpHeader,
            CcmpMpduError::Truncated => Self::Truncated,
            CcmpMpduError::TooLong => Self::TooLong,
            CcmpMpduError::InvalidMic => Self::InvalidMic,
        }
    }
}
