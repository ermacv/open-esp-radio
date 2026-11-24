//! Allocation-free RSN authenticator: one station's four-way handshake as
//! one machine, the counterpart of [`crate::supplicant`].
//!
//! [`RsnApHandshake`] is a sum by phase whose steps run the handshake's
//! cryptography in place, over the engine each step is lent: the PTK's
//! derivation from the association's PMK, Message 2's and Message 4's MIC,
//! the binding of the station's exact Association security IEs, and the wrap
//! and MIC of Message 3. A failure ends the handshake: the step returns the
//! end and its holder drops the machine. The access point keeps its peer
//! table, the response windows' timing, the group keys and the Message 3 key
//! data it advertises, and transmission. The SAE side of an access point,
//! `authenticator/sae.rs`, is outside the build until the access point's role
//! serves SAE.
//!
//! Received EAPOL reaches this module; nothing here panics on it.

use crate::{
    Akm, AssociationSecurityBinding, EapolKeyMessage, OwnedEapolFrame, Pmk, Ptk, PtkContext,
    RsnInterface,
    frames::{RsnFrameError, RsnPlainKeyData, RsnTxFrame},
    state::{RSN_NONCE_LEN, RsnStateError, RsnTransmit, RsnTxMessage},
};
use crate::{AkmKeys, MicFailure};
use oer_portable_crypto::{
    Aes128Key, KeyData, KeyWrapRequest,
    block::{Aes128Engine, BlockError, wrap_now},
    rfc3394::KeyWrapError,
};
use oer_protocols_ieee80211_core_security_cipher::aes::RSN_KEY_DATA_CAPACITY;
use oer_protocols_ieee80211_wire::ccmp::CcmpPacketNumber;

/// How many built transmissions of one handshake message stay awaited.
///
/// Every Message 1 and Message 3, a retransmission included, takes a new
/// counter that is never handed out again. Only a transmission whose frame
/// was built enters the history of awaited answers; a counter whose frame
/// failed to build stays spent, is not awaited and evicts nothing. An answer
/// to any of the latest `RSN_AP_REPLAY_WINDOW` built transmissions completes
/// the step, so a retransmission never invalidates an earlier frame the
/// station may already have answered. The history closes when the step
/// completes. hostapd keeps a bounded history of expected counters the same
/// way (`src/ap/wpa_auth.c`, `wpa_replay_counter_valid`).
pub const RSN_AP_REPLAY_WINDOW: usize = 4;

/// The replay counters of one step's built transmissions; the oldest is
/// replaced once all slots hold one.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct AwaitedAnswers {
    counters: [u64; RSN_AP_REPLAY_WINDOW],
    filled: u8,
    next: u8,
}

impl AwaitedAnswers {
    /// The history holding the one built transmission `first`.
    const fn first(first: u64) -> Self {
        const {
            if RSN_AP_REPLAY_WINDOW == 0 || RSN_AP_REPLAY_WINDOW > u8::MAX as usize {
                panic!("the awaited window holds between one and 255 transmissions");
            }
        }
        let mut counters = [0; RSN_AP_REPLAY_WINDOW];
        if let Some(slot) = counters.first_mut() {
            *slot = first;
        }
        Self {
            counters,
            filled: 1,
            next: if RSN_AP_REPLAY_WINDOW == 1 { 0 } else { 1 },
        }
    }

    fn contains(&self, counter: u64) -> bool {
        self.counters
            .get(..usize::from(self.filled))
            .is_some_and(|counters| counters.contains(&counter))
    }

    fn record(&mut self, counter: u64) {
        if let Some(slot) = self.counters.get_mut(usize::from(self.next)) {
            *slot = counter;
        }
        if usize::from(self.filled) < RSN_AP_REPLAY_WINDOW {
            self.filled = self.filled.saturating_add(1);
        }
        self.next = self
            .next
            .checked_add(1)
            .filter(|next| usize::from(*next) < RSN_AP_REPLAY_WINDOW)
            .unwrap_or(0);
    }
}

