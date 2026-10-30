//! RSN four-way-handshake control flow, parameterized by the negotiated suite.
//!
//! The state machines never execute cryptography, transmit a frame, install a
//! key, allocate, retry, or wait. Each method consumes one completion/event
//! and returns at most one owned action for the radio executor.

use crate::AkmKeys;
use crate::{Akm, DEFAULT_EAPOL_FRAME_CAPACITY, EapolKeyMessage, OwnedEapolFrame, RsnInterface};
use oer_ieee80211_wire::eapol::KeyInformation;

pub const RSN_NONCE_LEN: usize = 32;

const RSN_CCMP_TEMPORAL_KEY_LEN: u16 = 16;
/// Key Information of an authenticator pairwise Message 3 for `akm`.
fn pairwise_message3_key_info(akm: Akm) -> u16 {
    KeyInformation::EMPTY
        .with_descriptor_version(akm.key_descriptor_version())
        .with_pairwise(true)
        .with_install(true)
        .with_ack(true)
        .with_mic(true)
        .with_secure(true)
        .with_encrypted_key_data(true)
        .raw()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RsnTicket(u32);

impl RsnTicket {
    pub const fn raw(self) -> u32 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PtkContext {
    pub authenticator_address: [u8; 6],
    pub supplicant_address: [u8; 6],
    pub authenticator_nonce: [u8; RSN_NONCE_LEN],
    pub supplicant_nonce: [u8; RSN_NONCE_LEN],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RsnTxMessage {
    PairwiseMessage1,
    PairwiseMessage2,
    PairwiseMessage3,
    PairwiseMessage4,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RsnTransmit {
    pub message: RsnTxMessage,
    pub replay_counter: u64,
    pub retransmission: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RsnStateError {
    InvalidFrame(oer_ieee80211_wire::eapol::EapolParseError),
    WrongInterface,
    WrongPeer,
    UnsupportedDescriptorVersion,
    UnsupportedMessage,
    UnexpectedMessage,
    StaleReplayCounter,
    ReplayCounterMismatch,
    ReplayCounterExhausted,
    ZeroNonce,
    AuthenticatorNonceMismatch,
    InvalidKeyLength,
    InvalidMessage3KeyInfo,
    NonzeroKeyIv,
    NonzeroKeyIdentifier,
    MissingEncryptedKeyData,
    WrongPhase,
    StaleCompletion,
    RetainedFrameMismatch,
}

impl From<oer_ieee80211_wire::eapol::EapolParseError> for RsnStateError {
    fn from(error: oer_ieee80211_wire::eapol::EapolParseError) -> Self {
        Self::InvalidFrame(error)
    }
}

impl RsnStateError {
    /// Whether this error can be produced solely by rejecting the current
    /// untrusted peer frame before any authenticated protocol transition.
    ///
    /// Completion-ticket, retained-frame and phase failures are deliberately
    /// excluded: those indicate a local ownership/invariant defect and must
    /// remain visible to the executor.
    pub const fn is_peer_input_rejection(self) -> bool {
        matches!(
            self,
            Self::WrongInterface
                | Self::WrongPeer
                | Self::UnsupportedDescriptorVersion
                | Self::UnsupportedMessage
                | Self::UnexpectedMessage
                | Self::StaleReplayCounter
                | Self::ReplayCounterMismatch
                | Self::ZeroNonce
                | Self::AuthenticatorNonceMismatch
                | Self::InvalidKeyLength
                | Self::InvalidMessage3KeyInfo
                | Self::NonzeroKeyIv
                | Self::NonzeroKeyIdentifier
                | Self::MissingEncryptedKeyData
        )
    }
}

mod authenticator;
mod supplicant;

pub use authenticator::{RsnApAction, RsnApPeerError, RsnApPeers, RsnApPhase, RsnApState};
pub use supplicant::{RsnStaAction, RsnStaPhase, RsnStaState};

const fn nonce_is_zero(nonce: &[u8; RSN_NONCE_LEN]) -> bool {
    let mut remaining = nonce.as_slice();
    while let Some((&byte, rest)) = remaining.split_first() {
        if byte != 0 {
            return false;
        }
        remaining = rest;
    }
    true
}

#[cfg(test)]
mod tests;
