//! Software CCMP of individually addressed robust management frames.
//!
//! With management frame protection, a Deauthentication, Disassociation or
//! robust Action frame to the station arrives CCMP-protected under the
//! pairwise temporal key. The vendor opens these frames in software with its
//! own copy of the key (`ieee80211_ccmp_decrypt` through wpa_supplicant's
//! `ccmp_decrypt`), so the station keeps the temporal key for as long as its
//! association protects management frames.
//!
//! The nonce of a management frame sets the Management flag with priority
//! zero, its additional authentication data keeps the Frame Control subtype,
//! and its packet numbers form one replay counter apart from every data TID.
//!
//! SOURCE(esp32s31): IEEE 802.11-2016 12.5.3 (CCMP); complete pinned
//! `libnet80211.a[ieee80211_sta.o]::sta_recv_mgmt` and
//! `libnet80211.a[ieee80211_crypto_ccmp.o]::ieee80211_ccmp_decrypt`.
//!
//! At esp32-wifi-lib revision `af55a0ca258ce9d791d1661d7c2bbb65f08c0c21`,
//! the latter function requires Extended IV at offsets `0x58..0x60`, then
//! rejects a nonzero reserved CCMP octet (`ccmp[2]`) or any reserved Key ID
//! bits (`ccmp[3] & 0x1f`) at `0x98..0xc6`. Either violation returns `-1`
//! before replay checking or decryption. [`CcmpHeader::parse`] enforces the
//! same header requirements for protected management reception.

use crate::ccm::MIC_LEN;
use crate::ccmp_mpdu::{CcmpAssociation, CcmpMpduError};
use oer_ieee80211_mac::{
    ccmp::{CCMP_HEADER_LEN, Ccmp128Key, CcmpHeader, CcmpHeaderError},
    frame::DecodedHeader,
    security::Spp,
};
use zeroize::{Zeroize, ZeroizeOnDrop};

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

/// The receive state of protected management frames under one pairwise key.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct ManagementCcmpReceiver {
    temporal_key: Ccmp128Key,
    last_packet_number: Option<u64>,
}

impl ManagementCcmpReceiver {
    /// Copy from the association's zeroizing key owner directly into this
    /// zeroizing software receiver, without a by-value array argument.
    pub fn new(temporal_key: &[u8; 16]) -> Self {
        Self {
            temporal_key: Ccmp128Key::new(temporal_key),
            last_packet_number: None,
        }
    }

    /// Decrypt one received protected management frame in place and return
    /// its plaintext body. A frame that does not verify leaves the replay
    /// state unchanged. MIC failure erases tentative plaintext through the
    /// shared `CcmpAssociation` transform; this wrapper only owns replay.
    pub fn open<'a>(&mut self, frame: &'a mut [u8]) -> Result<&'a [u8], ManagementCcmpError> {
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
        if self
            .last_packet_number
            .is_some_and(|last| packet_number <= last)
        {
            return Err(ManagementCcmpError::Replay);
        }

        // SPP only affects QoS data; management uses the same transform in
        // either association mode. The receiver owns its separate PN frontier.
        let body = CcmpAssociation::new(Spp::NotNegotiated)
            .open(self.temporal_key.as_bytes(), frame)
            .map_err(ManagementCcmpError::from)?;
        self.last_packet_number = Some(packet_number);
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

#[cfg(test)]
mod tests;
