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
use oer_ieee80211_wire::eapol::KeyInformation;

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

pub(crate) const EAPOL_KEY_MIC_START: usize = 81;
pub(crate) const EAPOL_KEY_MIC_END: usize = 97;

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

    /// The field's subfields.
    pub const fn fields(self) -> KeyInformation {
        KeyInformation::from_raw(self.0)
    }

    pub const fn descriptor_version(self) -> u8 {
        self.fields().descriptor_version().get()
    }

    pub const fn is_pairwise(self) -> bool {
        self.fields().pairwise()
    }

    pub const fn install(self) -> bool {
        self.fields().install()
    }

    pub const fn ack(self) -> bool {
        self.fields().ack()
    }

    pub const fn mic(self) -> bool {
        self.fields().mic()
    }

    pub const fn secure(self) -> bool {
        self.fields().secure()
    }

    pub const fn error(self) -> bool {
        self.fields().error()
    }

    pub const fn request(self) -> bool {
        self.fields().request()
    }

    pub const fn encrypted_key_data(self) -> bool {
        self.fields().encrypted_key_data()
    }

    pub const fn smk(self) -> bool {
        self.fields().smk_message()
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
    fields: Fields<'a>,
}

/// The fixed EAPOL-Key fields, in wire order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Fields<'a> {
    protocol_version: u8,
    key_length: [u8; 2],
    replay_counter: [u8; 8],
    nonce: &'a [u8; 32],
    key_iv: &'a [u8; 16],
    key_receive_sequence: &'a [u8; 8],
    key_identifier: &'a [u8; 8],
    mic: &'a [u8; 16],
}

/// The parsed fields cover exactly the fixed part.
const FIELDS_COVER_THE_FIXED_PART: () = {
    if !(7 + 2 + 8 + 32 + 16 + 8 + 8 + 16 + 2 == EAPOL_KEY_PACKET_LEN
        && EAPOL_KEY_MIC_START == 7 + 2 + 8 + 32 + 16 + 8 + 8
        && EAPOL_KEY_MIC_END == EAPOL_KEY_MIC_START + 16)
    {
        panic!("EAPOL fixed fields must cover the packet");
    }
};

impl<'a> EapolKeyFrame<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, EapolParseError> {
        let () = FIELDS_COVER_THE_FIXED_PART;
        let (header, rest) = bytes
            .split_first_chunk::<7>()
            .ok_or(EapolParseError::Truncated)?;
        let (&key_length, rest) = rest
            .split_first_chunk::<2>()
            .ok_or(EapolParseError::Truncated)?;
        let (&replay_counter, rest) = rest
            .split_first_chunk::<8>()
            .ok_or(EapolParseError::Truncated)?;
        let (nonce, rest) = rest
            .split_first_chunk::<32>()
            .ok_or(EapolParseError::Truncated)?;
        let (key_iv, rest) = rest
            .split_first_chunk::<16>()
            .ok_or(EapolParseError::Truncated)?;
        let (key_receive_sequence, rest) = rest
            .split_first_chunk::<8>()
            .ok_or(EapolParseError::Truncated)?;
        let (key_identifier, rest) = rest
            .split_first_chunk::<8>()
            .ok_or(EapolParseError::Truncated)?;
        let (mic, rest) = rest
            .split_first_chunk::<16>()
            .ok_or(EapolParseError::Truncated)?;
        let (&[key_data_high, key_data_low], key_data) = rest
            .split_first_chunk::<2>()
            .ok_or(EapolParseError::Truncated)?;
        let &[
            protocol_version,
            packet_type,
            length_high,
            length_low,
            descriptor,
            info_high,
            info_low,
        ] = header;
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
            key_data,
            key_info: EapolKeyInfo(u16::from_be_bytes([info_high, info_low])),
            fields: Fields {
                protocol_version,
                key_length,
                replay_counter,
                nonce,
                key_iv,
                key_receive_sequence,
                key_identifier,
                mic,
            },
        })
    }

    pub const fn bytes(self) -> &'a [u8] {
        self.bytes
    }

    pub const fn protocol_version(self) -> u8 {
        self.fields.protocol_version
    }

    pub const fn key_info(self) -> EapolKeyInfo {
        self.key_info
    }

    pub const fn message(self) -> EapolKeyMessage {
        self.key_info.classify()
    }

    pub fn key_length(self) -> u16 {
        u16::from_be_bytes(self.fields.key_length)
    }

    pub fn replay_counter(self) -> u64 {
        u64::from_be_bytes(self.fields.replay_counter)
    }

    pub fn nonce(self) -> &'a [u8; 32] {
        self.fields.nonce
    }

    pub fn key_iv(self) -> &'a [u8; 16] {
        self.fields.key_iv
    }

    pub fn mic(self) -> &'a [u8; 16] {
        self.fields.mic
    }

    pub fn key_receive_sequence(self) -> &'a [u8; 8] {
        self.fields.key_receive_sequence
    }

    pub fn key_identifier(self) -> &'a [u8; 8] {
        self.fields.key_identifier
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
        let mut mac = akm.mic(kck);
        let Some(before_mic) = self.bytes.get(..EAPOL_KEY_MIC_START) else {
            return false;
        };
        let Some(after_mic) = self.bytes.get(EAPOL_KEY_MIC_END..) else {
            return false;
        };
        mac.update(before_mic);
        mac.update(&[0; EAPOL_KEY_MIC_END - EAPOL_KEY_MIC_START]);
        mac.update(after_mic);
        mac.verify(self.fields.mic)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OwnedEapolFrame<const N: usize = DEFAULT_EAPOL_FRAME_CAPACITY> {
    interface: RsnInterface,
    peer: [u8; 6],
    bytes: heapless::Vec<u8, N>,
}

impl<const N: usize> OwnedEapolFrame<N> {
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn invalidate_for_test(&mut self) {
        self.bytes.clear();
    }

    pub fn try_copy(
        interface: RsnInterface,
        peer: [u8; 6],
        bytes: &[u8],
    ) -> Result<Self, EapolCopyError> {
        EapolKeyFrame::parse(bytes).map_err(EapolCopyError::Invalid)?;
        let owned =
            heapless::Vec::from_slice(bytes).map_err(|_| EapolCopyError::CapacityExceeded)?;
        Ok(Self {
            interface,
            peer,
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
        self.bytes.as_slice()
    }

    pub fn key_frame(&self) -> Result<EapolKeyFrame<'_>, EapolParseError> {
        EapolKeyFrame::parse(self.as_bytes())
    }
}

#[cfg(test)]
mod tests;
