//! Validated borrowed and owned RSN EAPOL-Key packets and MIC verification.
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
    fixed: &'a [u8; EAPOL_KEY_PACKET_LEN],
    key_data: &'a [u8],
    key_info: EapolKeyInfo,
}

/// The fixed EAPOL-Key fields, in wire order.
struct Fields<'a> {
    key_length: [u8; 2],
    replay_counter: [u8; 8],
    nonce: &'a [u8; 32],
    key_iv: &'a [u8; 16],
    key_receive_sequence: &'a [u8; 8],
    key_identifier: &'a [u8; 8],
    mic: &'a [u8; 16],
}

/// Split the fixed part into its fields. The chunks add up to the fixed
/// length, so every split succeeds.
fn fields(fixed: &[u8; EAPOL_KEY_PACKET_LEN]) -> Option<Fields<'_>> {
    let (_header, rest) = fixed.split_first_chunk::<7>()?;
    let (&key_length, rest) = rest.split_first_chunk::<2>()?;
    let (&replay_counter, rest) = rest.split_first_chunk::<8>()?;
    let (nonce, rest) = rest.split_first_chunk::<32>()?;
    let (key_iv, rest) = rest.split_first_chunk::<16>()?;
    let (key_receive_sequence, rest) = rest.split_first_chunk::<8>()?;
    let (key_identifier, rest) = rest.split_first_chunk::<8>()?;
    let (mic, _key_data_length) = rest.split_first_chunk::<16>()?;
    Some(Fields {
        key_length,
        replay_counter,
        nonce,
        key_iv,
        key_receive_sequence,
        key_identifier,
        mic,
    })
}

/// A field split that cannot fail: [`fields`] covers exactly the fixed part.
const FIELDS_COVER_THE_FIXED_PART: () = assert!(
    7 + 2 + 8 + 32 + 16 + 8 + 8 + 16 + 2 == EAPOL_KEY_PACKET_LEN
        && EAPOL_KEY_MIC_START == 7 + 2 + 8 + 32 + 16 + 8 + 8
        && EAPOL_KEY_MIC_END == EAPOL_KEY_MIC_START + 16
);

impl<'a> EapolKeyFrame<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, EapolParseError> {
        let () = FIELDS_COVER_THE_FIXED_PART;
        let (fixed, key_data) = bytes
            .split_first_chunk::<EAPOL_KEY_PACKET_LEN>()
            .ok_or(EapolParseError::Truncated)?;
        let &[
            _,
            packet_type,
            length_high,
            length_low,
            descriptor,
            info_high,
            info_low,
            ..,
            key_data_high,
            key_data_low,
        ] = fixed;
        if packet_type != EAPOL_PACKET_TYPE_KEY {
            return Err(EapolParseError::NotKeyPacket);
        }

        let body_len = usize::from(u16::from_be_bytes([length_high, length_low]));
        let packet_len = EAPOL_HEADER_LEN
            .checked_add(body_len)
            .ok_or(EapolParseError::LengthMismatch)?;
        if packet_len != bytes.len() || body_len < EAPOL_KEY_FIXED_LEN {
            return Err(EapolParseError::LengthMismatch);
        }
        if descriptor != RSN_KEY_DESCRIPTOR_TYPE {
            return Err(EapolParseError::NotRsnKeyDescriptor);
        }

        let key_data_len = usize::from(u16::from_be_bytes([key_data_high, key_data_low]));
        if key_data_len != key_data.len() {
            return Err(EapolParseError::LengthMismatch);
        }

        Ok(Self {
            bytes,
            fixed,
            key_data,
            key_info: EapolKeyInfo(u16::from_be_bytes([info_high, info_low])),
        })
    }

    /// The fixed fields. `FIELDS_COVER_THE_FIXED_PART` proves the split
    /// total; were it not, the frame would read as all zeros and fail every
    /// check, never pass one.
    fn fields(self) -> Fields<'a> {
        fields(self.fixed).unwrap_or(Fields {
            key_length: [0; 2],
            replay_counter: [0; 8],
            nonce: &[0; 32],
            key_iv: &[0; 16],
            key_receive_sequence: &[0; 8],
            key_identifier: &[0; 8],
            mic: &[0; 16],
        })
    }

    /// A frame that matches no message and verifies under no key.
    const CLOSED: EapolKeyFrame<'static> = EapolKeyFrame {
        bytes: &[],
        fixed: &[0; EAPOL_KEY_PACKET_LEN],
        key_data: &[],
        key_info: EapolKeyInfo(0),
    };

    pub const fn bytes(self) -> &'a [u8] {
        self.bytes
    }

    pub const fn protocol_version(self) -> u8 {
        self.fixed[0]
    }

    pub const fn key_info(self) -> EapolKeyInfo {
        self.key_info
    }

    pub const fn message(self) -> EapolKeyMessage {
        self.key_info.classify()
    }

    pub fn key_length(self) -> u16 {
        u16::from_be_bytes(self.fields().key_length)
    }

    pub fn replay_counter(self) -> u64 {
        u64::from_be_bytes(self.fields().replay_counter)
    }

    pub fn nonce(self) -> &'a [u8; 32] {
        self.fields().nonce
    }

    pub fn key_iv(self) -> &'a [u8; 16] {
        self.fields().key_iv
    }

    pub fn mic(self) -> &'a [u8; 16] {
        self.fields().mic
    }

    pub fn key_receive_sequence(self) -> &'a [u8; 8] {
        self.fields().key_receive_sequence
    }

    pub fn key_identifier(self) -> &'a [u8; 8] {
        self.fields().key_identifier
    }

    pub const fn key_data(self) -> &'a [u8] {
        self.key_data
    }

    pub fn verify_mic(self, ptk: &Ptk) -> bool {
        self.verify_mic_with_kck(ptk.akm(), ptk.kck())
    }

    pub(crate) fn verify_mic_with_confirmation_key(self, key: &RsnKeyConfirmationKey) -> bool {
        self.verify_mic_with_kck(key.akm(), key.as_bytes())
    }

    fn verify_mic_with_kck(self, akm: Akm, kck: &[u8; RSN_KCK_LEN]) -> bool {
        let Some((before_mic, rest)) = self.fixed.split_first_chunk::<EAPOL_KEY_MIC_START>() else {
            return false;
        };
        let Some((mic, after_mic)) = rest.split_first_chunk::<16>() else {
            return false;
        };
        let mut mac = akm.mic(kck);
        mac.update(before_mic);
        mac.update(&[0; EAPOL_KEY_MIC_END - EAPOL_KEY_MIC_START]);
        mac.update(after_mic);
        mac.update(self.key_data);
        mac.verify(mic)
    }
}

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
        let mut owned = [0; N];
        owned
            .get_mut(..bytes.len())
            .ok_or(EapolCopyError::CapacityExceeded)?
            .copy_from_slice(bytes);
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
        self.bytes.get(..self.len).unwrap_or_default()
    }

    /// The frame view. The bytes were validated when copied; were they not,
    /// the view would match no message and verify under no key.
    pub fn key_frame(&self) -> EapolKeyFrame<'_> {
        EapolKeyFrame::parse(self.as_bytes()).unwrap_or(EapolKeyFrame::CLOSED)
    }
}

#[cfg(test)]
mod tests;
