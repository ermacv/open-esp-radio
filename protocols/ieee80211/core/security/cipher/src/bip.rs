//! Stateless BIP-CMAC-128 management-frame integrity transforms.

use oer_portable_crypto::{
    Aes128Key,
    block::{Aes128Engine, BlockError},
    cmac::Cmac,
};
use oer_protocols_ieee80211_wire::bip::{BIP_MIC_LEN, BipFormatError, BipProtection};
use zeroize::Zeroize;

const RSN_IGTK_LEN: usize = 16;

/// Why a group-addressed management frame failed BIP.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BipError {
    /// The frame is shorter than a management header and its MIC element.
    Truncated,
    /// The header is not that of a protocol-version-0 management frame.
    NotManagement,
    /// The frame does not end with a BIP-CMAC-128 Management MIC element.
    MissingManagementMic,
    /// The MIC element names another IGTK.
    UnknownKeyId,
    /// The MIC does not verify.
    InvalidMic,
    /// The AES engine refused the key or a block.
    Engine(BlockError),
}

/// Why a group-addressed management frame was not protected.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BipProtectError {
    /// The length or the output capacity cannot hold the frame and its MMIE.
    InvalidFrame,
    /// The AES engine refused the key or a block. The frame's first `length`
    /// bytes are unchanged; the bytes after them are not a valid MMIE.
    Engine(BlockError),
}

/// A parsed BIP frame of the expected IGTK; replay admission remains with
/// the caller.
pub struct BipFrame<'a>(oer_protocols_ieee80211_wire::bip::BipFrame<'a>);

impl<'a> BipFrame<'a> {
    pub fn parse(frame: &'a [u8], key_id: u8) -> Result<Self, BipError> {
        let frame =
            oer_protocols_ieee80211_wire::bip::BipFrame::parse(frame).map_err(
                |error| match error {
                    BipFormatError::MissingManagementMic => BipError::MissingManagementMic,
                    BipFormatError::NotManagement => BipError::NotManagement,
                    BipFormatError::Truncated => BipError::Truncated,
                },
            )?;
        if frame.key_id() != u16::from(key_id) {
            return Err(BipError::UnknownKeyId);
        }
        Ok(Self(frame))
    }

    /// The frame's IPN, as its six-octet field holds it.
    pub const fn packet_number(&self) -> oer_portable_wire::U64<48> {
        self.0.packet_number()
    }

    pub fn verify<E: Aes128Engine>(
        self,
        engine: &mut E,
        key: &[u8; RSN_IGTK_LEN],
    ) -> Result<(), BipError> {
        let key = Aes128Key::copy_from(key);
        let keyed = engine.load(&key).map_err(BipError::Engine)?;
        drop(key);
        let mut mac = Cmac::new(keyed).map_err(BipError::Engine)?;
        for part in self.0.mic_input() {
            mac.update(part).map_err(BipError::Engine)?;
        }
        if mac
            .verify_truncated(self.0.mic())
            .map_err(BipError::Engine)?
        {
            Ok(())
        } else {
            Err(BipError::InvalidMic)
        }
    }
}

/// Protect a frame with the caller's IPN, without allocating or advancing it.
pub fn protect<E: Aes128Engine>(
    engine: &mut E,
    key_id: u8,
    key: &[u8; RSN_IGTK_LEN],
    packet_number: oer_portable_wire::U64<48>,
    frame: &mut [u8],
    length: usize,
) -> Result<usize, BipProtectError> {
    let protection = BipProtection::append(frame, length, u16::from(key_id), packet_number)
        .map_err(|_| BipProtectError::InvalidFrame)?;
    let key = Aes128Key::copy_from(key);
    let keyed = engine.load(&key).map_err(BipProtectError::Engine)?;
    drop(key);
    let mut mac = Cmac::new(keyed).map_err(BipProtectError::Engine)?;
    for part in protection.mic_input() {
        mac.update(part).map_err(BipProtectError::Engine)?;
    }
    let mut tag = mac.finalize().map_err(BipProtectError::Engine)?;
    let mut mic = [0; BIP_MIC_LEN];
    for (out, byte) in mic.iter_mut().zip(tag) {
        *out = byte;
    }
    tag.zeroize();
    Ok(protection.finish(&mic))
}
