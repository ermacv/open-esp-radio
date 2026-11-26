//! Allocation-free RSN supplicant: the station's four-way handshake as one
//! machine, [`RsnStaHandshake`], and the connected association it becomes,
//! [`RsnConnectedSupplicant`].
//!
//! A step runs the handshake's cryptography in place: PTK derivation, MIC
//! verification against the exact Association security IEs, the RFC 3394
//! key-data unwrap. It never waits: the caller lends its synchronous AES-128
//! engine ([`Aes128Engine`]) to every step that verifies or authenticates an
//! EAPOL-Key MIC (AES-CMAC for PSK-SHA256 and SAE) or takes a Message 3's or
//! Group Message 1's key data, so a target handles only complete EAPOL RX/TX
//! frames, the engine and the platform-specific key-slot transaction. An
//! engine refusal is the `Engine` error of the step.

use crate::AkmKeys;
use crate::MicFailure;
use crate::{
    Akm, EapolKeyFrame, Kek, Pmk, Ptk, PtkContext, RsnInterface, RsnKeyConfirmationKey,
    element::{RsnElementError, validate_rsn_element},
    frames::{
        OwnedAssociationSecurityIes, OwnedRsnIe, RsnFrameError, RsnGroupKeys, RsnIgtk, RsnTxFrame,
        parse_group_gtk_key_data, parse_gtk_key_data,
    },
    keys::RsnKeyInstall,
    state::RsnStateError,
};
use oer_portable_crypto::{
    KeyUnwrapError, KeyUnwrapRequest,
    block::{Aes128Engine, BlockError},
};
use oer_portable_time::{Duration, Instant};
use oer_protocols_ieee802_wire::Eui48;
use oer_protocols_ieee80211_core_security_cipher::aes::RsnKeyData;
use oer_protocols_ieee80211_wire::eapol::KeyInformation;
use oer_protocols_ieee80211_wire::security::GroupManagementCipher;
use zeroize::{Zeroize, ZeroizeOnDrop};

/// Hardware-qualified open-STA compatibility window for the first EAPOL-Key
/// message. The numeric value is retained from the earlier hardware-tested
/// policy; it is not claimed as a recovered vendor constant.
pub const RSN_STA_MESSAGE1_TIMEOUT: Duration = Duration::from_secs(3);

/// Complete window after the original Message 2 transmission.
///
/// SOURCE: ESP-IDF 4d80c1b29aeabc8e735ffc6e1be9bb23e95d1d5d,
/// components/wpa_supplicant/src/rsn_supp/wpa.c::wpa_supplicant_process_1_of_4
/// emits one Message 2 and does not register a
/// retransmission timeout. A repeated Message 1 instead re-enters through
/// `wpa_sm_rx_eapol` and produces another Message 2. The six-second value
/// covers two three-second hardware receive windows without
/// inventing a station-originated Message 2 retry.
pub const RSN_STA_MESSAGE3_TIMEOUT: Duration = Duration::from_secs(6);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RsnStaResponseWait {
    Message1,
    Message3,
}

impl RsnStaResponseWait {
    /// How long the supplicant waits for this message.
    pub const fn timeout(self) -> Duration {
        match self {
            Self::Message1 => RSN_STA_MESSAGE1_TIMEOUT,
            Self::Message3 => RSN_STA_MESSAGE3_TIMEOUT,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RsnStaDeadlineEvent {
    Pending,
    Expired {
        wait: RsnStaResponseWait,
        elapsed: Duration,
    },
}

/// The absolute deadline of one response wait on the image's monotonic time.
///
/// The caller polls it with the time at which it last serviced received
/// frames, so a frame received by the deadline is processed before the
/// timeout wins. It never reads a clock or waits.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RsnStaResponseDeadline {
    wait: RsnStaResponseWait,
    started: Instant,
    deadline: Instant,
}

impl RsnStaResponseDeadline {
    /// Start waiting at `now`; `None` when the deadline lies past the
    /// representable time.
    pub const fn start(wait: RsnStaResponseWait, now: Instant) -> Option<Self> {
        match now.checked_add(wait.timeout()) {
            Some(deadline) => Some(Self {
                wait,
                started: now,
                deadline,
            }),
            None => None,
        }
    }

    pub const fn deadline(&self) -> Instant {
        self.deadline
    }

    /// Whether the wait expired at `now`.
    pub const fn poll(&self, now: Instant) -> RsnStaDeadlineEvent {
        if now.as_micros() >= self.deadline.as_micros() {
            RsnStaDeadlineEvent::Expired {
                wait: self.wait,
                elapsed: now.saturating_duration_since(self.started),
            }
        } else {
            RsnStaDeadlineEvent::Pending
        }
    }
}

/// Why a handshake could not be made; nothing is kept.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RsnStaNewError {
    /// A security element does not fit or is not one.
    Frame(RsnFrameError),
    /// The station's own Association RSN element, or the access point's,
    /// names no supported suite.
    Element(RsnElementError),
    /// The station's nonce is zero.
    ZeroNonce,
}

/// Why a step of the handshake refused its input; the handshake is as it
/// was.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RsnStaError {
    /// The received EAPOL-Key frame was refused: unsupported, stale,
    /// forged or otherwise unauthenticated peer input.
    Rejected(RsnStateError),
    /// The frame is not an EAPOL-Key frame.
    InvalidFrame(oer_protocols_ieee80211_wire::eapol::EapolParseError),
    /// A previously validated local security-IE owner lost its invariant.
    RetainedSecurityIes(RsnFrameError),
    /// The AES engine refused a MIC's key or block.
    Engine(BlockError),
}