/// Why a step of the authenticator refused its input; the handshake is as
/// it was.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RsnApError {
    /// The received EAPOL-Key frame was refused on admission: unsupported,
    /// stale or otherwise unauthenticated peer input, local to that peer.
    Rejected(RsnStateError),
    /// No replay counter is left for the next transmission.
    ReplayCounterExhausted,
    /// The frame could not be built; its counter stays spent and unawaited.
    Frame(RsnFrameError),
    /// The retransmitted Message 3's key data could not be wrapped.
    KeyWrap(KeyWrapError),
    /// The AES engine refused a MIC's key or block.
    Engine(BlockError),
}

impl From<RsnFrameError> for RsnApError {
    fn from(error: RsnFrameError) -> Self {
        Self::Frame(error)
    }
}

impl From<BlockError> for RsnApError {
    fn from(error: BlockError) -> Self {
        Self::Engine(error)
    }
}

impl From<oer_protocols_ieee80211_wire::eapol::EapolParseError> for RsnApError {
    fn from(error: oer_protocols_ieee80211_wire::eapol::EapolParseError) -> Self {
        Self::Rejected(RsnStateError::from(error))
    }
}

/// Why a handshake ended. Its holder drops the machine and deauthenticates
/// the peer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RsnApEnd {
    /// The authenticated Message 2 names other security elements than the
    /// association.
    SecurityIesMismatch,
    /// Message 3's key data could not be wrapped.
    KeyWrap(KeyWrapError),
}

/// What Message 3 carries beside its PTK: the key data the access point
/// advertises and, as its Key RSC, the last packet number the installed GTK
/// in it has handed out.
pub struct Message3Material<'a, const K: usize> {
    pub key_data: &'a RsnPlainKeyData<K>,
    pub group_packet_number: CcmpPacketNumber,
}

/// What a received EAPOL-Key frame did.
pub enum RsnApEapol<const N: usize> {
    /// Nothing: a frame the handshake ignores (a forged one, a repeat).
    None,
    /// A verified Message 2: Message 3, built, to transmit.
    Message3(RsnTxFrame<N>, RsnTransmit),
    /// A verified Message 4: the peer is authorized.
    Authorized,
    /// The handshake ended.
    End(RsnApEnd),
}

/// What a retransmission built.
pub enum RsnApRetransmit<const N: usize> {
    Message1(RsnTxFrame<N>, RsnTransmit),
    Message3(RsnTxFrame<N>, RsnTransmit),
    /// The peer is authorized: no response window remains.
    Authorized,
}

/// The proof that a handshake's Message 4 verified; it carries no key.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RsnApAuthorized(());

/// The message a handshake still awaits before its peer may be authorized.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RsnApAwaiting {
    Message2,
    Message4,
}

/// One station's four-way handshake.
pub struct RsnApHandshake {
    akm: Akm,
    authenticator: [u8; 6],
    supplicant: [u8; 6],
    authenticator_nonce: [u8; RSN_NONCE_LEN],
    /// The last replay counter handed out; none is handed out twice.
    last_counter: u64,
    phase: Phase,
}

/// What exists in each phase.
enum Phase {
    /// Message 1 is built; Message 2 is awaited.
    AwaitingMessage2 {
        binding: AssociationSecurityBinding,
        awaited: AwaitedAnswers,
        latest: RsnTransmit,
    },
    /// Message 2 verified: its PTK, through Message 4 and the key's
    /// installation.
    Keyed { ptk: Ptk, stage: KeyedStage },
}

/// What exists beside the PTK.
enum KeyedStage {
    /// Message 3 is built; Message 4 is awaited.
    AwaitingMessage4 {
        supplicant_nonce: [u8; RSN_NONCE_LEN],
        awaited: AwaitedAnswers,
        latest: RsnTransmit,
    },
    /// Message 4 verified: the PTK is lent to its installation.
    Authorized,
}

