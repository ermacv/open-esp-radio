//! Validated borrowed and owned RSN EAPOL-Key packets and MIC verification.

use crate::AkmKeys;
use crate::{Akm, Ptk, RSN_KCK_LEN, RsnInterface, RsnKeyConfirmationKey};

pub const EAPOL_HEADER_LEN: usize = 4;
pub const EAPOL_KEY_FIXED_LEN: usize = 95;
pub const EAPOL_KEY_PACKET_LEN: usize = EAPOL_HEADER_LEN + EAPOL_KEY_FIXED_LEN;
pub const EAPOL_PACKET_TYPE_KEY: u8 = 3;
pub const RSN_KEY_DESCRIPTOR_TYPE: u8 = 2;
pub const DEFAULT_EAPOL_FRAME_CAPACITY: usize = 512;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EapolCopyError {
    CapacityExceeded,
    Invalid(EapolParseError),
}

const KEY_INFO_DESCRIPTOR_VERSION_MASK: u16 = 0x0007;
const KEY_INFO_PAIRWISE: u16 = 1 << 3;
const KEY_INFO_INSTALL: u16 = 1 << 6;
const KEY_INFO_ACK: u16 = 1 << 7;
const KEY_INFO_MIC: u16 = 1 << 8;
const KEY_INFO_SECURE: u16 = 1 << 9;
const KEY_INFO_ERROR: u16 = 1 << 10;
const KEY_INFO_REQUEST: u16 = 1 << 11;
const KEY_INFO_ENCRYPTED_KEY_DATA: u16 = 1 << 12;
const KEY_INFO_SMK: u16 = 1 << 13;

