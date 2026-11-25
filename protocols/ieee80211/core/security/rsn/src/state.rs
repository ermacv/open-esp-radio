//! The facts both four-way handshakes share: nonces, the PTK's context,
//! the authenticator's transmissions and the reasons a peer's EAPOL-Key
//! frame is refused.

use crate::Akm;
use crate::AkmKeys;
use oer_protocols_ieee802_wire::Eui48;
use oer_protocols_ieee80211_wire::eapol::KeyInformation;

pub const RSN_NONCE_LEN: usize = 32;

pub(crate) const RSN_CCMP_TEMPORAL_KEY_LEN: u16 = 16;
/// Key Information of an authenticator pairwise Message 3 for `akm`.
pub(crate) fn pairwise_message3_key_info(akm: Akm) -> u16 {
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
pub struct PtkContext {
    pub authenticator_address: Eui48,
    pub supplicant_address: Eui48,
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
    InvalidFrame(oer_protocols_ieee80211_wire::eapol::EapolParseError),
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
    /// The frame's MIC is not the one its key computes: a forged frame.
    InvalidMic,
}

impl From<oer_protocols_ieee80211_wire::eapol::EapolParseError> for RsnStateError {
    fn from(error: oer_protocols_ieee80211_wire::eapol::EapolParseError) -> Self {
        Self::InvalidFrame(error)
    }
}
