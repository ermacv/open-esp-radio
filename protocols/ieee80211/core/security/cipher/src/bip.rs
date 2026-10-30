//! Stateless BIP-CMAC-128 management-frame integrity transforms.

#![deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::arithmetic_side_effects
)]

use aes::Aes128;
use cmac::{Cmac, Mac, digest::KeyInit};
use oer_ieee80211_wire::bip::{BIP_MIC_LEN, BipFormatError, BipProtection};

const RSN_IGTK_LEN: usize = 16;

/// Why a group-addressed management frame failed BIP.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BipError {
    /// The frame is shorter than a management header and its MIC element.
    Truncated,
    /// The frame does not end with a BIP-CMAC-128 Management MIC element.
    MissingManagementMic,
    /// The MIC element names another IGTK.
    UnknownKeyId,
    /// The MIC does not verify.
    InvalidMic,
}

/// A parsed BIP frame of the expected IGTK; replay admission remains with
/// the caller.
pub struct BipFrame<'a>(oer_ieee80211_wire::bip::BipFrame<'a>);

impl<'a> BipFrame<'a> {
    pub fn parse(frame: &'a [u8], key_id: u8) -> Result<Self, BipError> {
        let frame =
            oer_ieee80211_wire::bip::BipFrame::parse(frame).map_err(|error| match error {
                BipFormatError::MissingManagementMic => BipError::MissingManagementMic,
                BipFormatError::Truncated | BipFormatError::PacketNumberOutOfRange => {
                    BipError::Truncated
                }
            })?;
        if frame.key_id() != u16::from(key_id) {
            return Err(BipError::UnknownKeyId);
        }
        Ok(Self(frame))
    }

    pub const fn packet_number(&self) -> u64 {
        self.0.packet_number()
    }

    pub fn verify(self, key: &[u8; RSN_IGTK_LEN]) -> Result<(), BipError> {
        let mut mac = cmac(key);
        for part in self.0.mic_input() {
            mac.update(part);
        }
        mac.verify_truncated_left(self.0.mic())
            .map_err(|_| BipError::InvalidMic)?;
        Ok(())
    }
}

/// Protect a frame with the caller's IPN, without allocating or advancing it.
pub fn protect(
    key_id: u8,
    key: &[u8; RSN_IGTK_LEN],
    packet_number: u64,
    frame: &mut [u8],
    length: usize,
) -> Option<usize> {
    let protection = BipProtection::append(frame, length, u16::from(key_id), packet_number).ok()?;
    let mut mac = cmac(key);
    for part in protection.mic_input() {
        mac.update(part);
    }
    let tag = mac.finalize().into_bytes();
    let mut mic = [0; BIP_MIC_LEN];
    for (out, byte) in mic.iter_mut().zip(tag) {
        *out = byte;
    }
    Some(protection.finish(&mic))
}

/// AES-128-CMAC over a 16-byte IGTK.
fn cmac(key: &[u8; RSN_IGTK_LEN]) -> Cmac<Aes128> {
    <Cmac<Aes128> as KeyInit>::new(&(*key).into())
}