/// What a handshake starts from, checked when the association is
/// accepted: the AKM, both addresses, a nonzero ANonce, a first replay
/// counter that leaves one for Message 3, and the binding of the
/// association's security IEs.
pub struct RsnApStart {
    akm: Akm,
    authenticator: [u8; 6],
    supplicant: [u8; 6],
    authenticator_nonce: [u8; RSN_NONCE_LEN],
    initial_replay_counter: u64,
    binding: AssociationSecurityBinding,
}

impl RsnApStart {
    /// A refusal, `ZeroNonce` or `ReplayCounterExhausted`, keeps nothing.
    pub fn new(
        akm: Akm,
        authenticator: [u8; 6],
        supplicant: [u8; 6],
        authenticator_nonce: [u8; RSN_NONCE_LEN],
        initial_replay_counter: u64,
        binding: AssociationSecurityBinding,
    ) -> Result<Self, RsnStateError> {
        if authenticator_nonce.iter().all(|byte| *byte == 0) {
            return Err(RsnStateError::ZeroNonce);
        }
        // Message 3 takes the counter after Message 1's.
        if initial_replay_counter == u64::MAX {
            return Err(RsnStateError::ReplayCounterExhausted);
        }
        Ok(Self {
            akm,
            authenticator,
            supplicant,
            authenticator_nonce,
            initial_replay_counter,
            binding,
        })
    }

    pub const fn akm(&self) -> Akm {
        self.akm
    }
}

impl RsnApHandshake {
    /// Start the handshake with its Message 1, in a frame of `N` octets. It
    /// cannot refuse: `start` was checked when it was made, and Message 1's
    /// room in `N` is checked when `N` is chosen.
    pub fn start<const N: usize>(start: RsnApStart) -> (Self, RsnTxFrame<N>, RsnTransmit) {
        let RsnApStart {
            akm,
            authenticator,
            supplicant,
            authenticator_nonce,
            initial_replay_counter,
            binding,
        } = start;
        let transmit = RsnTransmit {
            message: RsnTxMessage::PairwiseMessage1,
            replay_counter: initial_replay_counter,
            retransmission: false,
        };
        let frame = RsnTxFrame::authenticator_message1(
            akm,
            supplicant,
            initial_replay_counter,
            authenticator_nonce,
        );
        (
            Self {
                akm,
                authenticator,
                supplicant,
                authenticator_nonce,
                last_counter: initial_replay_counter,
                phase: Phase::AwaitingMessage2 {
                    binding,
                    awaited: AwaitedAnswers::first(initial_replay_counter),
                    latest: transmit,
                },
            },
            frame,
            transmit,
        )
    }

    pub const fn akm(&self) -> Akm {
        self.akm
    }

    pub const fn peer(&self) -> &[u8; 6] {
        &self.supplicant
    }

    /// The latest built transmission, which owns the response window;
    /// `None` once the peer is authorized.
    pub const fn latest(&self) -> Option<RsnTransmit> {
        match &self.phase {
            Phase::AwaitingMessage2 { latest, .. }
            | Phase::Keyed {
                stage: KeyedStage::AwaitingMessage4 { latest, .. },
                ..
            } => Some(*latest),
            Phase::Keyed {
                stage: KeyedStage::Authorized,
                ..
            } => None,
        }
    }

    /// The PTK of the verified Message 2, from Message 3's build on.
    pub const fn ptk(&self) -> Option<&Ptk> {
        match &self.phase {
            Phase::Keyed { ptk, .. } => Some(ptk),
            Phase::AwaitingMessage2 { .. } => None,
        }
    }

