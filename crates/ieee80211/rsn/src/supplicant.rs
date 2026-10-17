//! Allocation-free RSN supplicant orchestration.
//!
//! [`RsnStaState`] remains the protocol transition owner. This module joins
//! those transitions to PMK/PTK derivation, MIC verification, exact
//! Association security IEs and typed key-data unwrap and key-install
//! requests. It never waits: the caller performs each requested AES key
//! unwrap (synchronously or through [`crate::aes::AsyncRsnKeyUnwrap`]) and
//! completes the request, so a target handles only complete EAPOL RX/TX frames,
//! the unwrap and the platform-specific key-slot transaction.

use crate::AkmKeys;
use crate::{
    Akm, EapolKeyFrame, Pmk, Ptk, PtkContext, RSN_KEK_LEN, RsnInterface, RsnKeyConfirmationKey,
    RsnKeyEncryptionKey,
    aes::RsnUnwrappedKeyData,
    element::{RsnElementError, validate_rsn_element},
    frames::{
        OwnedAssociationSecurityIes, OwnedRsnIe, RsnFrameError, RsnGroupKeys, RsnIgtk, RsnTxFrame,
        build_sta_action_frame, parse_group_gtk_key_data, parse_gtk_key_data,
    },
    keys::RsnKeyInstall,
    state::{RsnStaAction, RsnStaPhase, RsnStaState, RsnStateError, RsnTransmit},
};
use oer_time::{Duration, Instant};
use zeroize::{Zeroize, ZeroizeOnDrop};

/// Hardware-qualified open-STA compatibility window for the first EAPOL-Key
/// message. The numeric value is retained from the earlier hardware-tested
/// policy; it is not claimed as a recovered vendor constant.
pub const RSN_STA_MESSAGE1_TIMEOUT: Duration = Duration::from_secs(3);

/// Complete window after the original Message 2 transmission.
///
/// SOURCE: complete `libwpa_supplicant.a[wpa.c.obj]::
/// wpa_supplicant_process_1_of_4` emits one Message 2 and does not register a
/// retransmission timeout. A repeated Message 1 instead re-enters through
/// `wpa_sm_rx_eapol` and produces another Message 2. The six-second value
/// preserves the two former three-second hardware receive windows without
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RsnStaSupplicantError {
    State(RsnStateError),
    Frame(RsnFrameError),
    /// The station's own Association RSN element names no supported suite.
    Element(RsnElementError),
    MissingPtk,
    InvalidMessage3Mic,
    UnexpectedAction,
}

