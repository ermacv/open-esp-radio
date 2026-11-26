//! Validated borrowed and owned RSN EAPOL-Key packets and MIC verification.
//!
//! Received bytes reach this module; nothing here panics on them.

use crate::AkmKeys;
use crate::{Akm, Ptk, RSN_KCK_LEN, RsnInterface, RsnKeyConfirmationKey};
use oer_portable_crypto::block::{Aes128Engine, BlockError};
use oer_protocols_ieee802_wire::Eui48;
use oer_protocols_ieee80211_wire::eapol::{EapolKey, EapolParseError, KeyInformation};

/// Why an EAPOL-Key frame's MIC did not verify.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MicFailure {
    /// The MIC is not the frame's under the key: a peer's frame to refuse.
    Mismatch,
    /// The AES engine refused the MIC's key or a block.
    Engine(BlockError),
}

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

impl EapolKeyMessage {
    /// The message whose Key Information is `information`.
    pub const fn of(information: KeyInformation) -> Self {
        if information.error() || information.request() || information.smk_message() {
            return Self::Other;
        }

        if information.pairwise() {
            match (
                information.ack(),
                information.mic(),
                information.install(),
                information.secure(),
            ) {
                (true, false, false, false) => Self::PairwiseMessage1,
                (false, true, false, false) => Self::PairwiseMessage2,
                (true, true, true, _) => Self::PairwiseMessage3,
                (false, true, false, true) => Self::PairwiseMessage4,
                _ => Self::Other,
            }
        } else {
            match (information.ack(), information.mic()) {
                (true, true) => Self::GroupMessage1,
                (false, true) => Self::GroupMessage2,
                _ => Self::Other,
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

    pub const fn protocol_version(
        self,
    ) -> oer_protocols_ieee80211_wire::eapol::EapolProtocolVersion {
        self.0.protocol_version()
    }

    pub const fn key_info(self) -> KeyInformation {
        self.0.key_information()
    }

    pub const fn message(self) -> EapolKeyMessage {
        EapolKeyMessage::of(self.key_info())
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

    /// The MIC checked under the PTK's KCK: it verifies, or the failure.
    pub fn check_mic<E: Aes128Engine>(self, ptk: &Ptk, engine: &mut E) -> Result<(), MicFailure> {
        match self.verify_mic(ptk, engine) {
            Ok(true) => Ok(()),
            Ok(false) => Err(MicFailure::Mismatch),
            Err(error) => Err(MicFailure::Engine(error)),
        }
    }

    /// Whether the MIC verifies under the PTK's KCK. An AES-CMAC suite runs
    /// on `engine`; its refusal is the error.
    pub fn verify_mic<E: Aes128Engine>(
        self,
        ptk: &Ptk,
        engine: &mut E,
    ) -> Result<bool, BlockError> {
        self.verify_mic_with_kck(ptk.akm(), ptk.kck(), engine)
    }

    pub(crate) fn verify_mic_with_confirmation_key<E: Aes128Engine>(
        self,
        key: &RsnKeyConfirmationKey,
        engine: &mut E,
    ) -> Result<bool, BlockError> {
        self.verify_mic_with_kck(key.akm(), key.as_bytes(), engine)
    }

    fn verify_mic_with_kck<E: Aes128Engine>(
        self,
        akm: Akm,
        kck: &[u8; RSN_KCK_LEN],
        engine: &mut E,
    ) -> Result<bool, BlockError> {
        let mut mac = akm.mic(kck, engine)?;
        for part in self.0.mic_input() {
            mac.update(part)?;
        }
        mac.verify(self.0.mic())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OwnedEapolFrame<const N: usize = DEFAULT_EAPOL_FRAME_CAPACITY> {
    interface: RsnInterface,
    peer: Eui48,
    bytes: heapless::Vec<u8, N>,
}

impl<const N: usize> OwnedEapolFrame<N> {
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn invalidate_for_test(&mut self) {
        self.bytes.clear();
    }

    pub fn try_copy(
        interface: RsnInterface,
        peer: Eui48,
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

    pub const fn peer(&self) -> Eui48 {
        self.peer
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
