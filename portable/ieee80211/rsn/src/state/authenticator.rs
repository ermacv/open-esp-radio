//! Authenticator four-way-handshake state and its bounded peer table.

use super::*;
use crate::AkmKeys;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RsnApPhase {
    AwaitingMessage2,
    DerivingPtk,
    VerifyingMessage2,
    PreparingMessage3,
    AwaitingMessage4,
    VerifyingMessage4,
    Authorized,
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RsnApAction<const N: usize = DEFAULT_EAPOL_FRAME_CAPACITY> {
    None,
    DerivePtk {
        ticket: RsnTicket,
        context: PtkContext,
        message2: OwnedEapolFrame<N>,
    },
    VerifyMessage2Mic {
        ticket: RsnTicket,
        message2: OwnedEapolFrame<N>,
    },
    PrepareMessage3 {
        ticket: RsnTicket,
    },
    VerifyMessage4Mic {
        ticket: RsnTicket,
        message4: OwnedEapolFrame<N>,
    },
    Transmit(RsnTransmit),
    AuthorizePeer,
    DeauthenticatePeer,
}

pub struct RsnApState {
    akm: Akm,
    authenticator: [u8; 6],
    supplicant: [u8; 6],
    authenticator_nonce: [u8; RSN_NONCE_LEN],
    supplicant_nonce: [u8; RSN_NONCE_LEN],
    phase: RsnApPhase,
    message1_replay: u64,
    message3_replay: u64,
    active_ticket: RsnTicket,
    next_ticket: u32,
}

impl RsnApState {
    pub const fn new(
        akm: Akm,
        authenticator: [u8; 6],
        supplicant: [u8; 6],
        authenticator_nonce: [u8; RSN_NONCE_LEN],
        initial_replay_counter: u64,
    ) -> Result<Self, RsnStateError> {
        if initial_replay_counter == u64::MAX {
            return Err(RsnStateError::ReplayCounterExhausted);
        }
        if nonce_is_zero(&authenticator_nonce) {
            return Err(RsnStateError::ZeroNonce);
        }
        Ok(Self {
            akm,
            authenticator,
            supplicant,
            authenticator_nonce,
            supplicant_nonce: [0; RSN_NONCE_LEN],
            phase: RsnApPhase::AwaitingMessage2,
            message1_replay: initial_replay_counter,
            message3_replay: 0,
            active_ticket: RsnTicket(0),
            next_ticket: 1,
        })
    }

    pub const fn akm(&self) -> Akm {
        self.akm
    }

    pub const fn phase(&self) -> RsnApPhase {
        self.phase
    }

    pub const fn peer(&self) -> &[u8; 6] {
        &self.supplicant
    }

    pub const fn local_address(&self) -> &[u8; 6] {
        &self.authenticator
    }

    pub const fn supplicant_nonce(&self) -> &[u8; RSN_NONCE_LEN] {
        &self.supplicant_nonce
    }

    pub const fn authenticator_nonce(&self) -> &[u8; RSN_NONCE_LEN] {
        &self.authenticator_nonce
    }

    pub const fn message1(&self, retransmission: bool) -> Result<RsnApAction, RsnStateError> {
        if !matches!(self.phase, RsnApPhase::AwaitingMessage2) {
            return Err(RsnStateError::WrongPhase);
        }
        Ok(RsnApAction::Transmit(RsnTransmit {
            message: RsnTxMessage::PairwiseMessage1,
            replay_counter: self.message1_replay,
            retransmission,
        }))
    }

    /// Return the authenticator frame that owns the current response window.
    ///
    /// Timing and retry budgets belong to the AP service. The WPA state only
    /// exposes the protocol-correct message and replay counter for its current
    /// phase; it never reads time or schedules work itself.
    pub const fn retry_transmit(&self) -> Result<RsnTransmit, RsnStateError> {
        match self.phase {
            RsnApPhase::AwaitingMessage2 => Ok(RsnTransmit {
                message: RsnTxMessage::PairwiseMessage1,
                replay_counter: self.message1_replay,
                retransmission: true,
            }),
            RsnApPhase::AwaitingMessage4 => Ok(RsnTransmit {
                message: RsnTxMessage::PairwiseMessage3,
                replay_counter: self.message3_replay,
                retransmission: true,
            }),
            _ => Err(RsnStateError::WrongPhase),
        }
    }

    pub fn on_frame<const N: usize>(
        &mut self,
        frame: OwnedEapolFrame<N>,
    ) -> Result<RsnApAction<N>, RsnStateError> {
        self.validate_frame(&frame)?;
        let key = frame.key_frame();
        if key.key_info().descriptor_version() != self.akm.key_descriptor_version() {
            return Err(RsnStateError::UnsupportedDescriptorVersion);
        }
        match key.message() {
            EapolKeyMessage::PairwiseMessage2 => self.on_message2(frame),
            EapolKeyMessage::PairwiseMessage4 => self.on_message4(frame),
            _ => Err(RsnStateError::UnsupportedMessage),
        }
    }

    fn on_message2<const N: usize>(
        &mut self,
        frame: OwnedEapolFrame<N>,
    ) -> Result<RsnApAction<N>, RsnStateError> {
        let key = frame.key_frame();
        let replay = key.replay_counter();
        let nonce = *key.nonce();
        if replay != self.message1_replay {
            return Err(RsnStateError::ReplayCounterMismatch);
        }
        if nonce.iter().all(|byte| *byte == 0) {
            return Err(RsnStateError::ZeroNonce);
        }

        match self.phase {
            RsnApPhase::AwaitingMessage2 => {
                self.supplicant_nonce = nonce;
                self.phase = RsnApPhase::DerivingPtk;
                let ticket = self.issue_ticket();
                Ok(RsnApAction::DerivePtk {
                    ticket,
                    context: self.ptk_context(),
                    message2: frame,
                })
            }
            RsnApPhase::DerivingPtk
            | RsnApPhase::VerifyingMessage2
            | RsnApPhase::PreparingMessage3 => Ok(RsnApAction::None),
            RsnApPhase::AwaitingMessage4 | RsnApPhase::VerifyingMessage4 => {
                if nonce == self.supplicant_nonce {
                    // Public replay/SNonce fields do not authenticate a
                    // duplicate M2. The bounded AP M3 retry owner will
                    // retransmit on its timer; never emit a MIC-bearing M3 in
                    // direct response to an unverified peer frame.
                    Ok(RsnApAction::None)
                } else {
                    Err(RsnStateError::UnexpectedMessage)
                }
            }
            RsnApPhase::Authorized => Ok(RsnApAction::None),
            RsnApPhase::Failed => Err(RsnStateError::WrongPhase),
        }
    }

    fn on_message4<const N: usize>(
        &mut self,
        frame: OwnedEapolFrame<N>,
    ) -> Result<RsnApAction<N>, RsnStateError> {
        let replay = frame.key_frame().replay_counter();
        if replay != self.message3_replay {
            return Err(RsnStateError::ReplayCounterMismatch);
        }
        match self.phase {
            RsnApPhase::AwaitingMessage4 => {
                self.phase = RsnApPhase::VerifyingMessage4;
                let ticket = self.issue_ticket();
                Ok(RsnApAction::VerifyMessage4Mic {
                    ticket,
                    message4: frame,
                })
            }
            RsnApPhase::VerifyingMessage4 | RsnApPhase::Authorized => Ok(RsnApAction::None),
            _ => Err(RsnStateError::UnexpectedMessage),
        }
    }

    pub fn complete_ptk<const N: usize>(
        &mut self,
        ticket: RsnTicket,
        message2: OwnedEapolFrame<N>,
        valid: bool,
    ) -> Result<RsnApAction<N>, RsnStateError> {
        self.check_completion(ticket, RsnApPhase::DerivingPtk)?;
        self.validate_retained_message2(&message2)?;
        if !valid {
            self.phase = RsnApPhase::Failed;
            return Ok(RsnApAction::DeauthenticatePeer);
        }
        self.phase = RsnApPhase::VerifyingMessage2;
        let ticket = self.issue_ticket();
        Ok(RsnApAction::VerifyMessage2Mic { ticket, message2 })
    }

    pub fn complete_message2_mic<const N: usize>(
        &mut self,
        ticket: RsnTicket,
        message2: OwnedEapolFrame<N>,
        valid: bool,
    ) -> Result<RsnApAction<N>, RsnStateError> {
        self.check_completion(ticket, RsnApPhase::VerifyingMessage2)?;
        self.validate_retained_message2(&message2)?;
        if !valid {
            // SNonce and M2 replay are peer input until the MIC authenticates
            // them. A spoofed M2 must not poison this peer's M1 transaction.
            self.supplicant_nonce = [0; RSN_NONCE_LEN];
            self.phase = RsnApPhase::AwaitingMessage2;
            return Ok(RsnApAction::None);
        }
        self.phase = RsnApPhase::PreparingMessage3;
        let ticket = self.issue_ticket();
        Ok(RsnApAction::PrepareMessage3 { ticket })
    }

    pub fn complete_message3_preparation<const N: usize>(
        &mut self,
        ticket: RsnTicket,
        prepared: bool,
    ) -> Result<RsnApAction<N>, RsnStateError> {
        self.check_completion(ticket, RsnApPhase::PreparingMessage3)?;
        if !prepared {
            self.phase = RsnApPhase::Failed;
            return Ok(RsnApAction::DeauthenticatePeer);
        }
        self.message3_replay = self
            .message1_replay
            .checked_add(1)
            .ok_or(RsnStateError::ReplayCounterExhausted)?;
        self.phase = RsnApPhase::AwaitingMessage4;
        Ok(RsnApAction::Transmit(RsnTransmit {
            message: RsnTxMessage::PairwiseMessage3,
            replay_counter: self.message3_replay,
            retransmission: false,
        }))
    }

    pub fn complete_message4_mic<const N: usize>(
        &mut self,
        ticket: RsnTicket,
        message4: OwnedEapolFrame<N>,
        valid: bool,
    ) -> Result<RsnApAction<N>, RsnStateError> {
        self.check_completion(ticket, RsnApPhase::VerifyingMessage4)?;
        self.validate_retained_message4(&message4)?;
        if !valid {
            // Retain the installed-candidate PTK and M3 response window. A
            // forged M4 can then be ignored while a valid retry still
            // authorizes exactly this handshake.
            self.phase = RsnApPhase::AwaitingMessage4;
            return Ok(RsnApAction::None);
        }
        self.phase = RsnApPhase::Authorized;
        Ok(RsnApAction::AuthorizePeer)
    }

    const fn ptk_context(&self) -> PtkContext {
        PtkContext {
            authenticator_address: self.authenticator,
            supplicant_address: self.supplicant,
            authenticator_nonce: self.authenticator_nonce,
            supplicant_nonce: self.supplicant_nonce,
        }
    }

    fn validate_frame<const N: usize>(
        &self,
        frame: &OwnedEapolFrame<N>,
    ) -> Result<(), RsnStateError> {
        if frame.interface() != RsnInterface::AccessPoint {
            return Err(RsnStateError::WrongInterface);
        }
        if frame.peer() != &self.supplicant {
            return Err(RsnStateError::WrongPeer);
        }
        Ok(())
    }

    fn validate_retained_message2<const N: usize>(
        &self,
        frame: &OwnedEapolFrame<N>,
    ) -> Result<(), RsnStateError> {
        self.validate_frame(frame)?;
        let key = frame.key_frame();
        if key.message() != EapolKeyMessage::PairwiseMessage2
            || key.replay_counter() != self.message1_replay
            || key.nonce() != &self.supplicant_nonce
        {
            return Err(RsnStateError::RetainedFrameMismatch);
        }
        Ok(())
    }

    fn validate_retained_message4<const N: usize>(
        &self,
        frame: &OwnedEapolFrame<N>,
    ) -> Result<(), RsnStateError> {
        self.validate_frame(frame)?;
        let key = frame.key_frame();
        if key.message() != EapolKeyMessage::PairwiseMessage4
            || key.replay_counter() != self.message3_replay
        {
            return Err(RsnStateError::RetainedFrameMismatch);
        }
        Ok(())
    }

    fn check_completion(&self, ticket: RsnTicket, phase: RsnApPhase) -> Result<(), RsnStateError> {
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RsnApPeerError {
    DuplicatePeer,
    Full,
}

/// Fixed AP peer table. Construction and lookup are bounded by `P`; neither
/// path allocates or waits.
pub struct RsnApPeers<const P: usize> {
    peers: [Option<RsnApState>; P],
}

impl<const P: usize> RsnApPeers<P> {
    pub fn new() -> Self {
        assert!(P > 0);
        Self {
            peers: core::array::from_fn(|_| None),
        }
    }

    pub fn insert(&mut self, peer: RsnApState) -> Result<(), RsnApPeerError> {
        if self
            .peers
            .iter()
            .flatten()
            .any(|existing| existing.peer() == peer.peer())
        {
            return Err(RsnApPeerError::DuplicatePeer);
        }
        let slot = self
            .peers
            .iter_mut()
            .find(|slot| slot.is_none())
            .ok_or(RsnApPeerError::Full)?;
        *slot = Some(peer);
        Ok(())
    }

    pub fn get_mut(&mut self, peer: &[u8; 6]) -> Option<&mut RsnApState> {
        self.peers
            .iter_mut()
            .flatten()
            .find(|state| state.peer() == peer)
    }

    pub fn remove(&mut self, peer: &[u8; 6]) -> Option<RsnApState> {
        let slot = self
            .peers
            .iter_mut()
            .find(|slot| slot.as_ref().is_some_and(|state| state.peer() == peer))?;
        slot.take()
    }

    pub fn len(&self) -> usize {
        self.peers.iter().filter(|peer| peer.is_some()).count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl<const P: usize> Default for RsnApPeers<P> {
    fn default() -> Self {
        Self::new()
    }
}