impl RsnStaSupplicantError {
    /// Rejects caused by the current unauthenticated peer frame are
    /// recoverable within the existing response window. Local state,
    /// cryptographic ownership and frame-construction failures are not.
    pub const fn is_peer_input_rejection(self) -> bool {
        match self {
            Self::State(error) => error.is_peer_input_rejection(),
            Self::InvalidMessage3Mic => true,
            Self::Frame(_) | Self::Element(_) | Self::MissingPtk | Self::UnexpectedAction => false,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RsnStaProcessError<E> {
    Supplicant(RsnStaSupplicantError),
    KeyUnwrap(E),
}

impl<E> RsnStaProcessError<E> {
    pub const fn is_peer_input_rejection(&self) -> bool {
        match self {
            Self::Supplicant(error) => error.is_peer_input_rejection(),
            Self::KeyUnwrap(_) => false,
        }
    }
}

impl<E> From<RsnStaSupplicantError> for RsnStaProcessError<E> {
    fn from(error: RsnStaSupplicantError) -> Self {
        Self::Supplicant(error)
    }
}

/// Complete pairwise/group installation transaction prepared from Message 3.
///
/// Both keys are owned and zeroized on drop. The private state ticket makes a
/// completion valid only for the exact Message 3 that produced this request.
#[must_use]
pub struct RsnStaKeyInstallRequest {
    pairwise: RsnKeyInstall,
    group: RsnStaRetryInstall,
}

impl RsnStaKeyInstallRequest {
    pub const fn replay_counter(&self) -> u64 {
        self.group.replay_counter()
    }
    pub const fn encrypted_key_data(&self) -> bool {
        self.group.encrypted_key_data()
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

    /// Move the pairwise installation to its consumer and retain group
    /// publication authority separately. No temporal-key bytes are copied.
    pub fn into_parts(self) -> (RsnKeyInstall, RsnStaRetryInstall) {
        (self.pairwise, self.group)
    }
}

/// Group-only work from an authenticated repeated M3. It cannot publish PTK
/// material or reset its packet numbers because it contains no pairwise key.
#[must_use]
pub struct RsnStaRetryInstall {
    ticket: crate::state::RsnTicket,
    replay_counter: u64,
    encrypted_key_data: bool,
    plain_key_data_len: usize,
    group: RsnKeyInstall,
    igtk: Option<RsnIgtk>,
}

impl RsnStaRetryInstall {
    pub const fn replay_counter(&self) -> u64 {
        self.replay_counter
    }
    pub const fn encrypted_key_data(&self) -> bool {
        self.encrypted_key_data
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
}

/// RFC 3394 unwrap of the encrypted key data of one authenticated Message 3.
///
/// The caller unwraps [`Self::wrapped_key_data`] under [`Self::kek`] and
/// returns the result to [`RsnStaSupplicant::complete_key_data_unwrap`]; until
/// then the handshake waits in its key-data phase. The KEK copy is cleared on
/// drop, and the private state ticket makes the completion valid only for the
/// exact Message 3 that produced this request.
pub struct RsnStaKeyDataRequest<const N: usize> {
    ticket: crate::state::RsnTicket,
    frame: crate::OwnedEapolFrame<N>,
    kek: RsnKeyEncryptionKey,
}

impl<const N: usize> RsnStaKeyDataRequest<N> {
    /// Key-encryption key of the PTK that authenticated Message 3.
    pub const fn kek(&self) -> &[u8; RSN_KEK_LEN] {
        self.kek.as_bytes()
    }

    /// The wrapped Key Data field of Message 3.
    pub fn wrapped_key_data(&self) -> &[u8] {
        self.frame.key_frame().key_data()
    }

    pub fn replay_counter(&self) -> u64 {
        self.frame.key_frame().replay_counter()
    }
}

#[must_use]
pub enum RsnStaSupplicantAction<const N: usize> {
    Ready(RsnStaReady<N>),
    /// Unwrap this owned request and complete it before applying an effect.
    UnwrapKeyData(RsnStaKeyDataRequest<N>),
}

/// Resolved effects contain no outstanding key-data unwrap request.
#[must_use]
pub enum RsnStaReady<const N: usize> {
    None,
    Transmit(RsnTxFrame<N>),
    InstallKeys(RsnStaKeyInstallRequest),
    RetryInstall(RsnStaRetryInstall),
    Deauthenticate,
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
    InstallFailed,
    Frame(RsnFrameError),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RsnConnectedProcessError<E> {
    Supplicant(RsnConnectedSupplicantError),
    KeyUnwrap(E),
}

/// Exact connected-state GTK publication ticket.
pub struct RsnGroupKeyInstallRequest<const N: usize> {
    replay_counter: u64,
    commitment: RsnCompletedKeyMessage,
    group: RsnKeyInstall,
    igtk: Option<RsnIgtk>,
    response: RsnTxFrame<N>,
}

impl<const N: usize> RsnGroupKeyInstallRequest<N> {
    pub const fn replay_counter(&self) -> u64 {
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

/// RFC 3394 unwrap of the encrypted key data of one authenticated Group
/// Message 1.
///
/// The caller unwraps [`Self::wrapped_key_data`] under [`Self::kek`] and
/// returns the result to
/// [`RsnConnectedSupplicant::complete_group_key_data_unwrap`]. The supplicant
/// request is an affine owner of the authenticated frame and KEK. A station
/// may retain it until the current TX ends, or replace it with a newer checked
/// frame. No ticket or busy latch is needed; completion checks the committed
/// replay frontier. The KEK copy is cleared on drop.
pub struct RsnGroupKeyDataRequest<const N: usize> {
    frame: crate::OwnedEapolFrame<N>,
    kek: RsnKeyEncryptionKey,
}

impl<const N: usize> RsnGroupKeyDataRequest<N> {
    /// Key-encryption key of the connected association.
    pub const fn kek(&self) -> &[u8; RSN_KEK_LEN] {
        self.kek.as_bytes()
    }

    /// The wrapped Key Data field of Group Message 1.
    pub fn wrapped_key_data(&self) -> &[u8] {
        self.frame.key_frame().key_data()
    }

    pub fn replay_counter(&self) -> u64 {
        self.frame.key_frame().replay_counter()
    }
    /// Return the authenticated frame when its exchange is disabled. Consuming
    /// the request also clears its private KEK copy.
    pub fn into_frame(self) -> crate::OwnedEapolFrame<N> {
        let Self { frame, kek } = self;
        drop(kek);
        frame
    }
}

pub enum RsnConnectedAction<const N: usize> {
    /// A new authenticated Group Message 1: unwrap its key data, then call
    /// [`RsnConnectedSupplicant::complete_group_key_data_unwrap`].
    UnwrapGroupKeyData(RsnGroupKeyDataRequest<N>),
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
    protocol_version: u8,
    key_info: u16,
    replay_counter: u64,
    authenticator_nonce: [u8; 32],
    mic: [u8; 16],
}

impl RsnCompletedMessage3 {
    fn capture(frame: EapolKeyFrame<'_>) -> Self {
        Self {
            protocol_version: frame.protocol_version(),
            key_info: frame.key_info().raw(),
            replay_counter: frame.replay_counter(),
            authenticator_nonce: *frame.nonce(),
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
            || frame.key_info().raw() != self.key_info
        {
            return Err(RsnConnectedSupplicantError::UnsupportedMessage);
        }
        if frame.nonce() != &self.authenticator_nonce {
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
    authenticator: [u8; 6],
    key_confirmation: RsnKeyConfirmationKey,
    key_encryption: RsnKeyEncryptionKey,
    authenticator_security_ies: OwnedAssociationSecurityIes,
    completed_message3: RsnCompletedMessage3,
    replay_counter: u64,
    completed_group_message1: Option<RsnCompletedGroupMessage1>,
    management_protection: bool,
}

impl RsnConnectedSupplicant {
    pub const fn replay_counter(&self) -> u64 {
        self.replay_counter
    }

    /// Authenticate a repeat of Message 3 without producing pairwise work.
    /// An exact repeat reuses M4. A larger counter authenticates and unwraps
    /// the new GTK/IGTK while retaining the installed PTK and its frontiers.
    pub fn on_message3<const N: usize>(
        &mut self,
        frame: crate::OwnedEapolFrame<N>,
    ) -> Result<RsnConnectedAction<N>, RsnConnectedSupplicantError> {
        if frame.interface() != RsnInterface::Station {
            return Err(RsnConnectedSupplicantError::WrongInterface);
        }
        if frame.peer() != &self.authenticator {
            return Err(RsnConnectedSupplicantError::WrongPeer);
        }
        let key = frame.key_frame();
        self.completed_message3
            .validate_fields(self.key_confirmation.akm(), key)?;
        if !key.verify_mic_with_confirmation_key(&self.key_confirmation) {
            return Err(RsnConnectedSupplicantError::InvalidMic);
        }
        if key.replay_counter() == self.completed_message3.replay_counter {
            if key.mic() != &self.completed_message3.mic {
                return Err(RsnConnectedSupplicantError::RetainedMessage3Mismatch);
            }
            return RsnTxFrame::message4(
                self.key_confirmation.akm(),
                self.authenticator,
                key.replay_counter(),
            )
            .map_err(RsnConnectedSupplicantError::Frame)
            .map(|response| {
                RsnConnectedAction::Retransmit(
                    response
                        .authenticate_with_confirmation_key(&self.key_confirmation)
                        .mark_retransmission(),
                )
            });
        }
        if key.replay_counter() <= self.replay_counter {
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
    pub fn on_group_message1<const N: usize>(
        &mut self,
        frame: crate::OwnedEapolFrame<N>,
    ) -> Result<RsnConnectedAction<N>, RsnConnectedSupplicantError> {
        if frame.interface() != RsnInterface::Station {
            return Err(RsnConnectedSupplicantError::WrongInterface);
        }
        if frame.peer() != &self.authenticator {
            return Err(RsnConnectedSupplicantError::WrongPeer);
        }
        let key = frame.key_frame();
        if key.key_info().descriptor_version()
            != self.key_confirmation.akm().key_descriptor_version()
        {
            return Err(RsnConnectedSupplicantError::UnsupportedDescriptorVersion);
        }
        if key.message() != crate::EapolKeyMessage::GroupMessage1 {
            return Err(RsnConnectedSupplicantError::UnsupportedMessage);
        }
        let replay_counter = key.replay_counter();
        if replay_counter < self.replay_counter
            || (replay_counter == self.replay_counter && self.completed_group_message1.is_none())
        {
            return Err(RsnConnectedSupplicantError::StaleReplayCounter);
        }
        // A cached Group Message 2 is still an authenticated response. Verify
        // every repeated Group Message 1 before admitting the idempotent path,
        // otherwise a forged frame can elicit a valid MIC-bearing response.
        if !key.verify_mic_with_confirmation_key(&self.key_confirmation) {
            return Err(RsnConnectedSupplicantError::InvalidMic);
        }
        if let Some(completed) = self
            .completed_group_message1
            .as_ref()
            .filter(|_| replay_counter == self.replay_counter)
        {
            if !completed.matches(key) {
                return Err(RsnConnectedSupplicantError::RetainedGroupMessage1Mismatch);
            }
            let response = RsnTxFrame::group_message2(
                self.key_confirmation.akm(),
                self.authenticator,
                replay_counter,
            )
            .map_err(RsnConnectedSupplicantError::Frame)?
            .authenticate_with_confirmation_key(&self.key_confirmation);
            return Ok(RsnConnectedAction::Retransmit(response));
        }
        self.request_group_unwrap(frame)
    }

    fn request_group_unwrap<const N: usize>(
        &mut self,
        frame: crate::OwnedEapolFrame<N>,
    ) -> Result<RsnConnectedAction<N>, RsnConnectedSupplicantError> {
        let key = frame.key_frame();
        if !key.key_info().encrypted_key_data() || key.key_data().is_empty() {
            return Err(RsnConnectedSupplicantError::MissingEncryptedKeyData);
        }
        Ok(RsnConnectedAction::UnwrapGroupKeyData(
            RsnGroupKeyDataRequest {
                frame,
                kek: RsnKeyEncryptionKey::copy_from(self.key_encryption.as_bytes()),
            },
        ))
    }

    /// Complete the key-data unwrap requested by [`Self::on_group_message1`]
    /// and prepare the exact GTK publication.
    ///
    /// A failed unwrap or unusable key data returns the error and leaves the
    /// supplicant ready for the next Group Message 1, with nothing retained.
    pub fn complete_group_key_data_unwrap<const N: usize, E>(
        &mut self,
        request: RsnGroupKeyDataRequest<N>,
        unwrapped: Result<RsnUnwrappedKeyData, E>,
    ) -> Result<RsnGroupKeyInstallRequest<N>, RsnConnectedProcessError<E>> {
        let RsnGroupKeyDataRequest { frame, kek } = request;
        drop(kek);
        let replay_counter = frame.key_frame().replay_counter();
        if replay_counter <= self.replay_counter {
            return Err(RsnConnectedProcessError::Supplicant(
                RsnConnectedSupplicantError::StaleCompletion,
            ));
        }
        self.prepare_group_key_install(&frame, unwrapped)
    }

    fn prepare_group_key_install<const N: usize, E>(
        &self,
        frame: &crate::OwnedEapolFrame<N>,
        unwrapped: Result<RsnUnwrappedKeyData, E>,
    ) -> Result<RsnGroupKeyInstallRequest<N>, RsnConnectedProcessError<E>> {
        let key = frame.key_frame();
        let replay_counter = key.replay_counter();
        let plain = unwrapped.map_err(RsnConnectedProcessError::KeyUnwrap)?;
        let (keys, commitment, response) = match key.message() {
            crate::EapolKeyMessage::PairwiseMessage3 => (
                parse_gtk_key_data(
                    plain.as_bytes(),
                    self.authenticator_security_ies.rsn_ie(),
                    self.authenticator_security_ies.rsnxe(),
                    self.management_protection,
                ),
                RsnCompletedKeyMessage::Pairwise(RsnCompletedMessage3::capture(key)),
                RsnTxFrame::message4(
                    self.key_confirmation.akm(),
                    self.authenticator,
                    replay_counter,
                ),
            ),
            crate::EapolKeyMessage::GroupMessage1 => (
                parse_group_gtk_key_data(plain.as_bytes(), self.management_protection),
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
            .map_err(map_error)?
            .authenticate_with_confirmation_key(&self.key_confirmation);
        Ok(RsnGroupKeyInstallRequest {
            replay_counter,
            commitment,
            group,
            igtk,
            response,
        })
    }

    pub fn complete_group_key_install<const N: usize>(
        &mut self,
        request: RsnGroupKeyInstallRequest<N>,
        installed: bool,
    ) -> Result<RsnTxFrame<N>, RsnConnectedSupplicantError> {
        if request.replay_counter <= self.replay_counter {
            return Err(RsnConnectedSupplicantError::StaleCompletion);
        }
        if !installed {
            return Err(RsnConnectedSupplicantError::InstallFailed);
        }
        self.replay_counter = request.replay_counter;
        match request.commitment {
            RsnCompletedKeyMessage::Pairwise(completed) => {
                self.completed_message3 = completed;
                self.completed_group_message1 = None;
            }
            RsnCompletedKeyMessage::Group(completed) => {
                self.completed_group_message1 = Some(completed);
            }
        }
        Ok(request.response)
    }
}

/// One WPA2-Personal station handshake with owned cryptographic state.
pub struct RsnStaSupplicant {
    state: RsnStaState,
    ptk: Option<Ptk>,
    completed_message3: Option<RsnCompletedMessage3>,
    association_security_ies: OwnedAssociationSecurityIes,
    authenticator_security_ies: OwnedAssociationSecurityIes,
    /// Both RSN elements advertise MFPC: management frames are protected.
    management_protection: bool,
}

impl RsnStaSupplicant {
    pub fn try_new(
        local: [u8; 6],
        authenticator: [u8; 6],
        supplicant_nonce: [u8; 32],
        association_security_ies: &[u8],
        authenticator_rsn_ie: &[u8],
        authenticator_rsnxe: &[u8],
    ) -> Result<Self, RsnStaSupplicantError> {
        let association_security_ies =
            OwnedAssociationSecurityIes::try_copy_bytes(association_security_ies)
                .map_err(RsnStaSupplicantError::Frame)?;
        // The station's own Association RSN element names the negotiated suite.
        let own = validate_rsn_element(association_security_ies.rsn_ie())
            .map_err(RsnStaSupplicantError::Element)?;
        let akm = own.akm();
        let management_protection = own.management_frame_protection().negotiated(
            validate_rsn_element(authenticator_rsn_ie)
                .map_err(RsnStaSupplicantError::Element)?
                .management_frame_protection(),
        );
        let state = RsnStaState::new(akm, local, authenticator, supplicant_nonce)
            .map_err(RsnStaSupplicantError::State)?;
        let authenticator_rsn_ie =
            OwnedRsnIe::<{ crate::frames::RSN_IE_CAPACITY }>::try_copy(authenticator_rsn_ie)
                .map_err(RsnStaSupplicantError::Frame)?;
        let authenticator_security_ies =
            OwnedAssociationSecurityIes::try_copy(&authenticator_rsn_ie, authenticator_rsnxe)
                .map_err(RsnStaSupplicantError::Frame)?;
        Ok(Self {
            state,
            ptk: None,
            completed_message3: None,
            association_security_ies,
            authenticator_security_ies,
            management_protection,
        })
    }

    /// Whether the association protects its management frames.
    pub const fn management_protection(&self) -> bool {
        self.management_protection
    }

    pub const fn phase(&self) -> RsnStaPhase {
        self.state.phase()
    }

    pub fn into_connected(self) -> Result<RsnConnectedSupplicant, RsnStaSupplicantError> {
        let replay_counter = self
            .state
            .completed_replay_counter()
            .ok_or(RsnStaSupplicantError::UnexpectedAction)?;
        let ptk = self.ptk.ok_or(RsnStaSupplicantError::MissingPtk)?;
        let completed_message3 = self
            .completed_message3
            .ok_or(RsnStaSupplicantError::UnexpectedAction)?;
        let (key_confirmation, key_encryption) = ptk.into_connected_keys();
        Ok(RsnConnectedSupplicant {
            authenticator: *self.state.peer(),
            key_confirmation,
            key_encryption,
            authenticator_security_ies: self.authenticator_security_ies,
            completed_message3,
            replay_counter,
            completed_group_message1: None,
            management_protection: self.management_protection,
        })
    }

    /// Consume one validated peer EAPOL-Key frame and resolve every
    /// hardware-independent action. Message 3 with encrypted key data yields
    /// [`RsnStaSupplicantAction::UnwrapKeyData`]: the AES unwrap stays with
    /// the caller, so a hardware backend can suspend on an interrupt without
    /// changing this state owner.
    pub fn on_frame<const N: usize>(
        &mut self,
        frame: crate::OwnedEapolFrame<N>,
        pmk: &Pmk,
    ) -> Result<RsnStaSupplicantAction<N>, RsnStaSupplicantError> {
        // A completed installation may still be waiting for M4 delivery.
        // Authenticate a repeated M3 before changing its replay frontier;
        // exact-counter repeats must also match the retained authenticated M3.
        if self.state.phase() == RsnStaPhase::Completed
            && frame.key_frame().message() == crate::EapolKeyMessage::PairwiseMessage3
        {
            let key = frame.key_frame();
            if !key.verify_mic(self.ptk()?) {
                return Err(RsnStaSupplicantError::InvalidMessage3Mic);
            }
            if let Some(completed) = &self.completed_message3
                && key.replay_counter() == completed.replay_counter
                && key.mic() != &completed.mic
            {
                return Err(RsnStaSupplicantError::State(
                    RsnStateError::UnexpectedMessage,
                ));
            }
        }
        let action = self
            .state
            .on_frame(frame)
            .map_err(RsnStaSupplicantError::State)?;
        match action {
            RsnStaAction::None => Ok(RsnStaSupplicantAction::Ready(RsnStaReady::None)),
            RsnStaAction::DerivePtk { ticket, context } => {
                self.completed_message3 = None;
                self.ptk = Some(pmk.derive_ptk(
                    self.state.akm(),
                    PtkContext {
                        authenticator_address: context.authenticator_address,
                        supplicant_address: context.supplicant_address,
                        authenticator_nonce: context.authenticator_nonce,
                        supplicant_nonce: context.supplicant_nonce,
                    },
                ));
                let action = self
                    .state
                    .complete_ptk::<N>(ticket, true)
                    .map_err(RsnStaSupplicantError::State)?;
                let RsnStaAction::Transmit(transmit) = action else {
                    return Err(RsnStaSupplicantError::UnexpectedAction);
                };
                self.build_transmit(transmit)
                    .map(RsnStaSupplicantAction::Ready)
            }
            RsnStaAction::Transmit(transmit) => self
                .build_transmit(transmit)
                .map(RsnStaSupplicantAction::Ready),
            RsnStaAction::VerifyMessage3Mic { ticket, frame } => {
                if !frame.key_frame().verify_mic(self.ptk()?) {
                    self.state
                        .complete_message3_mic::<N>(ticket, frame, false)
                        .map_err(RsnStaSupplicantError::State)?;
                    return Err(RsnStaSupplicantError::InvalidMessage3Mic);
                }
                let action = self
                    .state
                    .complete_message3_mic(ticket, frame, true)
                    .map_err(RsnStaSupplicantError::State)?;
                self.resolve_message3(action)
            }
            RsnStaAction::Deauthenticate => {
                Ok(RsnStaSupplicantAction::Ready(RsnStaReady::Deauthenticate))
            }
            RsnStaAction::DecryptMessage3KeyData { .. } | RsnStaAction::InstallKeys { .. } => {
                Err(RsnStaSupplicantError::UnexpectedAction)
            }
        }
    }

    /// Complete the key-data unwrap requested by
    /// [`RsnStaSupplicantAction::UnwrapKeyData`] and prepare the key install.
    ///
    /// A failed unwrap rejects the key data in the protocol state and returns
    /// [`RsnStaProcessError::KeyUnwrap`] with the backend error.
    pub fn complete_key_data_unwrap<const N: usize, E>(
        &mut self,
        request: RsnStaKeyDataRequest<N>,
        unwrapped: Result<RsnUnwrappedKeyData, E>,
    ) -> Result<RsnStaReady<N>, RsnStaProcessError<E>> {
        let RsnStaKeyDataRequest { ticket, frame, kek } = request;
        drop(kek);
        let unwrapped = match unwrapped {
            Ok(unwrapped) => unwrapped,
            Err(error) => {
                self.state
                    .complete_key_data::<N>(ticket, frame, false)
                    .map_err(RsnStaSupplicantError::State)?;
                return Err(RsnStaProcessError::KeyUnwrap(error));
            }
        };
        let action = self
            .state
            .complete_key_data(ticket, frame, true)
            .map_err(RsnStaSupplicantError::State)?;
        let RsnStaAction::InstallKeys { ticket, frame } = action else {
            return Err(RsnStaSupplicantError::UnexpectedAction.into());
        };
        self.prepare_key_install(ticket, frame, Some(unwrapped.as_bytes()))
            .map_err(Into::into)
    }

    /// Complete the exact platform key-slot transaction represented by
    /// `request`. A successful completion produces an already authenticated
    /// Message 4; a failed completion moves the protocol state to Failed.
    pub fn complete_key_install<const N: usize>(
        &mut self,
        request: RsnStaKeyInstallRequest,
        installed: bool,
    ) -> Result<RsnStaReady<N>, RsnStaSupplicantError> {
        let (pairwise, group) = request.into_parts();
        drop(pairwise);
        self.complete_retry_install(group, installed)
    }

    /// Complete group work without obtaining pairwise installation authority.
    pub fn complete_retry_install<const N: usize>(
        &mut self,
        request: RsnStaRetryInstall,
        installed: bool,
    ) -> Result<RsnStaReady<N>, RsnStaSupplicantError> {
        let action = self
            .state
            .complete_key_install::<N>(request.ticket, installed)
            .map_err(RsnStaSupplicantError::State)?;
        match action {
            RsnStaAction::Transmit(transmit) => self.build_transmit(transmit),
            RsnStaAction::Deauthenticate => Ok(RsnStaReady::Deauthenticate),
            _ => Err(RsnStaSupplicantError::UnexpectedAction),
        }
    }

    fn resolve_message3<const N: usize>(
        &mut self,
        action: RsnStaAction<N>,
    ) -> Result<RsnStaSupplicantAction<N>, RsnStaSupplicantError> {
        match action {
            RsnStaAction::DecryptMessage3KeyData { ticket, frame } => {
                let kek = RsnKeyEncryptionKey::copy_from(self.ptk()?.kek());
                Ok(RsnStaSupplicantAction::UnwrapKeyData(
                    RsnStaKeyDataRequest { ticket, frame, kek },
                ))
            }
            RsnStaAction::InstallKeys { ticket, frame } => self
                .prepare_key_install(ticket, frame, None)
                .map(RsnStaSupplicantAction::Ready),
            _ => Err(RsnStaSupplicantError::UnexpectedAction),
        }
    }

    fn prepare_key_install<const N: usize>(
        &mut self,
        ticket: crate::state::RsnTicket,
        frame: crate::OwnedEapolFrame<N>,
        plain_key_data: Option<&[u8]>,
    ) -> Result<RsnStaReady<N>, RsnStaSupplicantError> {
        let key = frame.key_frame();
        let encrypted_key_data = key.key_info().encrypted_key_data();
        let replay_counter = key.replay_counter();
        let key_receive_sequence = *key.key_receive_sequence();
        let plain_key_data = plain_key_data.unwrap_or_else(|| key.key_data());
        let plain_key_data_len = plain_key_data.len();
        let RsnGroupKeys { gtk, igtk } = match parse_gtk_key_data(
            plain_key_data,
            self.authenticator_security_ies.rsn_ie(),
            self.authenticator_security_ies.rsnxe(),
            self.management_protection,
        ) {
            Ok(keys) => keys,
            Err(error) => {
                self.state
                    .complete_key_install::<N>(ticket, false)
                    .map_err(RsnStaSupplicantError::State)?;
                return Err(RsnStaSupplicantError::Frame(error));
            }
        };
        let retry = self.completed_message3.is_some();
        let group = RsnStaRetryInstall {
            ticket,
            replay_counter,
            encrypted_key_data,
            plain_key_data_len,
            group: RsnKeyInstall::group(RsnInterface::Station, &gtk, key_receive_sequence),
            igtk,
        };
        self.completed_message3 = Some(RsnCompletedMessage3::capture(key));
        if retry {
            Ok(RsnStaReady::RetryInstall(group))
        } else {
            Ok(RsnStaReady::InstallKeys(RsnStaKeyInstallRequest {
                pairwise: RsnKeyInstall::pairwise(
                    RsnInterface::Station,
                    *self.state.peer(),
                    [0; 8],
                    self.ptk()?,
                ),
                group,
            }))
        }
    }

    fn build_transmit<const N: usize>(
        &self,
        transmit: RsnTransmit,
    ) -> Result<RsnStaReady<N>, RsnStaSupplicantError> {
        let frame =
            build_sta_action_frame::<N, _>(&self.state, transmit, &self.association_security_ies)
                .map_err(RsnStaSupplicantError::Frame)?
                .authenticate(self.ptk()?);
        Ok(RsnStaReady::Transmit(frame))
    }

    fn ptk(&self) -> Result<&Ptk, RsnStaSupplicantError> {
        self.ptk.as_ref().ok_or(RsnStaSupplicantError::MissingPtk)
    }
}

#[cfg(test)]
mod tests;
