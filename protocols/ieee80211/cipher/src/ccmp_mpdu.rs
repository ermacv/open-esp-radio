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
//!
//! Received frames reach this module; nothing here panics on them.
#![deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::arithmetic_side_effects
)]

use crate::ccm::{self, MIC_LEN, NONCE_LEN};
use oer_ieee80211_contracts_security::Ccmp128Key;
use oer_ieee80211_wire::{ccmp::CcmpHeader, frame::DecodedHeader, security::Spp};
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
    pub fn seal(&self, key: &Ccmp128Key, frame: &mut [u8]) -> Result<(), CcmpMpduError> {
        let blocks = self.blocks(frame)?;
        let (_, body_and_mic) = frame
            .split_at_mut_checked(blocks.body_start)
            .ok_or(CcmpMpduError::Truncated)?;
        let (body, mic) = body_and_mic
            .split_at_mut_checked(blocks.body_len)
            .ok_or(CcmpMpduError::Truncated)?;
        let tag = ccm::seal(key.as_bytes(), &blocks.nonce, &blocks.aad, body)
            .map_err(|_| CcmpMpduError::TooLong)?;
        for (out, byte) in mic.iter_mut().zip(tag) {
            *out = byte;
        }
        Ok(())
    }

    /// Verify and decrypt. Failed authentication erases the tentative plaintext;
    /// a caller must discard the frame and leave its replay frontier unchanged.
    pub fn open<'a>(
        &self,
        key: &Ccmp128Key,
        frame: &'a mut [u8],
    ) -> Result<&'a [u8], CcmpMpduError> {
        let blocks = self.blocks(frame)?;
        let (_, body_and_mic) = frame
            .split_at_mut_checked(blocks.body_start)
            .ok_or(CcmpMpduError::Truncated)?;
        let (body, mic) = body_and_mic
            .split_at_mut_checked(blocks.body_len)
            .ok_or(CcmpMpduError::Truncated)?;
        let mic = mic
            .first_chunk::<MIC_LEN>()
            .ok_or(CcmpMpduError::Truncated)?;
        if ccm::open(key.as_bytes(), &blocks.nonce, &blocks.aad, body, mic).is_err() {
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
        let [a0, a1, a2, a3, a4, a5] = header.transmitter();
        let [_, _, p0, p1, p2, p3, p4, p5] = ccmp.packet_number().value().to_be_bytes();
        let nonce = [flags, a0, a1, a2, a3, a4, a5, p0, p1, p2, p3, p4, p5];
        Ok(Blocks {
            aad,
            nonce,
            body_len,
            body_start: frame
                .len()
                .checked_sub(protected.len())
                .ok_or(CcmpMpduError::Truncated)?,
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

        // Mutable subtype bits are masked only for data, never management.
        let masked = if control.is_data() { 0x3870 } else { 0x3800 };
        let mut fc = control.get() & !masked;
        if control.is_qos_data() {
            fc &= !0x8000;
        }
        let [f0, f1] = fc.to_le_bytes();
        let [r0, r1, r2, r3, r4, r5] = header.receiver();
        let [t0, t1, t2, t3, t4, t5] = header.transmitter();
        let [a0, a1, a2, a3, a4, a5] = header.address3();
        let base = [
            f0,
            f1,
            r0,
            r1,
            r2,
            r3,
            r4,
            r5,
            t0,
            t1,
            t2,
            t3,
            t4,
            t5,
            a0,
            a1,
            a2,
            a3,
            a4,
            a5,
            (header.sequence_control() & 15) as u8,
            0,
        ];
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
                for (out, byte) in bytes.iter_mut().zip(base.into_iter().chain([qos, 0])) {
                    *out = byte;
                }
                AadBytes::ThreeAddressQos(bytes)
            }
            (Some(address), None) => {
                let mut bytes = [0; 28];
                for (out, byte) in bytes.iter_mut().zip(base.into_iter().chain(address)) {
                    *out = byte;
                }
                AadBytes::FourAddress(bytes)
            }
            (Some(address), Some(qos)) => {
                let mut bytes = [0; 30];
                for (out, byte) in bytes
                    .iter_mut()
                    .zip(base.into_iter().chain(address).chain([qos, 0]))
                {
                    *out = byte;
                }
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
