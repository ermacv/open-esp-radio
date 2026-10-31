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
use oer_ieee80211_wire::ccmp::protected::{CcmpAad, ProtectedMpdu, ProtectedMpduError};
use oer_ieee80211_wire::security::Spp;
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
        let mpdu = ProtectedMpdu::parse(frame).map_err(|error| match error {
            ProtectedMpduError::MalformedHeader => CcmpMpduError::MalformedHeader,
            ProtectedMpduError::NotProtected => CcmpMpduError::NotProtected,
            ProtectedMpduError::InvalidCcmpHeader(_) => CcmpMpduError::InvalidCcmpHeader,
            ProtectedMpduError::Truncated => CcmpMpduError::Truncated,
            ProtectedMpduError::TooLong => CcmpMpduError::TooLong,
        })?;
        let (body_start, body_len) = mpdu.body_range();
        Ok(Blocks {
            aad: mpdu.aad(self.spp).ok_or(CcmpMpduError::MalformedHeader)?,
            nonce: mpdu.nonce(self.spp),
            body_start,
            body_len,
        })
    }
}

struct Blocks {
    aad: CcmpAad,
    nonce: [u8; NONCE_LEN],
    body_start: usize,
    body_len: usize,
}

#[cfg(test)]
mod tests;