    /// Whether the peer may be authorized: the proof that Message 4
    /// verified, or the message still awaited. It changes nothing.
    pub const fn authorization(&self) -> Result<RsnApAuthorized, RsnApAwaiting> {
        match &self.phase {
            Phase::Keyed {
                stage: KeyedStage::Authorized,
                ..
            } => Ok(RsnApAuthorized(())),
            Phase::Keyed {
                stage: KeyedStage::AwaitingMessage4 { .. },
                ..
            } => Err(RsnApAwaiting::Message4),
            Phase::AwaitingMessage2 { .. } => Err(RsnApAwaiting::Message2),
        }
    }

    /// Advance through Message 2 or Message 4 with the association's `pmk`.
    ///
    /// A Message 2 derives the PTK and verifies its MIC; only then may its
    /// Key Data, the station's security IEs, decide the outcome, and a
    /// mismatch ends the handshake. A verified Message 2 builds Message 3
    /// from `material` in the same step, its key data wrapped and its MIC
    /// computed on `engine`. A forged frame changes nothing.
    pub fn on_eapol<const N: usize, const K: usize, E: Aes128Engine>(
        &mut self,
        pmk: &Pmk,
        frame: OwnedEapolFrame<N>,
        material: Message3Material<'_, K>,
        engine: &mut E,
    ) -> Result<RsnApEapol<N>, RsnApError> {
        if frame.interface() != RsnInterface::AccessPoint {
            return Err(RsnApError::Rejected(RsnStateError::WrongInterface));
        }
        if frame.peer() != &self.supplicant {
            return Err(RsnApError::Rejected(RsnStateError::WrongPeer));
        }
        let key = frame.key_frame()?;
        if key.key_info().descriptor_version() != self.akm.key_descriptor_version().get() {
            return Err(RsnApError::Rejected(
                RsnStateError::UnsupportedDescriptorVersion,
            ));
        }
        match key.message() {
            EapolKeyMessage::PairwiseMessage2 => self.on_message2(pmk, &frame, material, engine),
            EapolKeyMessage::PairwiseMessage4 => self.on_message4(&frame, engine),
            _ => Err(RsnApError::Rejected(RsnStateError::UnsupportedMessage)),
        }
    }

    fn on_message2<const N: usize, const K: usize, E: Aes128Engine>(
        &mut self,
        pmk: &Pmk,
        frame: &OwnedEapolFrame<N>,
        material: Message3Material<'_, K>,
        engine: &mut E,
    ) -> Result<RsnApEapol<N>, RsnApError> {
        let key = frame.key_frame()?;
        let replay = key.replay_counter();
        let supplicant_nonce = *key.nonce();
        let (binding, awaited) = match &self.phase {
            Phase::AwaitingMessage2 {
                binding, awaited, ..
            } => (binding, awaited),
            Phase::Keyed {
                stage:
                    KeyedStage::AwaitingMessage4 {
                        supplicant_nonce: accepted,
                        ..
                    },
                ..
            } => {
                // Public replay and SNonce fields do not authenticate a
                // duplicate Message 2: the Message 3 retry answers on its
                // timer, never in response to an unverified peer frame.
                return if supplicant_nonce == *accepted {
                    Ok(RsnApEapol::None)
                } else {
                    Err(RsnApError::Rejected(RsnStateError::UnexpectedMessage))
                };
            }
            Phase::Keyed {
                stage: KeyedStage::Authorized,
                ..
            } => return Ok(RsnApEapol::None),
        };
        if !awaited.contains(replay) {
            return Err(RsnApError::Rejected(RsnStateError::ReplayCounterMismatch));
        }
        if supplicant_nonce.iter().all(|byte| *byte == 0) {
            return Err(RsnApError::Rejected(RsnStateError::ZeroNonce));
        }
        let ptk = pmk.derive_ptk(
            self.akm,
            PtkContext {
                authenticator_address: self.authenticator,
                supplicant_address: self.supplicant,
                authenticator_nonce: self.authenticator_nonce,
                supplicant_nonce,
            },
        );
        match key.check_mic(&ptk, engine) {
            Ok(()) => {}
            // A spoofed Message 2 is ignored; the peer may send it again.
            Err(MicFailure::Mismatch) => return Ok(RsnApEapol::None),
            Err(MicFailure::Engine(error)) => return Err(RsnApError::Engine(error)),
        }
        // The association commitment is an authenticated semantic binding:
        // attacker-controlled Key Data decides nothing until this exact
        // Message 2 has passed its PTK-derived MIC.
        if !binding.matches(pmk, key.key_data()) {
            return Ok(RsnApEapol::End(RsnApEnd::SecurityIesMismatch));
        }
        let replay_counter = self
            .last_counter
            .checked_add(1)
            .ok_or(RsnApError::ReplayCounterExhausted)?;
        let transmit = RsnTransmit {
            message: RsnTxMessage::PairwiseMessage3,
            replay_counter,
            retransmission: false,
        };
        let built = message3(
            Message3Facts {
                akm: self.akm,
                supplicant: self.supplicant,
                authenticator_nonce: self.authenticator_nonce,
                ptk: &ptk,
                transmit,
            },
            &material,
            engine,
        );
        // A frame that fails to build leaves its counter spent.
        self.last_counter = replay_counter;
        let frame = match built {
            Ok(frame) => frame,
            Err(Message3Failure::Wrap(error)) => {
                return Ok(RsnApEapol::End(RsnApEnd::KeyWrap(error)));
            }
            // The handshake awaits Message 2 again.
            Err(Message3Failure::Step(error)) => return Err(error),
        };
        self.phase = Phase::Keyed {
            ptk,
            stage: KeyedStage::AwaitingMessage4 {
                supplicant_nonce,
                awaited: AwaitedAnswers::first(replay_counter),
                latest: transmit,
            },
        };
        Ok(RsnApEapol::Message3(frame, transmit))
    }