impl From<oer_protocols_ieee80211_wire::eapol::EapolParseError> for RsnStaError {
    fn from(error: oer_protocols_ieee80211_wire::eapol::EapolParseError) -> Self {
        Self::InvalidFrame(error)
    }
}

/// Why a handshake ended. Its holder drops it, takes down what it
/// installed and deauthenticates.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RsnStaEnd {
    /// The authenticated Key Data field is longer than
    /// [`oer_protocols_ieee80211_core_security_cipher::aes::RSN_KEY_DATA_CAPACITY`].
    KeyDataTooLong,
    /// The authenticated key data did not unwrap.
    KeyUnwrap(KeyUnwrapError),
    /// The unwrapped key data did not parse.
    KeyData(RsnFrameError),
    /// The AES engine refused Message 4's MIC.
    Engine(BlockError),
    /// `complete` was given an install another handshake requested: this
    /// one requested none.
    Unrequested,
}

/// The keys of an authenticated Message 3 to install: the pairwise key,
/// then the group keys. Both keys are owned copies, zeroized on drop: the
/// port's commands take them by value.
#[must_use]
pub struct RsnStaKeyInstallRequest {
    pairwise: RsnKeyInstall,
    group: RsnStaGroupInstall,
}

impl RsnStaKeyInstallRequest {
    pub const fn replay_counter(&self) -> oer_protocols_ieee80211_wire::eapol::ReplayCounter {
        self.group.replay_counter()
    }
    pub const fn plain_key_data_len(&self) -> usize {
        self.group.plain_key_data_len()
    }
    pub const fn pairwise(&self) -> &RsnKeyInstall {
        &self.pairwise
    }
    pub const fn group(&self) -> &RsnKeyInstall {
        self.group.group()
    }
    pub const fn igtk(&self) -> Option<&RsnIgtk> {
        self.group.igtk()
    }

    /// Move the pairwise installation to its consumer and retain the group
    /// installation separately. No temporal-key bytes are copied.
    pub fn into_parts(self) -> (RsnKeyInstall, RsnStaGroupInstall) {
        (self.pairwise, self.group)
    }
}

/// The group half of a Message 3's installation. It holds no pairwise key.
#[must_use]
pub struct RsnStaGroupInstall {
    replay_counter: oer_protocols_ieee80211_wire::eapol::ReplayCounter,
    plain_key_data_len: usize,
    group: RsnKeyInstall,
    igtk: Option<RsnIgtk>,
}

impl RsnStaGroupInstall {
    pub const fn replay_counter(&self) -> oer_protocols_ieee80211_wire::eapol::ReplayCounter {
        self.replay_counter
    }
    pub const fn plain_key_data_len(&self) -> usize {
        self.plain_key_data_len
    }
    pub const fn group(&self) -> &RsnKeyInstall {
        &self.group
    }
    pub const fn igtk(&self) -> Option<&RsnIgtk> {
        self.igtk.as_ref()
    }

    /// The group keys are installed: the keys go, and what remains is the
    /// proof [`RsnStaHandshake::complete`] takes.
    pub fn installed(self) -> RsnStaInstalled {
        RsnStaInstalled(())
    }
}

/// The proof that a handshake's installs ended; it carries no key.
#[must_use]
pub struct RsnStaInstalled(());

/// A completed handshake: the connected supplicant and Message 4.
pub struct RsnStaConnected<const N: usize> {
    pub supplicant: RsnConnectedSupplicant,
    pub message4: RsnTxFrame<N>,
}

/// What a received EAPOL-Key frame did.
#[must_use]
pub enum RsnStaEapol<const N: usize> {
    /// Nothing: a frame the handshake ignores (a repeat).
    None,
    /// Message 2, built, to transmit: the answer to a new Message 1 or the
    /// same frame again for a repeated one.
    Transmit(RsnTxFrame<N>),
    /// A verified Message 3: its keys to install, then
    /// [`RsnStaHandshake::complete`].
    Install(RsnStaKeyInstallRequest),
    /// The handshake ended.
    End(RsnStaEnd),
}

/// Unwrap `wrapped` under `kek` to completion over `engine`.
fn unwrap_key_data<E: Aes128Engine>(
    engine: &mut E,
    kek: &Kek,
    wrapped: &[u8],
) -> Result<RsnKeyData, KeyDataError> {
    let wrapped = RsnKeyData::try_copy(wrapped).map_err(|_| KeyDataError::TooLong)?;
    oer_portable_crypto::block::unwrap_now(
        engine,
        KeyUnwrapRequest::new(kek.request_key(), wrapped),
    )
    .into_plaintext()
    .map_err(KeyDataError::Unwrap)
}

/// Why authenticated key data yields no plaintext.
enum KeyDataError {
    TooLong,
    Unwrap(KeyUnwrapError),
}

impl KeyDataError {
    const fn into_end(self) -> RsnStaEnd {
        match self {
            Self::TooLong => RsnStaEnd::KeyDataTooLong,
            Self::Unwrap(error) => RsnStaEnd::KeyUnwrap(error),
        }
    }

