//! Station four-way-handshake state and its complete event/action transitions.

use super::*;
use crate::AkmKeys;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RsnStaPhase {
    AwaitingMessage1,
    DerivingPtk,
    AwaitingMessage3,
    VerifyingMessage3,
    DecryptingKeyData,
    InstallingKeys,
    Completed,
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RsnStaAction<const N: usize = DEFAULT_EAPOL_FRAME_CAPACITY> {
    None,
    DerivePtk {
        ticket: RsnTicket,
        context: PtkContext,
    },
    VerifyMessage3Mic {
        ticket: RsnTicket,
        frame: OwnedEapolFrame<N>,
    },
    DecryptMessage3KeyData {
        ticket: RsnTicket,
        frame: OwnedEapolFrame<N>,
    },
    InstallKeys {
        ticket: RsnTicket,
        frame: OwnedEapolFrame<N>,
    },
    Transmit(RsnTransmit),
    Deauthenticate,
}

pub struct RsnStaState {
    akm: Akm,
    local: [u8; 6],
    authenticator: [u8; 6],
    supplicant_nonce: [u8; RSN_NONCE_LEN],
    authenticator_nonce: [u8; RSN_NONCE_LEN],
    phase: RsnStaPhase,
    message1_replay: u64,
    message3_replay: u64,
    active_ticket: RsnTicket,
    next_ticket: u32,
}

impl RsnStaState {
    pub const fn new(
        akm: Akm,
        local: [u8; 6],
        authenticator: [u8; 6],
        supplicant_nonce: [u8; RSN_NONCE_LEN],
    ) -> Result<Self, RsnStateError> {
        if nonce_is_zero(&supplicant_nonce) {
            return Err(RsnStateError::ZeroNonce);
        }
        Ok(Self {
            akm,
            local,
            authenticator,
            supplicant_nonce,
            authenticator_nonce: [0; RSN_NONCE_LEN],
            phase: RsnStaPhase::AwaitingMessage1,
            message1_replay: 0,
            message3_replay: 0,
            active_ticket: RsnTicket(0),
            next_ticket: 1,
        })
    }

    pub const fn akm(&self) -> Akm {
        self.akm
    }

    pub const fn phase(&self) -> RsnStaPhase {
        self.phase
    }

    pub const fn peer(&self) -> &[u8; 6] {
        &self.authenticator
    }

    pub const fn local_address(&self) -> &[u8; 6] {
        &self.local
    }

    pub const fn supplicant_nonce(&self) -> &[u8; RSN_NONCE_LEN] {
        &self.supplicant_nonce
    }

    pub const fn authenticator_nonce(&self) -> &[u8; RSN_NONCE_LEN] {
        &self.authenticator_nonce
    }

    /// Replay frontier authenticated by the completed four-way handshake.
    pub const fn completed_replay_counter(&self) -> Option<u64> {
        match self.phase {
            RsnStaPhase::Completed => Some(self.message3_replay),
            _ => None,
        }
    }

    pub fn on_frame<const N: usize>(
        &mut self,
        frame: OwnedEapolFrame<N>,
    ) -> Result<RsnStaAction<N>, RsnStateError> {
        self.validate_frame(&frame)?;
        let key = frame.key_frame();
        if key.key_info().descriptor_version() != self.akm.key_descriptor_version() {
            return Err(RsnStateError::UnsupportedDescriptorVersion);
        }

        match key.message() {
            EapolKeyMessage::PairwiseMessage1 => self.on_message1(frame),
            EapolKeyMessage::PairwiseMessage3 => self.on_message3(frame),
            EapolKeyMessage::PairwiseMessage2
            | EapolKeyMessage::PairwiseMessage4
            | EapolKeyMessage::GroupMessage1
            | EapolKeyMessage::GroupMessage2
            | EapolKeyMessage::Other => Err(RsnStateError::UnsupportedMessage),
        }
    }

    fn on_message1<const N: usize>(
        &mut self,
        frame: OwnedEapolFrame<N>,
    ) -> Result<RsnStaAction<N>, RsnStateError> {
        let key = frame.key_frame();
        let replay = key.replay_counter();
        let nonce = *key.nonce();
        if nonce.iter().all(|byte| *byte == 0) {
            return Err(RsnStateError::ZeroNonce);
        }

        match self.phase {
            RsnStaPhase::AwaitingMessage1 | RsnStaPhase::Completed => {
                if self.phase == RsnStaPhase::Completed && replay <= self.message3_replay {
                    return Err(RsnStateError::StaleReplayCounter);
                }
                self.start_ptk(replay, nonce)
            }
            RsnStaPhase::DerivingPtk => {
                if replay == self.message1_replay && nonce == self.authenticator_nonce {
                    Ok(RsnStaAction::None)
                } else if replay < self.message1_replay {
                    Err(RsnStateError::StaleReplayCounter)
                } else {
                    self.start_ptk(replay, nonce)
                }
            }
            RsnStaPhase::AwaitingMessage3 => {
                if replay == self.message1_replay && nonce == self.authenticator_nonce {
                    Ok(RsnStaAction::Transmit(RsnTransmit {
                        message: RsnTxMessage::PairwiseMessage2,
                        replay_counter: replay,
                        retransmission: true,
                    }))
                } else if replay < self.message1_replay {
                    Err(RsnStateError::StaleReplayCounter)
                } else {
                    self.start_ptk(replay, nonce)
                }
            }
            RsnStaPhase::VerifyingMessage3
            | RsnStaPhase::DecryptingKeyData
            | RsnStaPhase::InstallingKeys => {
                if replay <= self.message1_replay {
                    Ok(RsnStaAction::None)
                } else {
                    Err(RsnStateError::UnexpectedMessage)
                }
            }
            RsnStaPhase::Failed => Err(RsnStateError::WrongPhase),
        }
    }

    fn start_ptk<const N: usize>(
        &mut self,
        replay: u64,
        nonce: [u8; RSN_NONCE_LEN],
    ) -> Result<RsnStaAction<N>, RsnStateError> {
        self.message1_replay = replay;
        self.message3_replay = 0;
        self.authenticator_nonce = nonce;
        self.phase = RsnStaPhase::DerivingPtk;
        let ticket = self.issue_ticket();
        Ok(RsnStaAction::DerivePtk {
            ticket,
            context: self.ptk_context(),
        })
    }

    fn on_message3<const N: usize>(
        &mut self,
        frame: OwnedEapolFrame<N>,
    ) -> Result<RsnStaAction<N>, RsnStateError> {
        self.validate_message3_fields(&frame)?;
        let replay = frame.key_frame().replay_counter();
        match self.phase {
            RsnStaPhase::AwaitingMessage3 => {
                if replay <= self.message1_replay {
                    return Err(RsnStateError::StaleReplayCounter);
                }
                self.message3_replay = replay;
                self.phase = RsnStaPhase::VerifyingMessage3;
                let ticket = self.issue_ticket();
                Ok(RsnStaAction::VerifyMessage3Mic { ticket, frame })
            }
            RsnStaPhase::VerifyingMessage3
            | RsnStaPhase::DecryptingKeyData
            | RsnStaPhase::InstallingKeys => {
                if replay == self.message3_replay {
                    Ok(RsnStaAction::None)
                } else if replay < self.message3_replay {
                    Err(RsnStateError::StaleReplayCounter)
                } else {
                    Err(RsnStateError::UnexpectedMessage)
                }
            }
            RsnStaPhase::Completed => {
                if replay == self.message3_replay {
                    // KRACK-safe retransmission: acknowledge again but never
                    // reinstall PTK/GTK or reset packet numbers.
                    Ok(RsnStaAction::Transmit(RsnTransmit {
                        message: RsnTxMessage::PairwiseMessage4,
                        replay_counter: replay,
                        retransmission: true,
                    }))
                } else if replay < self.message3_replay {
                    Err(RsnStateError::StaleReplayCounter)
                } else {
                    // An authenticated retry can carry a larger counter and
                    // fresh group material. The caller retains the installed
                    // PTK and decides whether the GTK material changed.
                    self.message3_replay = replay;
                    self.phase = RsnStaPhase::VerifyingMessage3;
                    let ticket = self.issue_ticket();
                    Ok(RsnStaAction::VerifyMessage3Mic { ticket, frame })
                }
            }
            RsnStaPhase::AwaitingMessage1 | RsnStaPhase::DerivingPtk | RsnStaPhase::Failed => {
                Err(RsnStateError::UnexpectedMessage)
            }
        }
    }

    pub fn complete_ptk<const N: usize>(
        &mut self,
        ticket: RsnTicket,
        valid: bool,
    ) -> Result<RsnStaAction<N>, RsnStateError> {
        self.check_completion(ticket, RsnStaPhase::DerivingPtk)?;
        if !valid {
            self.phase = RsnStaPhase::Failed;
            return Ok(RsnStaAction::Deauthenticate);
        }
        self.phase = RsnStaPhase::AwaitingMessage3;
        Ok(RsnStaAction::Transmit(RsnTransmit {
            message: RsnTxMessage::PairwiseMessage2,
            replay_counter: self.message1_replay,
            retransmission: false,
        }))
    }

    pub fn complete_message3_mic<const N: usize>(
        &mut self,
        ticket: RsnTicket,
        frame: OwnedEapolFrame<N>,
        valid: bool,
    ) -> Result<RsnStaAction<N>, RsnStateError> {
        self.check_completion(ticket, RsnStaPhase::VerifyingMessage3)?;
        self.validate_retained_message3(&frame)?;
        if !valid {
            // The candidate replay counter is not authenticated until the
            // MIC succeeds. Roll back the speculative M3 edge so a forged
            // parse-valid frame cannot kill the join or reserve its replay
            // value ahead of the real authenticator frame.
            self.message3_replay = 0;
            self.phase = RsnStaPhase::AwaitingMessage3;
            return Ok(RsnStaAction::None);
        }

        let key = frame.key_frame();
        if key.key_info().encrypted_key_data() {
            if key.key_data().is_empty() {
                return Err(RsnStateError::MissingEncryptedKeyData);
            }
            self.phase = RsnStaPhase::DecryptingKeyData;
            let ticket = self.issue_ticket();
            Ok(RsnStaAction::DecryptMessage3KeyData { ticket, frame })
        } else {
            self.phase = RsnStaPhase::InstallingKeys;
            let ticket = self.issue_ticket();
            Ok(RsnStaAction::InstallKeys { ticket, frame })
        }
    }

    pub fn complete_key_data<const N: usize>(
        &mut self,
        ticket: RsnTicket,
        frame: OwnedEapolFrame<N>,
        valid: bool,
    ) -> Result<RsnStaAction<N>, RsnStateError> {
        self.check_completion(ticket, RsnStaPhase::DecryptingKeyData)?;
        self.validate_retained_message3(&frame)?;
        if !valid {
            self.phase = RsnStaPhase::Failed;
            return Ok(RsnStaAction::Deauthenticate);
        }
        self.phase = RsnStaPhase::InstallingKeys;
        let ticket = self.issue_ticket();
        Ok(RsnStaAction::InstallKeys { ticket, frame })
    }

    pub fn complete_key_install<const N: usize>(
        &mut self,
        ticket: RsnTicket,
        installed: bool,
    ) -> Result<RsnStaAction<N>, RsnStateError> {
        self.check_completion(ticket, RsnStaPhase::InstallingKeys)?;
        if !installed {
            self.phase = RsnStaPhase::Failed;
            return Ok(RsnStaAction::Deauthenticate);
        }
        self.phase = RsnStaPhase::Completed;
        Ok(RsnStaAction::Transmit(RsnTransmit {
            message: RsnTxMessage::PairwiseMessage4,
            replay_counter: self.message3_replay,
            retransmission: false,
        }))
    }

    const fn ptk_context(&self) -> PtkContext {
        PtkContext {
            authenticator_address: self.authenticator,
            supplicant_address: self.local,
            authenticator_nonce: self.authenticator_nonce,
            supplicant_nonce: self.supplicant_nonce,
        }
    }

    fn validate_frame<const N: usize>(
        &self,
        frame: &OwnedEapolFrame<N>,
    ) -> Result<(), RsnStateError> {
        if frame.interface() != RsnInterface::Station {
            return Err(RsnStateError::WrongInterface);
        }
        if frame.peer() != &self.authenticator {
            return Err(RsnStateError::WrongPeer);
        }
        Ok(())
    }

    fn validate_retained_message3<const N: usize>(
        &self,
        frame: &OwnedEapolFrame<N>,
    ) -> Result<(), RsnStateError> {
        self.validate_frame(frame)?;
        self.validate_message3_fields(frame)?;
        let key = frame.key_frame();
        if key.message() != EapolKeyMessage::PairwiseMessage3
            || key.replay_counter() != self.message3_replay
        {
            return Err(RsnStateError::RetainedFrameMismatch);
        }
        Ok(())
    }

    fn validate_message3_fields<const N: usize>(
        &self,
        frame: &OwnedEapolFrame<N>,
    ) -> Result<(), RsnStateError> {
        let key = frame.key_frame();
        if key.key_info().raw() != pairwise_message3_key_info(self.akm) {
            return Err(RsnStateError::InvalidMessage3KeyInfo);
        }
        if key.key_length() != RSN_CCMP_TEMPORAL_KEY_LEN {
            return Err(RsnStateError::InvalidKeyLength);
        }
        if key.nonce() != &self.authenticator_nonce {
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

    fn check_completion(&self, ticket: RsnTicket, phase: RsnStaPhase) -> Result<(), RsnStateError> {
        if self.phase != phase {
            return Err(RsnStateError::WrongPhase);
        }
        if self.active_ticket != ticket {
            return Err(RsnStateError::StaleCompletion);
        }
        Ok(())
    }

    fn issue_ticket(&mut self) -> RsnTicket {
        let ticket = RsnTicket(self.next_ticket);
        self.next_ticket = self.next_ticket.wrapping_add(1);
        if self.next_ticket == 0 {
            self.next_ticket = 1;
        }
        self.active_ticket = ticket;
        ticket
    }
}
