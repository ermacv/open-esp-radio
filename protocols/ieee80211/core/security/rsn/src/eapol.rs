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
use oer_ieee80211_wire::eapol::{EapolKey, EapolParseError, KeyInformation};

pub const DEFAULT_EAPOL_FRAME_CAPACITY: usize = 512;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EapolCopyError {
    CapacityExceeded,
    Invalid(EapolParseError),
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

/// Validated borrowed view of one complete RSN EAPOL-Key packet: the
/// codec's checked fields, classified and authenticated here.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EapolKeyFrame<'a>(EapolKey<'a>);

impl<'a> EapolKeyFrame<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, EapolParseError> {
        EapolKey::parse(bytes).map(Self)
    }

    pub const fn bytes(self) -> &'a [u8] {
        self.0.bytes()
    }

    pub const fn protocol_version(self) -> u8 {
        self.0.protocol_version()
    }

    pub const fn key_info(self) -> EapolKeyInfo {
        EapolKeyInfo(self.0.key_information().raw())
    }

    pub const fn message(self) -> EapolKeyMessage {
        self.key_info().classify()
    }

    pub const fn key_length(self) -> u16 {
        self.0.key_length()
    }

    pub const fn replay_counter(self) -> u64 {
        self.0.replay_counter()
    }

    pub const fn nonce(self) -> &'a [u8; 32] {
        self.0.nonce()
    }

    pub const fn key_iv(self) -> &'a [u8; 16] {
        self.0.key_iv()
    }

    pub const fn mic(self) -> &'a [u8; 16] {
        self.0.mic()
    }

    pub const fn key_receive_sequence(self) -> &'a [u8; 8] {
        self.0.key_rsc()
    }

    pub const fn key_identifier(self) -> &'a [u8; 8] {
        self.0.key_identifier()
    }

    pub const fn key_data(self) -> &'a [u8] {
        self.0.key_data()
    }

    pub fn verify_mic(self, ptk: &Ptk) -> bool {
        self.verify_mic_with_kck(ptk.akm(), ptk.kck())
    }

    pub(crate) fn verify_mic_with_confirmation_key(self, key: &RsnKeyConfirmationKey) -> bool {
        self.verify_mic_with_kck(key.akm(), key.as_bytes())
    }

    fn verify_mic_with_kck(self, akm: Akm, kck: &[u8; RSN_KCK_LEN]) -> bool {
        let mut mac = akm.mic(kck);
        for part in self.0.mic_input() {
            mac.update(part);
        }
        mac.verify(self.0.mic())
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