    fn on_message4<const N: usize, E: Aes128Engine>(
        &mut self,
        frame: &OwnedEapolFrame<N>,
        engine: &mut E,
    ) -> Result<RsnApEapol<N>, RsnApError> {
        let key = frame.key_frame()?;
        let Phase::Keyed { ptk, stage } = &mut self.phase else {
            return Err(RsnApError::Rejected(RsnStateError::UnexpectedMessage));
        };
        let KeyedStage::AwaitingMessage4 { awaited, .. } = stage else {
            // The window closed with the step: a later Message 4 neither
            // authorizes nor installs a key again.
            return Ok(RsnApEapol::None);
        };
        if !awaited.contains(key.replay_counter()) {
            return Err(RsnApError::Rejected(RsnStateError::ReplayCounterMismatch));
        }
        match key.check_mic(ptk, engine) {
            Ok(()) => {}
            // A forged Message 4 is ignored; a valid retry still authorizes.
            Err(MicFailure::Mismatch) => return Ok(RsnApEapol::None),
            Err(MicFailure::Engine(error)) => return Err(RsnApError::Engine(error)),
        }
        *stage = KeyedStage::Authorized;
        Ok(RsnApEapol::Authorized)
    }

    /// The current message again, under the next replay counter: Message 1
    /// while Message 2 is awaited, Message 3 (from `material`, wrapped and
    /// signed on `engine`) while Message 4 is. Once built, an answer to it or
    /// to an earlier built frame of the window completes the step; a counter
    /// whose frame fails to build stays spent and is never awaited.
    pub fn retransmit<const N: usize, const K: usize, E: Aes128Engine>(
        &mut self,
        material: Message3Material<'_, K>,
        engine: &mut E,
    ) -> Result<RsnApRetransmit<N>, RsnApError> {
        let Self {
            akm,
            supplicant,
            authenticator_nonce,
            last_counter,
            phase,
            ..
        } = self;
        match phase {
            Phase::AwaitingMessage2 {
                awaited, latest, ..
            } => {
                // The counter after this one stays free for Message 3.
                let replay_counter = last_counter
                    .checked_add(1)
                    .filter(|counter| *counter != u64::MAX)
                    .ok_or(RsnApError::ReplayCounterExhausted)?;
                *last_counter = replay_counter;
                let transmit = RsnTransmit {
                    message: RsnTxMessage::PairwiseMessage1,
                    replay_counter,
                    retransmission: true,
                };
                let mut frame = RsnTxFrame::authenticator_message1(
                    *akm,
                    *supplicant,
                    replay_counter,
                    *authenticator_nonce,
                );
                frame.retransmission = true;
                awaited.record(replay_counter);
                *latest = transmit;
                Ok(RsnApRetransmit::Message1(frame, transmit))
            }
            Phase::Keyed {
                ptk,
                stage:
                    KeyedStage::AwaitingMessage4 {
                        awaited, latest, ..
                    },
            } => {
                let replay_counter = last_counter
                    .checked_add(1)
                    .ok_or(RsnApError::ReplayCounterExhausted)?;
                let transmit = RsnTransmit {
                    message: RsnTxMessage::PairwiseMessage3,
                    replay_counter,
                    retransmission: true,
                };
                let built = message3(
                    Message3Facts {
                        akm: *akm,
                        supplicant: *supplicant,
                        authenticator_nonce: *authenticator_nonce,
                        ptk,
                        transmit,
                    },
                    &material,
                    engine,
                );
                // A frame that fails to build leaves its counter spent.
                *last_counter = replay_counter;
                let frame = match built {
                    Ok(frame) => frame,
                    Err(Message3Failure::Wrap(error)) => return Err(RsnApError::KeyWrap(error)),
                    Err(Message3Failure::Step(error)) => return Err(error),
                };
                awaited.record(replay_counter);
                *latest = transmit;
                Ok(RsnApRetransmit::Message3(frame, transmit))
            }
            Phase::Keyed {
                stage: KeyedStage::Authorized,
                ..
            } => Ok(RsnApRetransmit::Authorized),
        }
    }
}

