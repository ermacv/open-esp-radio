//! Software CCMP for data and management, including negotiated SPP A-MSDU.
//!
//! The association owns the SPP decision; individual keys do not select it.
//! The transform consumes the same checked MAC geometry as hardware-backed RX.
//! It does not allocate packet numbers or commit replay: those belong to the
//! installed key's separate TX/RX owners.
//!
//! IEEE 802.11 CCMP AAD masks mutable FC/sequence/QoS fields. With SPP, the
//! QoS A-MSDU-present bit joins TID in AAD and nonce flags. This follows
//! [Linux v6.12 `ccmp_gcmp_aad` / `ccmp_special_blocks`](https://github.com/torvalds/linux/blob/v6.12/net/mac80211/wpa.c).

use crate::ccm::{self, MIC_LEN, NONCE_LEN};
use aes::{
    Aes128,
    cipher::{KeyInit, generic_array::GenericArray},
};
use oer_ieee80211_mac::{ccmp::CcmpHeader, frame::DecodedHeader, security::Spp};
use subtle::ConstantTimeEq;
use zeroize::Zeroize;

/// Why a protected data or management MPDU cannot be authenticated.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CcmpMpduError {
    MalformedHeader,
    NotProtected,
    InvalidCcmpHeader,
    Truncated,
    TooLong,
    InvalidMic,
}

/// Immutable authentication policy of one association, shared by PTK and GTK.
/// It contains no key material. Hardware/model installation retains this
/// record while every installed key refers to the same association owner.
#[derive(Debug)]
pub struct CcmpAssociation {
    spp: Spp,
}

impl CcmpAssociation {
    pub const fn new(spp: Spp) -> Self {
        Self { spp }
    }
    pub const fn spp(&self) -> Spp {
        self.spp
    }

    /// Encrypt an MPDU in place. The caller has already allocated its PN and
    /// encoded the CCMP header, and reserved eight trailing bytes for the MIC.
    pub fn seal(&self, key: &[u8; 16], frame: &mut [u8]) -> Result<(), CcmpMpduError> {
        let blocks = self.blocks(frame)?;
        let (_, body_and_mic) = frame.split_at_mut(blocks.body_start);
        let (body, mic) = body_and_mic.split_at_mut(blocks.body_len);
        let cipher = Aes128::new(GenericArray::from_slice(key));
        mic.copy_from_slice(&ccm::authenticate(
            &cipher,
            &blocks.nonce,
            &blocks.aad,
            body,
        ));
        ccm::crypt(&cipher, &blocks.nonce, body);
        Ok(())
    }

    /// Verify and decrypt. Failed authentication erases the tentative plaintext;
    /// a caller must discard the frame and leave its replay frontier unchanged.
    pub fn open<'a>(&self, key: &[u8; 16], frame: &'a mut [u8]) -> Result<&'a [u8], CcmpMpduError> {
        let blocks = self.blocks(frame)?;
        let (_, body_and_mic) = frame.split_at_mut(blocks.body_start);
        let (body, mic) = body_and_mic.split_at_mut(blocks.body_len);
        let cipher = Aes128::new(GenericArray::from_slice(key));
        ccm::crypt(&cipher, &blocks.nonce, body);
        let mut expected = ccm::authenticate(&cipher, &blocks.nonce, &blocks.aad, body);
        let valid = bool::from(expected.ct_eq(mic));
        expected.zeroize();
        if !valid {
            body.zeroize();
            return Err(CcmpMpduError::InvalidMic);
        }
        Ok(body)
    }

    fn blocks(&self, frame: &[u8]) -> Result<Blocks, CcmpMpduError> {
        let header = DecodedHeader::decode(frame).ok_or(CcmpMpduError::MalformedHeader)?;
        let control = header.control();
        if !control.protected() {
            return Err(CcmpMpduError::NotProtected);
        }
        let (ccmp, protected) = header
            .body()
            .split_first_chunk::<8>()
            .ok_or(CcmpMpduError::Truncated)?;
        let ccmp = CcmpHeader::parse(*ccmp).map_err(|_| CcmpMpduError::InvalidCcmpHeader)?;
        let body_len = protected
            .len()
            .checked_sub(MIC_LEN)
            .ok_or(CcmpMpduError::Truncated)?;
        u16::try_from(body_len).map_err(|_| CcmpMpduError::TooLong)?;
        let (aad, flags) = Aad::from_header(&header, self.spp);
        let mut nonce = [0; NONCE_LEN];
        nonce[0] = flags;
        nonce[1..7].copy_from_slice(&header.transmitter());
        nonce[7..].copy_from_slice(&ccmp.packet_number().value().to_be_bytes()[2..]);
        Ok(Blocks {
            aad,
            nonce,
            body_len,
            body_start: frame.len() - protected.len(),
        })
    }
}

/// Only checked MAC-header builders can create CCM additional data. The closed
/// wire shapes make AAD beyond the two CCM blocks (30 bytes) unrepresentable.
pub(crate) struct Aad(AadBytes);

enum AadBytes {
    ThreeAddress([u8; 22]),
    ThreeAddressQos([u8; 24]),
    FourAddress([u8; 28]),
    FourAddressQos([u8; 30]),
}

impl Aad {
    fn from_header(header: &DecodedHeader<'_>, spp: Spp) -> (Self, u8) {
        let control = header.control();
        let mut base = [0; 22];
        // Mutable subtype bits are masked only for data, never management.
        let masked = if control.is_data() { 0x3870 } else { 0x3800 };
        let mut fc = control.get() & !masked;
        if control.is_qos_data() {
            fc &= !0x8000;
        }
        base[..2].copy_from_slice(&fc.to_le_bytes());
        base[2..8].copy_from_slice(&header.receiver());
        base[8..14].copy_from_slice(&header.transmitter());
        base[14..20].copy_from_slice(&header.address3());
        base[20] = (header.sequence_control() & 15) as u8;
        let qos = header.qos().map(|qos| {
            (qos as u8)
                & match spp {
                    Spp::Negotiated => 0x8f,
                    Spp::NotNegotiated => 0x0f,
                }
        });
        let flags = qos.unwrap_or(if control.is_data() { 0 } else { 0x10 });
        let bytes = match (header.address4(), qos) {
            (None, None) => AadBytes::ThreeAddress(base),
            (None, Some(qos)) => {
                let mut bytes = [0; 24];
                bytes[..22].copy_from_slice(&base);
                bytes[22] = qos;
                AadBytes::ThreeAddressQos(bytes)
            }
            (Some(address), None) => {
                let mut bytes = [0; 28];
                bytes[..22].copy_from_slice(&base);
                bytes[22..].copy_from_slice(&address);
                AadBytes::FourAddress(bytes)
            }
            (Some(address), Some(qos)) => {
                let mut bytes = [0; 30];
                bytes[..22].copy_from_slice(&base);
                bytes[22..28].copy_from_slice(&address);
                bytes[28] = qos;
                AadBytes::FourAddressQos(bytes)
            }
        };
        (Self(bytes), flags)
    }

    pub(crate) fn bytes(&self) -> &[u8] {
        match &self.0 {
            AadBytes::ThreeAddress(bytes) => bytes,
            AadBytes::ThreeAddressQos(bytes) => bytes,
            AadBytes::FourAddress(bytes) => bytes,
            AadBytes::FourAddressQos(bytes) => bytes,
        }
    }
}

struct Blocks {
    aad: Aad,
    nonce: [u8; NONCE_LEN],
    body_start: usize,
    body_len: usize,
}

#[cfg(test)]
mod tests;

#[cfg(test)]
pub(crate) mod test_vector;