const EAPOL_KEY_MIC_START: usize = 81;
const EAPOL_KEY_MIC_END: usize = 97;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EapolParseError {
    Truncated,
    LengthMismatch,
    NotKeyPacket,
    NotRsnKeyDescriptor,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EapolKeyMessage {
    PairwiseMessage1,
    PairwiseMessage2,
    PairwiseMessage3,
    PairwiseMessage4,
    GroupMessage1,
    GroupMessage2,
    Other,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EapolKeyInfo(u16);

impl EapolKeyInfo {
    pub const fn raw(self) -> u16 {
        self.0
    }

    pub const fn descriptor_version(self) -> u8 {
        (self.0 & KEY_INFO_DESCRIPTOR_VERSION_MASK) as u8
    }

    pub const fn is_pairwise(self) -> bool {
        self.0 & KEY_INFO_PAIRWISE != 0
    }

    pub const fn install(self) -> bool {
        self.0 & KEY_INFO_INSTALL != 0
    }

    pub const fn ack(self) -> bool {
        self.0 & KEY_INFO_ACK != 0
    }

    pub const fn mic(self) -> bool {
        self.0 & KEY_INFO_MIC != 0
    }

    pub const fn secure(self) -> bool {
        self.0 & KEY_INFO_SECURE != 0
    }

    pub const fn error(self) -> bool {
        self.0 & KEY_INFO_ERROR != 0
    }

    pub const fn request(self) -> bool {
        self.0 & KEY_INFO_REQUEST != 0
    }

    pub const fn encrypted_key_data(self) -> bool {
        self.0 & KEY_INFO_ENCRYPTED_KEY_DATA != 0
    }

    pub const fn smk(self) -> bool {
        self.0 & KEY_INFO_SMK != 0
    }

    pub const fn classify(self) -> EapolKeyMessage {
        if self.error() || self.request() || self.smk() {
            return EapolKeyMessage::Other;
        }

        if self.is_pairwise() {
            match (self.ack(), self.mic(), self.install(), self.secure()) {
                (true, false, false, false) => EapolKeyMessage::PairwiseMessage1,
                (false, true, false, false) => EapolKeyMessage::PairwiseMessage2,
                (true, true, true, _) => EapolKeyMessage::PairwiseMessage3,
                (false, true, false, true) => EapolKeyMessage::PairwiseMessage4,
                _ => EapolKeyMessage::Other,
            }
        } else {
            match (self.ack(), self.mic()) {
                (true, true) => EapolKeyMessage::GroupMessage1,
                (false, true) => EapolKeyMessage::GroupMessage2,
                _ => EapolKeyMessage::Other,
            }
        }
    }
}

/// Validated borrowed view of one complete RSN EAPOL-Key packet.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EapolKeyFrame<'a> {
    bytes: &'a [u8],
    key_data: &'a [u8],
    key_info: EapolKeyInfo,
}

impl<'a> EapolKeyFrame<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, EapolParseError> {
        if bytes.len() < EAPOL_KEY_PACKET_LEN {
            return Err(EapolParseError::Truncated);
        }
        if bytes[1] != EAPOL_PACKET_TYPE_KEY {
            return Err(EapolParseError::NotKeyPacket);
        }

        let body_len = u16::from_be_bytes([bytes[2], bytes[3]]) as usize;
        let packet_len = EAPOL_HEADER_LEN
            .checked_add(body_len)
            .ok_or(EapolParseError::LengthMismatch)?;
        if packet_len != bytes.len() || body_len < EAPOL_KEY_FIXED_LEN {
            return Err(EapolParseError::LengthMismatch);
        }
        if bytes[4] != RSN_KEY_DESCRIPTOR_TYPE {
            return Err(EapolParseError::NotRsnKeyDescriptor);
        }

        let key_data_len = u16::from_be_bytes([bytes[97], bytes[98]]) as usize;
        let key_data_end = EAPOL_KEY_PACKET_LEN
            .checked_add(key_data_len)
            .ok_or(EapolParseError::LengthMismatch)?;
        if key_data_end != packet_len {
            return Err(EapolParseError::LengthMismatch);
        }

        Ok(Self {
            bytes,
            key_data: &bytes[EAPOL_KEY_PACKET_LEN..key_data_end],
            key_info: EapolKeyInfo(u16::from_be_bytes([bytes[5], bytes[6]])),
        })
    }

    pub const fn bytes(self) -> &'a [u8] {
        self.bytes
    }

    pub const fn protocol_version(self) -> u8 {
        self.bytes[0]
    }

    pub const fn key_info(self) -> EapolKeyInfo {
        self.key_info
    }

    pub const fn message(self) -> EapolKeyMessage {
        self.key_info.classify()
    }

    pub fn key_length(self) -> u16 {
        u16::from_be_bytes([self.bytes[7], self.bytes[8]])
    }

    pub fn replay_counter(self) -> u64 {
        u64::from_be_bytes([
            self.bytes[9],
            self.bytes[10],
            self.bytes[11],
            self.bytes[12],
            self.bytes[13],
            self.bytes[14],
            self.bytes[15],
            self.bytes[16],
        ])
    }

    pub fn nonce(self) -> &'a [u8; 32] {
        self.bytes[17..49]
            .try_into()
            .expect("validated EAPOL-Key nonce range")
    }

    pub fn key_iv(self) -> &'a [u8; 16] {
        self.bytes[49..65]
            .try_into()
            .expect("validated EAPOL-Key IV range")
    }

    pub fn mic(self) -> &'a [u8; 16] {
        self.bytes[81..97]
            .try_into()
            .expect("validated EAPOL-Key MIC range")
    }

    pub fn key_receive_sequence(self) -> &'a [u8; 8] {
        self.bytes[65..73]
            .try_into()
            .expect("validated EAPOL-Key RSC range")
    }

    pub fn key_identifier(self) -> &'a [u8; 8] {
        self.bytes[73..81]
            .try_into()
            .expect("validated EAPOL-Key identifier range")
    }

    pub const fn key_data(self) -> &'a [u8] {
        self.key_data
    }

    /// Verifies the MIC of the suite that derived `ptk` without copying the frame.
    pub fn verify_mic(self, ptk: &Ptk) -> bool {
        self.verify_mic_with_kck(ptk.akm(), ptk.kck())
    }

    pub(crate) fn verify_mic_with_confirmation_key(self, key: &RsnKeyConfirmationKey) -> bool {
        self.verify_mic_with_kck(key.akm(), key.as_bytes())
    }

    fn verify_mic_with_kck(self, akm: Akm, kck: &[u8; RSN_KCK_LEN]) -> bool {
        let mut mac = akm.mic(kck);
        mac.update(&self.bytes[..EAPOL_KEY_MIC_START]);
        mac.update(&[0; EAPOL_KEY_MIC_END - EAPOL_KEY_MIC_START]);
        mac.update(&self.bytes[EAPOL_KEY_MIC_END..]);
        mac.verify(self.mic())
    }
}

/// Complete validated EAPOL-Key packet with fixed, Rust-owned storage.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OwnedEapolFrame<const N: usize = DEFAULT_EAPOL_FRAME_CAPACITY> {
    interface: RsnInterface,
    peer: [u8; 6],
    len: usize,
    bytes: [u8; N],
}

impl<const N: usize> OwnedEapolFrame<N> {
    pub fn try_copy(
        interface: RsnInterface,
        peer: [u8; 6],
        bytes: &[u8],
    ) -> Result<Self, EapolCopyError> {
        EapolKeyFrame::parse(bytes).map_err(EapolCopyError::Invalid)?;
        if bytes.len() > N {
            return Err(EapolCopyError::CapacityExceeded);
        }

        let mut owned = [0; N];
        owned[..bytes.len()].copy_from_slice(bytes);
        Ok(Self {
            interface,
            peer,
            len: bytes.len(),
            bytes: owned,
        })
    }

    pub const fn interface(&self) -> RsnInterface {
        self.interface
    }

    pub const fn peer(&self) -> &[u8; 6] {
        &self.peer
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }

    pub fn key_frame(&self) -> EapolKeyFrame<'_> {
        EapolKeyFrame::parse(self.as_bytes()).expect("owned EAPOL frame was validated on creation")
    }
}

#[cfg(test)]
mod tests;