/// What Message 3 is built from beside its material.
struct Message3Facts<'a> {
    akm: Akm,
    supplicant: [u8; 6],
    authenticator_nonce: [u8; RSN_NONCE_LEN],
    ptk: &'a Ptk,
    transmit: RsnTransmit,
}

/// Message 3 of `facts.transmit` under `facts.ptk`: its key data wrapped
/// under the KEK and its MIC computed, both on `engine`.
fn message3<const N: usize, const K: usize, E: Aes128Engine>(
    facts: Message3Facts<'_>,
    material: &Message3Material<'_, K>,
    engine: &mut E,
) -> Result<RsnTxFrame<N>, Message3Failure> {
    let Message3Facts {
        akm,
        supplicant,
        authenticator_nonce,
        ptk,
        transmit,
    } = facts;
    let plaintext = KeyData::<RSN_KEY_DATA_CAPACITY>::try_copy(material.key_data.as_bytes())
        .map_err(|_| Message3Failure::Wrap(KeyWrapError::CapacityExceeded))?;
    let wrapped = wrap_now(
        engine,
        KeyWrapRequest::new(Aes128Key::copy_from(ptk.kek()), plaintext),
    )
    .into_wrapped()
    .map_err(Message3Failure::Wrap)?;
    let mut frame = RsnTxFrame::message3(
        akm,
        supplicant,
        transmit.replay_counter,
        authenticator_nonce,
        material.group_packet_number.receive_sequence(),
        wrapped.as_bytes(),
    )
    .map_err(|error| Message3Failure::Step(RsnApError::Frame(error)))?;
    frame.retransmission = transmit.retransmission;
    frame
        .authenticate(ptk, engine)
        .map_err(|error| Message3Failure::Step(RsnApError::Engine(error)))
}

/// Why Message 3 was not built: a wrap failure, which ends a first build,
/// or a step's error.
enum Message3Failure {
    Wrap(KeyWrapError),
    Step(RsnApError),
}

#[cfg(test)]
mod tests;
