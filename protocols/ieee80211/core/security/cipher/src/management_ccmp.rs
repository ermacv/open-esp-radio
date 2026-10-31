//! Stateless validation and decryption of protected management MPDUs.

#![deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::arithmetic_side_effects
)]

use crate::ccmp_mpdu::{CcmpAssociation, CcmpMpduError};
use oer_ieee80211_contracts_security::Ccmp128Key;
use oer_ieee80211_wire::{
    address::AddressFlags,
    ccmp::protected::{CCMP_MIC_LEN, ProtectedMpdu, ProtectedMpduError},
    ccmp::{CCMP_HEADER_LEN, CcmpHeaderError, CcmpKeyId, CcmpPacketNumber},
    frame::DecodedHeader,
    management::MANAGEMENT_HEADER_LEN,
    security::Spp,
};
/// Why a protected management frame did not open.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ManagementCcmpError {
    /// The MAC header is malformed or does not describe management traffic.
    MalformedHeader,
    /// The CCMP header has reserved bits or bytes set.
    InvalidCcmpHeader,
    /// Individually addressed management uses only the pairwise KeyID zero.
    UnexpectedKeyId,
    /// Group-addressed robust management uses BIP rather than CCMP.
    GroupAddressed,
    /// Shorter than a header, CCMP header and MIC.
    Truncated,
    /// Beyond the two-byte CCM length field.
    TooLong,
    /// The Protected Frame bit is clear.
    NotProtected,
    /// The CCMP header lacks its Extended IV bit.
    MissingExtendedIv,
    /// The MIC does not verify.
    InvalidMic,
}

/// One validated management MPDU; the caller owns its replay decision.
pub struct ManagementCcmpFrame<'a> {
    frame: &'a mut [u8],
    packet_number: CcmpPacketNumber,
}

impl<'a> ManagementCcmpFrame<'a> {
    pub fn parse(frame: &'a mut [u8]) -> Result<Self, ManagementCcmpError> {
        // Room for a management header, the CCMP header and the MIC comes
        // first; a frame of that length whose header does not decode is
        // malformed, not short.
        if frame.len() < MANAGEMENT_HEADER_LEN + CCMP_HEADER_LEN + CCMP_MIC_LEN {
            return Err(ManagementCcmpError::Truncated);
        }
        let header = DecodedHeader::decode(frame).ok_or(ManagementCcmpError::MalformedHeader)?;
        if !header.control().is_management_frame() {
            return Err(ManagementCcmpError::MalformedHeader);
        }
        let [first, ..] = header.receiver();
        if AddressFlags::from_bytes([first]).group() {
            return Err(ManagementCcmpError::GroupAddressed);
        }
        let mpdu = ProtectedMpdu::parse(frame).map_err(|error| match error {
            ProtectedMpduError::MalformedHeader => ManagementCcmpError::MalformedHeader,
            ProtectedMpduError::NotProtected => ManagementCcmpError::NotProtected,
            ProtectedMpduError::Truncated => ManagementCcmpError::Truncated,
            ProtectedMpduError::TooLong => ManagementCcmpError::TooLong,
            ProtectedMpduError::InvalidCcmpHeader(CcmpHeaderError::ExtIvMissing) => {
                ManagementCcmpError::MissingExtendedIv
            }
            ProtectedMpduError::InvalidCcmpHeader(
                CcmpHeaderError::ReservedOctet | CcmpHeaderError::ReservedKeyBits,
            ) => ManagementCcmpError::InvalidCcmpHeader,
        })?;
        let ccmp = mpdu.ccmp_header();
        if ccmp.key_id() != CcmpKeyId::PAIRWISE {
            return Err(ManagementCcmpError::UnexpectedKeyId);
        }
        let packet_number = ccmp.packet_number();
        Ok(Self {
            frame,
            packet_number,
        })
    }

    pub const fn packet_number(&self) -> CcmpPacketNumber {
        self.packet_number
    }

    pub fn open(self, temporal_key: &Ccmp128Key) -> Result<&'a [u8], ManagementCcmpError> {
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