    const fn into_connected(self) -> RsnConnectedProcessError {
        match self {
            Self::TooLong => RsnConnectedProcessError::KeyDataTooLong,
            Self::Unwrap(error) => RsnConnectedProcessError::KeyUnwrap(error),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RsnConnectedSupplicantError {
    WrongInterface,
    WrongPeer,
    ProtocolVersionMismatch,
    UnsupportedDescriptorVersion,
    UnsupportedMessage,
    AuthenticatorNonceMismatch,
    RetainedMessage3Mismatch,
    RetainedGroupMessage1Mismatch,
    MissingEncryptedKeyData,
    InvalidMic,
    StaleReplayCounter,
    StaleCompletion,
    Frame(RsnFrameError),
    InvalidFrame(oer_protocols_ieee80211_wire::eapol::EapolParseError),
    /// A previously validated local security-IE owner lost its invariant.
    RetainedSecurityIes(RsnFrameError),
    /// The AES engine refused a MIC's key or block.
    Engine(BlockError),
}

/// Why the connected supplicant refused a frame or the key data of an
/// authenticated Message 3 or Group Message 1.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RsnConnectedProcessError {
    Supplicant(RsnConnectedSupplicantError),
    /// The authenticated Key Data field is longer than
    /// [`oer_protocols_ieee80211_core_security_cipher::aes::RSN_KEY_DATA_CAPACITY`].
    KeyDataTooLong,
    /// The authenticated key data did not unwrap.
    KeyUnwrap(KeyUnwrapError),
}

impl From<RsnConnectedSupplicantError> for RsnConnectedProcessError {
    fn from(error: RsnConnectedSupplicantError) -> Self {
        Self::Supplicant(error)
    }
}

impl From<oer_protocols_ieee80211_wire::eapol::EapolParseError> for RsnConnectedSupplicantError {
    fn from(error: oer_protocols_ieee80211_wire::eapol::EapolParseError) -> Self {
        Self::InvalidFrame(error)
    }
}
impl From<oer_protocols_ieee80211_wire::eapol::EapolParseError> for RsnConnectedProcessError {
    fn from(error: oer_protocols_ieee80211_wire::eapol::EapolParseError) -> Self {
        Self::Supplicant(error.into())
    }
}
impl From<RsnFrameError> for RsnConnectedSupplicantError {
    fn from(error: RsnFrameError) -> Self {
        Self::Frame(error)
    }
}
impl From<RsnFrameError> for RsnConnectedProcessError {
    fn from(error: RsnFrameError) -> Self {
        Self::Supplicant(error.into())
    }
}

/// Exact connected-state GTK publication ticket.
pub struct RsnGroupKeyInstallRequest<const N: usize> {
    replay_counter: oer_protocols_ieee80211_wire::eapol::ReplayCounter,
    commitment: RsnCompletedKeyMessage,
    group: RsnKeyInstall,
    igtk: Option<RsnIgtk>,
    response: RsnTxFrame<N>,
}

impl<const N: usize> RsnGroupKeyInstallRequest<N> {
    pub const fn replay_counter(&self) -> oer_protocols_ieee80211_wire::eapol::ReplayCounter {
        self.replay_counter
    }

    pub const fn group(&self) -> &RsnKeyInstall {
        &self.group
    }

    /// The IGTK of an association that protects its management frames.
    pub const fn igtk(&self) -> Option<&RsnIgtk> {
        self.igtk.as_ref()
    }
}

/// One authenticated Message 3 or Group Message 1 with encrypted key data,
/// awaiting [`RsnConnectedSupplicant::prepare_group_key_install`].
///
/// It is the affine owner of the authenticated frame. A station may retain it
/// until the current TX ends, or replace it with a newer checked frame. No
/// ticket or busy latch is needed; preparation checks the committed replay
/// frontier.
pub struct RsnGroupKeyFrame<const N: usize> {
    frame: crate::OwnedEapolFrame<N>,
    replay_counter: oer_protocols_ieee80211_wire::eapol::ReplayCounter,
}

impl<const N: usize> RsnGroupKeyFrame<N> {
    #[cfg(any(test, feature = "test-support"))]
    pub fn invalidate_frame_for_test(&mut self) {
        self.frame.invalidate_for_test();
    }

    pub fn replay_counter(&self) -> oer_protocols_ieee80211_wire::eapol::ReplayCounter {
        self.replay_counter
    }

    /// Return the authenticated frame when its exchange is disabled.
    pub fn into_frame(self) -> crate::OwnedEapolFrame<N> {
        self.frame
    }
}

pub enum RsnConnectedAction<const N: usize> {
    /// A new authenticated Message 3 or Group Message 1, whose key data
    /// [`RsnConnectedSupplicant::prepare_group_key_install`] unwraps.
    GroupKeyData(RsnGroupKeyFrame<N>),
    Retransmit(RsnTxFrame<N>),
}

/// Bounded binding to the exact authenticated Message 3 which installed the
/// connected association's keys.
///
/// The retained MIC is an HMAC-SHA1-128 commitment to the complete EAPOL-Key
/// packet with its MIC field cleared. The small explicit protocol prefix lets
/// descriptor, flag and replay mismatches fail before authentication; after a
/// candidate MIC verifies under the retained KCK, equality with the completed
/// MIC binds every remaining field, including ANonce, IV, RSC and encrypted
/// Key Data, without retaining those handshake bytes after connection.
#[derive(Zeroize, ZeroizeOnDrop)]
struct RsnCompletedMessage3 {
    #[zeroize(skip)]
    protocol_version: oer_protocols_ieee80211_wire::eapol::EapolProtocolVersion,
    #[zeroize(skip)]
    key_info: KeyInformation,
    /// Sent in clear in every EAPOL-Key frame (IEEE Std 802.11-2020,
    /// 12.7.2): erasing it guards nothing.
    #[zeroize(skip)]
    replay_counter: oer_protocols_ieee80211_wire::eapol::ReplayCounter,
    /// The ANonce, sent in clear in Messages 1 and 3 (IEEE Std 802.11-2020,
    /// 12.7.2, 12.7.6.2, 12.7.6.4): erasing it guards nothing.
    #[zeroize(skip)]
    authenticator_nonce: oer_protocols_ieee80211_wire::eapol::Nonce,
    mic: [u8; 16],
}

impl RsnCompletedMessage3 {
    fn capture(frame: EapolKeyFrame<'_>) -> Self {
        Self {
            protocol_version: frame.protocol_version(),
            key_info: frame.key_info(),
            replay_counter: frame.replay_counter(),
            authenticator_nonce: frame.nonce(),
            mic: *frame.mic(),
        }
    }

    fn validate_fields(
        &self,
        akm: Akm,
        frame: EapolKeyFrame<'_>,
    ) -> Result<(), RsnConnectedSupplicantError> {
        if frame.protocol_version() != self.protocol_version {
            return Err(RsnConnectedSupplicantError::ProtocolVersionMismatch);
        }
        if frame.key_info().descriptor_version() != akm.key_descriptor_version() {
            return Err(RsnConnectedSupplicantError::UnsupportedDescriptorVersion);
        }
        if frame.message() != crate::EapolKeyMessage::PairwiseMessage3
            || frame.key_info() != self.key_info
        {
            return Err(RsnConnectedSupplicantError::UnsupportedMessage);
        }
        if frame.nonce() != self.authenticator_nonce {
            return Err(RsnConnectedSupplicantError::AuthenticatorNonceMismatch);
        }
        Ok(())
    }
}

/// Authenticated commitment to the exact Group Message 1 whose GTK was
/// successfully published.
///
/// The retained MIC commits to the complete EAPOL-Key packet with the MIC
/// field cleared. Equality after candidate authentication therefore binds the
/// RSC, encrypted GTK and every other packet byte without retaining key data.
#[derive(Zeroize, ZeroizeOnDrop)]
struct RsnCompletedGroupMessage1 {
    mic: [u8; 16],
}

impl RsnCompletedGroupMessage1 {
    fn capture(frame: EapolKeyFrame<'_>) -> Self {
        Self { mic: *frame.mic() }
    }

    fn matches(&self, frame: EapolKeyFrame<'_>) -> bool {
        self.mic == *frame.mic()
    }
}

enum RsnCompletedKeyMessage {
    Pairwise(RsnCompletedMessage3),
    Group(RsnCompletedGroupMessage1),
}

/// WPA2 state which must survive for the complete connected association.
///
/// It retains only PTK-derived authentication material and replay state. PMK
/// and association retry policy remain owned by the outer station lifecycle.
pub struct RsnConnectedSupplicant {
    authenticator: Eui48,
    key_confirmation: RsnKeyConfirmationKey,
    key_encryption: Kek,
    authenticator_security_ies: OwnedAssociationSecurityIes,
    /// The Message 3 whose keys are installed, which a repeat must match.
    completed_message3: RsnCompletedMessage3,
    /// The last key message whose keys were installed: the replay frontier.
    frontier: Frontier,
    group_management: Option<GroupManagementCipher>,
}

/// The last key message whose keys were installed.
enum Frontier {
    /// The completed Message 3; no Group Message 1 since.
    Message3,
    /// A Group Message 1 after it, which a repeat must match.
    Group {
        replay_counter: oer_protocols_ieee80211_wire::eapol::ReplayCounter,
        completed: RsnCompletedGroupMessage1,
    },
}

impl RsnConnectedSupplicant {
    #[cfg(any(test, feature = "test-support"))]
    pub fn invalidate_security_ies_for_test(&mut self) {
        self.authenticator_security_ies.invalidate_for_test();
    }

    /// The replay frontier: the counter of the last key message whose keys
    /// were installed.
    pub const fn replay_counter(&self) -> oer_protocols_ieee80211_wire::eapol::ReplayCounter {
        match &self.frontier {
            Frontier::Message3 => self.completed_message3.replay_counter,
            Frontier::Group { replay_counter, .. } => *replay_counter,
        }
    }

    /// Authenticate a repeat of Message 3 without producing pairwise work.
    /// An exact repeat reuses M4. A larger counter authenticates and unwraps
    /// the new GTK/IGTK while retaining the installed PTK and its frontiers.
    pub fn on_message3<const N: usize, E: Aes128Engine>(
        &mut self,
        frame: crate::OwnedEapolFrame<N>,
        engine: &mut E,
    ) -> Result<RsnConnectedAction<N>, RsnConnectedSupplicantError> {
        if frame.interface() != RsnInterface::Station {
            return Err(RsnConnectedSupplicantError::WrongInterface);
        }
        if frame.peer() != self.authenticator {
            return Err(RsnConnectedSupplicantError::WrongPeer);
        }
        let key = frame.key_frame()?;
        self.completed_message3
            .validate_fields(self.key_confirmation.akm(), key)?;
        key.verify_mic_with_confirmation_key(&self.key_confirmation, engine)
            .map_err(|failure| match failure {
                MicFailure::Mismatch => RsnConnectedSupplicantError::InvalidMic,
                MicFailure::Engine(error) => RsnConnectedSupplicantError::Engine(error),
            })?;
        if key.replay_counter() == self.completed_message3.replay_counter {
            if key.mic() != &self.completed_message3.mic {
                return Err(RsnConnectedSupplicantError::RetainedMessage3Mismatch);
            }
            let response = RsnTxFrame::message4(
                self.key_confirmation.akm(),
                self.authenticator,
                key.replay_counter(),
            )
            .authenticate_with_confirmation_key(&self.key_confirmation, engine)
            .map_err(RsnConnectedSupplicantError::Engine)?;
            return Ok(RsnConnectedAction::Retransmit(
                response.mark_retransmission(),
            ));
        }
        if key.replay_counter() <= self.replay_counter() {
            return Err(RsnConnectedSupplicantError::StaleReplayCounter);
        }
        self.request_group_unwrap(frame)
    }

    /// Authenticate one Group Message 1.
    ///
    /// An exact repeat of the Group Message 1 whose GTK was published answers
    /// with the cached Group Message 2; a new one returns an affine request
    /// owning its verified frame. Issuing a request does not block validation
    /// of a newer frame while an older response is in flight.
    pub fn on_group_message1<const N: usize, E: Aes128Engine>(
        &mut self,
        frame: crate::OwnedEapolFrame<N>,
        engine: &mut E,
    ) -> Result<RsnConnectedAction<N>, RsnConnectedSupplicantError> {
        if frame.interface() != RsnInterface::Station {
            return Err(RsnConnectedSupplicantError::WrongInterface);
        }
        if frame.peer() != self.authenticator {
            return Err(RsnConnectedSupplicantError::WrongPeer);
        }
        let key = frame.key_frame()?;
        if key.key_info().descriptor_version()
            != self.key_confirmation.akm().key_descriptor_version()
        {
            return Err(RsnConnectedSupplicantError::UnsupportedDescriptorVersion);
        }
        if key.message() != crate::EapolKeyMessage::GroupMessage1 {
            return Err(RsnConnectedSupplicantError::UnsupportedMessage);
        }
        let replay_counter = key.replay_counter();
        // The Group Message 1 whose GTK was installed, when this frame
        // names its counter: a repeat.
        let repeated = match &self.frontier {
            Frontier::Group {
                replay_counter: installed,
                completed,
            } if *installed == replay_counter => Some(completed),
            Frontier::Group { .. } | Frontier::Message3 => None,
        };
        if replay_counter < self.replay_counter()
            || (replay_counter == self.replay_counter() && repeated.is_none())
        {
            return Err(RsnConnectedSupplicantError::StaleReplayCounter);
        }
        // A cached Group Message 2 is still an authenticated response. Verify
        // every repeated Group Message 1 before admitting the idempotent path,
        // otherwise a forged frame can elicit a valid MIC-bearing response.
        key.verify_mic_with_confirmation_key(&self.key_confirmation, engine)
            .map_err(|failure| match failure {
                MicFailure::Mismatch => RsnConnectedSupplicantError::InvalidMic,
                MicFailure::Engine(error) => RsnConnectedSupplicantError::Engine(error),
            })?;
        if let Some(completed) = repeated {
            if !completed.matches(key) {
                return Err(RsnConnectedSupplicantError::RetainedGroupMessage1Mismatch);
            }
            let response = RsnTxFrame::group_message2(
                self.key_confirmation.akm(),
                self.authenticator,
                replay_counter,
            )
            .authenticate_with_confirmation_key(&self.key_confirmation, engine)
            .map_err(RsnConnectedSupplicantError::Engine)?;
            return Ok(RsnConnectedAction::Retransmit(response));
        }
        self.request_group_unwrap(frame)
    }

    fn request_group_unwrap<const N: usize>(
        &mut self,
        frame: crate::OwnedEapolFrame<N>,
    ) -> Result<RsnConnectedAction<N>, RsnConnectedSupplicantError> {
        let key = frame.key_frame()?;
        if !key.key_info().encrypted_key_data() || key.key_data().is_empty() {
            return Err(RsnConnectedSupplicantError::MissingEncryptedKeyData);
        }
        Ok(RsnConnectedAction::GroupKeyData(RsnGroupKeyFrame {
            replay_counter: key.replay_counter(),
            frame,
        }))
    }

    /// Unwrap the key data of a frame accepted by [`Self::on_message3`] or
    /// [`Self::on_group_message1`] over `engine` and prepare the exact GTK
    /// publication.
    ///
    /// Key data that does not unwrap or parse returns the error and leaves the
    /// supplicant ready for the next Group Message 1, with nothing retained.
    pub fn prepare_group_key_install<const N: usize, E: Aes128Engine>(
        &mut self,
        frame: RsnGroupKeyFrame<N>,
        engine: &mut E,
    ) -> Result<RsnGroupKeyInstallRequest<N>, RsnConnectedProcessError> {
        let RsnGroupKeyFrame {
            frame,
            replay_counter: verified,
        } = frame;
        if verified <= self.replay_counter() {
            return Err(RsnConnectedProcessError::Supplicant(
                RsnConnectedSupplicantError::StaleCompletion,
            ));
        }
        let key = frame.key_frame()?;
        let replay_counter = key.replay_counter();
        let plain = unwrap_key_data(engine, &self.key_encryption, key.key_data())
            .map_err(KeyDataError::into_connected)?;
        let (keys, commitment, response) = match key.message() {
            crate::EapolKeyMessage::PairwiseMessage3 => (
                parse_gtk_key_data(
                    plain.as_bytes(),
                    self.authenticator_security_ies
                        .rsn_ie()
                        .map_err(RsnConnectedSupplicantError::RetainedSecurityIes)?,
                    self.authenticator_security_ies
                        .rsnxe()
                        .map_err(RsnConnectedSupplicantError::RetainedSecurityIes)?,
                    self.group_management,
                ),
                RsnCompletedKeyMessage::Pairwise(RsnCompletedMessage3::capture(key)),
                RsnTxFrame::message4(
                    self.key_confirmation.akm(),
                    self.authenticator,
                    replay_counter,
                ),
            ),
            crate::EapolKeyMessage::GroupMessage1 => (
                parse_group_gtk_key_data(plain.as_bytes(), self.group_management),
                RsnCompletedKeyMessage::Group(RsnCompletedGroupMessage1::capture(key)),
                RsnTxFrame::group_message2(
                    self.key_confirmation.akm(),
                    self.authenticator,
                    replay_counter,
                ),
            ),
            _ => {
                return Err(RsnConnectedProcessError::Supplicant(
                    RsnConnectedSupplicantError::UnsupportedMessage,
                ));
            }
        };
        let map_error =
            |error| RsnConnectedProcessError::Supplicant(RsnConnectedSupplicantError::Frame(error));
        let RsnGroupKeys { gtk, igtk } = keys.map_err(map_error)?;
        let group = RsnKeyInstall::group(RsnInterface::Station, &gtk, *key.key_receive_sequence());
        let response = response
            .authenticate_with_confirmation_key(&self.key_confirmation, engine)
            .map_err(|error| {
                RsnConnectedProcessError::Supplicant(RsnConnectedSupplicantError::Engine(error))
            })?;
        Ok(RsnGroupKeyInstallRequest {
            replay_counter,
            commitment,
            group,
            igtk,
            response,
        })
    }

    /// The platform installed the group keys of `request`: the response
    /// follows. An installation that did not take effect never comes here:
    /// its owner ends the attempt.
    pub fn complete_group_key_install<const N: usize>(
        &mut self,
        request: RsnGroupKeyInstallRequest<N>,
    ) -> Result<RsnTxFrame<N>, RsnConnectedSupplicantError> {
        if request.replay_counter <= self.replay_counter() {
            return Err(RsnConnectedSupplicantError::StaleCompletion);
        }
        match request.commitment {
            RsnCompletedKeyMessage::Pairwise(completed) => {
                self.completed_message3 = completed;
                self.frontier = Frontier::Message3;
            }
            RsnCompletedKeyMessage::Group(completed) => {
                self.frontier = Frontier::Group {
                    replay_counter: request.replay_counter,
                    completed,
                };
            }
        }
        Ok(request.response)
    }
}

/// One station's four-way handshake with the access point it associated
/// with: a sum by phase whose steps run the handshake's cryptography in
/// place, over the engine each step is lent.
pub struct RsnStaHandshake {
    akm: Akm,
    local: Eui48,
    authenticator: Eui48,
    supplicant_nonce: oer_protocols_ieee80211_wire::eapol::Nonce,
    /// The station's Association RSN element and RSNXE, which Message 2
    /// echoes.
    association_security_ies: OwnedAssociationSecurityIes,
    /// The access point's, which Message 3's key data must repeat.
    authenticator_security_ies: OwnedAssociationSecurityIes,
    /// The group management cipher, when both RSN elements advertise MFPC:
    /// management frames are protected.
    group_management: Option<GroupManagementCipher>,
    phase: Phase,
}

/// What exists in each phase.
enum Phase {
    /// No Message 1 was accepted.
    AwaitingMessage1,
    /// A Message 1 was accepted: its PTK, through Message 3 and the keys'
    /// installation.
    Keyed {
        ptk: Ptk,
        authenticator_nonce: oer_protocols_ieee80211_wire::eapol::Nonce,
        message1_replay: oer_protocols_ieee80211_wire::eapol::ReplayCounter,
        stage: Stage,
    },
}

/// What exists beside the PTK.
enum Stage {
    /// Message 2 is built; Message 3 is awaited.
    AwaitingMessage3,
    /// Message 3 verified: its keys are being installed, and the frame is
    /// kept as the commitment a repeat must match.
    Installing { completed: RsnCompletedMessage3 },
}

impl RsnStaHandshake {
    #[cfg(any(test, feature = "test-support"))]
    pub fn invalidate_security_ies_for_test(&mut self) {
        self.authenticator_security_ies.invalidate_for_test();
    }

    /// A handshake of the association whose security IEs the station sent,
    /// with the access point's RSN element and RSNXE. A refusal keeps
    /// nothing.
    pub fn new(
        local: Eui48,
        authenticator: Eui48,
        supplicant_nonce: oer_protocols_ieee80211_wire::eapol::Nonce,
        association_security_ies: &[u8],
        authenticator_rsn_ie: &[u8],
        authenticator_rsnxe: &[u8],
    ) -> Result<Self, RsnStaNewError> {
        if supplicant_nonce.is_zero() {
            return Err(RsnStaNewError::ZeroNonce);
        }
        let association_security_ies =
            OwnedAssociationSecurityIes::try_copy_bytes(association_security_ies)
                .map_err(RsnStaNewError::Frame)?;
        // The station's own Association RSN element names the negotiated suite.
        let own = validate_rsn_element(
            association_security_ies
                .rsn_ie()
                .map_err(RsnStaNewError::Frame)?,
        )
        .map_err(RsnStaNewError::Element)?;
        let peer_protection = validate_rsn_element(authenticator_rsn_ie)
            .map_err(RsnStaNewError::Element)?
            .management_frame_protection();
        // Protection is in effect when both parties are capable; the
        // validated elements carry BIP-CMAC-128 then.
        let group_management = (own.management_frame_protection().capable()
            && peer_protection.capable())
        .then_some(GroupManagementCipher::BipCmac128);
        let authenticator_rsn_ie =
            OwnedRsnIe::<{ crate::frames::RSN_IE_CAPACITY }>::try_copy(authenticator_rsn_ie)
                .map_err(RsnStaNewError::Frame)?;
        let authenticator_security_ies =
            OwnedAssociationSecurityIes::try_copy(&authenticator_rsn_ie, authenticator_rsnxe)
                .map_err(RsnStaNewError::Frame)?;
        Ok(Self {
            akm: own.akm(),
            local,
            authenticator,
            supplicant_nonce,
            association_security_ies,
            authenticator_security_ies,
            group_management,
            phase: Phase::AwaitingMessage1,
        })
    }

    /// The group management cipher, when the association protects its
    /// management frames.
    pub const fn group_management(&self) -> Option<GroupManagementCipher> {
        self.group_management
    }

    /// Take one EAPOL-Key frame of the access point with the association's
    /// `pmk`.
    ///
    /// A Message 1 derives the PTK and builds Message 2 in the same step. A
    /// Message 3 is checked against the accepted Message 1, its MIC
    /// verified and its key data unwrapped and parsed on `engine`, all
    /// before it changes anything. A refusal changes nothing; key data that
    /// does not unwrap or parse ends the handshake.
    #[inline(never)]
    pub fn on_eapol<const N: usize, E: Aes128Engine>(
        &mut self,
        frame: crate::OwnedEapolFrame<N>,
        pmk: &Pmk,
        engine: &mut E,
    ) -> Result<RsnStaEapol<N>, RsnStaError> {
        if frame.interface() != RsnInterface::Station {
            return Err(RsnStaError::Rejected(RsnStateError::WrongInterface));
        }
        if frame.peer() != self.authenticator {
            return Err(RsnStaError::Rejected(RsnStateError::WrongPeer));
        }
        let key = frame.key_frame()?;
        if key.key_info().descriptor_version() != self.akm.key_descriptor_version() {
            return Err(RsnStaError::Rejected(
                RsnStateError::UnsupportedDescriptorVersion,
            ));
        }
        match key.message() {
            crate::EapolKeyMessage::PairwiseMessage1 => self.on_message1(key, pmk, engine),
            crate::EapolKeyMessage::PairwiseMessage3 => self.on_message3(key, engine),
            _ => Err(RsnStaError::Rejected(RsnStateError::UnsupportedMessage)),
        }
    }

    fn on_message1<const N: usize, E: Aes128Engine>(
        &mut self,
        key: EapolKeyFrame<'_>,
        pmk: &Pmk,
        engine: &mut E,
    ) -> Result<RsnStaEapol<N>, RsnStaError> {
        let replay = key.replay_counter();
        let nonce = key.nonce();
        if nonce.is_zero() {
            return Err(RsnStaError::Rejected(RsnStateError::ZeroNonce));
        }
        match &self.phase {
            Phase::AwaitingMessage1 => {}
            Phase::Keyed {
                ptk,
                authenticator_nonce,
                message1_replay,
                stage: Stage::AwaitingMessage3,
            } => {
                if replay == *message1_replay && nonce == *authenticator_nonce {
                    // A repeat of the accepted Message 1: Message 2 again.
                    let frame = message2(self.facts(replay), ptk, engine)?;
                    return Ok(RsnStaEapol::Transmit(frame.mark_retransmission()));
                }
                if replay < *message1_replay {
                    return Err(RsnStaError::Rejected(RsnStateError::StaleReplayCounter));
                }
            }
            Phase::Keyed {
                message1_replay,
                stage: Stage::Installing { .. },
                ..
            } => {
                return if replay <= *message1_replay {
                    Ok(RsnStaEapol::None)
                } else {
                    Err(RsnStaError::Rejected(RsnStateError::UnexpectedMessage))
                };
            }
        }
        // A new Message 1: its PTK and Message 2, built before the phase
        // changes, so that a refused MIC changes nothing.
        let ptk = pmk.derive_ptk(
            self.akm,
            PtkContext {
                authenticator_address: self.authenticator,
                supplicant_address: self.local,
                authenticator_nonce: nonce,
                supplicant_nonce: self.supplicant_nonce,
            },
        );
        let frame = message2(self.facts(replay), &ptk, engine)?;
        self.phase = Phase::Keyed {
            ptk,
            authenticator_nonce: nonce,
            message1_replay: replay,
            stage: Stage::AwaitingMessage3,
        };
        Ok(RsnStaEapol::Transmit(frame))
    }

    fn on_message3<const N: usize, E: Aes128Engine>(
        &mut self,
        key: EapolKeyFrame<'_>,
        engine: &mut E,
    ) -> Result<RsnStaEapol<N>, RsnStaError> {
        let Self {
            akm,
            authenticator,
            authenticator_security_ies,
            group_management,
            phase,
            ..
        } = self;
        let Phase::Keyed {
            ptk,
            authenticator_nonce,
            message1_replay,
            stage,
        } = phase
        else {
            return Err(RsnStaError::Rejected(RsnStateError::UnexpectedMessage));
        };
        let replay = key.replay_counter();
        match stage {
            Stage::AwaitingMessage3 => {}
            Stage::Installing { completed } => {
                return if replay == completed.replay_counter {
                    Ok(RsnStaEapol::None)
                } else if replay < completed.replay_counter {
                    Err(RsnStaError::Rejected(RsnStateError::StaleReplayCounter))
                } else {
                    Err(RsnStaError::Rejected(RsnStateError::UnexpectedMessage))
                };
            }
        }
        check_message3_fields(*akm, key, authenticator_nonce).map_err(RsnStaError::Rejected)?;
        if replay <= *message1_replay {
            return Err(RsnStaError::Rejected(RsnStateError::StaleReplayCounter));
        }
        match key.check_mic(ptk, engine) {
            Ok(()) => {}
            // A forged Message 3 is refused and stores nothing; the real one
            // may still come.
            Err(MicFailure::Mismatch) => {
                return Err(RsnStaError::Rejected(RsnStateError::InvalidMic));
            }
            Err(MicFailure::Engine(error)) => return Err(RsnStaError::Engine(error)),
        }
        let rsn_ie = authenticator_security_ies
            .rsn_ie()
            .map_err(RsnStaError::RetainedSecurityIes)?;
        let rsnxe = authenticator_security_ies
            .rsnxe()
            .map_err(RsnStaError::RetainedSecurityIes)?;
        let plain = match unwrap_key_data(engine, ptk.kek(), key.key_data()) {
            Ok(plain) => plain,
            Err(error) => return Ok(RsnStaEapol::End(error.into_end())),
        };
        let RsnGroupKeys { gtk, igtk } =
            match parse_gtk_key_data(plain.as_bytes(), rsn_ie, rsnxe, *group_management) {
                Ok(keys) => keys,
                Err(error) => return Ok(RsnStaEapol::End(RsnStaEnd::KeyData(error))),
            };
        let request = RsnStaKeyInstallRequest {
            pairwise: RsnKeyInstall::pairwise(RsnInterface::Station, *authenticator, [0; 8], ptk),
            group: RsnStaGroupInstall {
                replay_counter: replay,
                plain_key_data_len: plain.as_bytes().len(),
                group: RsnKeyInstall::group(
                    RsnInterface::Station,
                    &gtk,
                    *key.key_receive_sequence(),
                ),
                igtk,
            },
        };
        *stage = Stage::Installing {
            completed: RsnCompletedMessage3::capture(key),
        };
        Ok(RsnStaEapol::Install(request))
    }

    /// The installs of Message 3 ended: Message 4, authenticated on
    /// `engine`, and the connected supplicant the handshake becomes. A
    /// refused MIC ends the handshake with its cause.
    pub fn complete<const N: usize, E: Aes128Engine>(
        self,
        installed: RsnStaInstalled,
        engine: &mut E,
    ) -> Result<RsnStaConnected<N>, RsnStaEnd> {
        let RsnStaInstalled(()) = installed;
        let Self {
            akm,
            authenticator,
            authenticator_security_ies,
            group_management,
            phase,
            ..
        } = self;
        let Phase::Keyed {
            ptk,
            stage: Stage::Installing { completed },
            ..
        } = phase
        else {
            return Err(RsnStaEnd::Unrequested);
        };
        let message4 = RsnTxFrame::message4(akm, authenticator, completed.replay_counter)
            .authenticate(&ptk, engine)
            .map_err(RsnStaEnd::Engine)?;
        let (key_confirmation, key_encryption) = ptk.into_connected_keys();
        Ok(RsnStaConnected {
            supplicant: RsnConnectedSupplicant {
                authenticator,
                key_confirmation,
                key_encryption,
                authenticator_security_ies,
                completed_message3: completed,
                frontier: Frontier::Message3,
                group_management,
            },
            message4,
        })
    }

    /// What Message 2 of `replay` is built from beside its PTK.
    fn facts(
        &self,
        replay: oer_protocols_ieee80211_wire::eapol::ReplayCounter,
    ) -> Message2Facts<'_> {
        Message2Facts {
            akm: self.akm,
            authenticator: self.authenticator,
            supplicant_nonce: self.supplicant_nonce,
            replay,
            security_ies: &self.association_security_ies,
        }
    }
}

/// What Message 2 is built from beside its PTK.
struct Message2Facts<'a> {
    akm: Akm,
    authenticator: Eui48,
    supplicant_nonce: oer_protocols_ieee80211_wire::eapol::Nonce,
    replay: oer_protocols_ieee80211_wire::eapol::ReplayCounter,
    security_ies: &'a OwnedAssociationSecurityIes,
}

/// Message 2 of `facts`, its MIC computed under `ptk` on `engine`.
fn message2<const N: usize, E: Aes128Engine>(
    facts: Message2Facts<'_>,
    ptk: &Ptk,
    engine: &mut E,
) -> Result<RsnTxFrame<N>, RsnStaError> {
    RsnTxFrame::message2_with_security_ies(
        facts.akm,
        facts.authenticator,
        facts.replay,
        facts.supplicant_nonce,
        facts.security_ies,
    )
    .authenticate(ptk, engine)
    .map_err(RsnStaError::Engine)
}

/// Whether Message 3's fields are those of an authenticator's Message 3 for
/// `akm`, bound to the accepted Message 1's ANonce.
fn check_message3_fields(
    akm: Akm,
    key: EapolKeyFrame<'_>,
    authenticator_nonce: &oer_protocols_ieee80211_wire::eapol::Nonce,
) -> Result<(), RsnStateError> {
    if key.key_info() != crate::state::pairwise_message3_key_info(akm) {
        return Err(RsnStateError::InvalidMessage3KeyInfo);
    }
    if key.key_length() != crate::state::RSN_CCMP_TEMPORAL_KEY_LEN {
        return Err(RsnStateError::InvalidKeyLength);
    }
    if key.nonce() != *authenticator_nonce {
        return Err(RsnStateError::AuthenticatorNonceMismatch);
    }
    if key.key_iv().iter().any(|byte| *byte != 0) {
        return Err(RsnStateError::NonzeroKeyIv);
    }
    if key.key_identifier().iter().any(|byte| *byte != 0) {
        return Err(RsnStateError::NonzeroKeyIdentifier);
    }
    if key.key_data().is_empty() {
        return Err(RsnStateError::MissingEncryptedKeyData);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
