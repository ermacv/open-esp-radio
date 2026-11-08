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

/// How many of the latest replay counters of one handshake message stay
/// awaited.
///
/// Every Message 1 and Message 3, a retransmission included, takes a new
/// counter; an answer to any of the latest `RSN_AP_REPLAY_WINDOW` of them
/// completes the step, so a retransmission never invalidates an earlier
/// frame the station may already have answered. The window closes when the
/// step completes. hostapd keeps a bounded history of expected counters the
/// same way (`src/ap/wpa_auth.c`, `wpa_replay_counter_valid`).
pub const RSN_AP_REPLAY_WINDOW: u64 = 4;

pub struct RsnApState {
    akm: Akm,
    authenticator: [u8; 6],
    supplicant: [u8; 6],
    authenticator_nonce: [u8; RSN_NONCE_LEN],
    supplicant_nonce: [u8; RSN_NONCE_LEN],
    phase: RsnApPhase,
    /// The first and the latest counter of the Message 1 transmissions.
    message1_first: u64,
    message1_replay: u64,
    /// The first and the latest counter of the Message 3 transmissions.
    message3_first: u64,
    message3_replay: u64,
    /// The counter of the Message 2 or 4 whose verification is in progress.
    answered_replay: u64,
    /// Whether the counter of the current response window belongs to a
    /// retransmission.
    retransmitted: bool,
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
            message1_first: initial_replay_counter,
            message1_replay: initial_replay_counter,
            message3_first: 0,
            message3_replay: 0,
            answered_replay: 0,
            retransmitted: false,
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

    /// The authenticator frame that owns the current response window: its
    /// message and the replay counter its response must echo.
    ///
    /// Timing and retry budgets belong to the AP service. The WPA state only
    /// exposes the message and replay counter of its current phase; it never
    /// reads time or schedules work itself.
    pub const fn outstanding_transmit(&self) -> Result<RsnTransmit, RsnStateError> {
        match self.phase {
            RsnApPhase::AwaitingMessage2 => Ok(RsnTransmit {
                message: RsnTxMessage::PairwiseMessage1,
                replay_counter: self.message1_replay,
                retransmission: self.retransmitted,
            }),
            RsnApPhase::AwaitingMessage4 => Ok(RsnTransmit {
                message: RsnTxMessage::PairwiseMessage3,
                replay_counter: self.message3_replay,
                retransmission: self.retransmitted,
            }),
            _ => Err(RsnStateError::WrongPhase),
        }
    }

    /// Retransmit Message 1 under the next replay counter.
    ///
    /// Every EAPOL-Key frame the authenticator sends carries a new, larger
    /// counter; Message 2 may answer any of the latest
    /// [`RSN_AP_REPLAY_WINDOW`] Message 1 counters. The counter after the
    /// latest stays free for Message 3, so a retransmission that would leave
    /// none is refused with `ReplayCounterExhausted` and the state is
    /// unchanged.
    pub fn retransmit_message1(&mut self) -> Result<RsnTransmit, RsnStateError> {
        if !matches!(self.phase, RsnApPhase::AwaitingMessage2) {
            return Err(RsnStateError::WrongPhase);
        }
        let replay_counter = self
            .message1_replay
            .checked_add(1)
            .filter(|counter| *counter != u64::MAX)
            .ok_or(RsnStateError::ReplayCounterExhausted)?;
        self.message1_replay = replay_counter;
        self.retransmitted = true;
        Ok(RsnTransmit {
            message: RsnTxMessage::PairwiseMessage1,
            replay_counter,
            retransmission: true,
        })
    }

    /// Retransmit Message 3 under the next replay counter. Message 4 may
    /// answer any of the latest [`RSN_AP_REPLAY_WINDOW`] Message 3 counters.
    /// On `ReplayCounterExhausted` the state is unchanged.
    pub fn retransmit_message3(&mut self) -> Result<RsnTransmit, RsnStateError> {
        if !matches!(self.phase, RsnApPhase::AwaitingMessage4) {
            return Err(RsnStateError::WrongPhase);
        }
        let replay_counter = self
            .message3_replay
            .checked_add(1)
            .ok_or(RsnStateError::ReplayCounterExhausted)?;
        self.message3_replay = replay_counter;
        self.retransmitted = true;
        Ok(RsnTransmit {
            message: RsnTxMessage::PairwiseMessage3,
            replay_counter,
            retransmission: true,
        })
    }

    pub fn on_frame<const N: usize>(
        &mut self,
        frame: OwnedEapolFrame<N>,
    ) -> Result<RsnApAction<N>, RsnStateError> {
        self.validate_frame(&frame)?;
        let key = frame.key_frame()?;
        if key.key_info().descriptor_version() != self.akm.key_descriptor_version().get() {
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
        let key = frame.key_frame()?;
        let replay = key.replay_counter();
        let nonce = *key.nonce();
        if !awaited(self.message1_first, self.message1_replay, replay) {
            return Err(RsnStateError::ReplayCounterMismatch);
        }
        if nonce.iter().all(|byte| *byte == 0) {
            return Err(RsnStateError::ZeroNonce);
        }

        match self.phase {
            RsnApPhase::AwaitingMessage2 => {
                self.supplicant_nonce = nonce;
                self.answered_replay = replay;
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
        let replay = frame.key_frame()?.replay_counter();
        match self.phase {
            // The window closed with the step: a later Message 4 neither
            // authorizes nor installs a key again.
            RsnApPhase::Authorized => return Ok(RsnApAction::None),
            RsnApPhase::AwaitingMessage4 | RsnApPhase::VerifyingMessage4 => {}
            _ => return Err(RsnStateError::UnexpectedMessage),
        }
        if !awaited(self.message3_first, self.message3_replay, replay) {
            return Err(RsnStateError::ReplayCounterMismatch);
        }
        match self.phase {
            RsnApPhase::AwaitingMessage4 => {
                self.answered_replay = replay;
                self.phase = RsnApPhase::VerifyingMessage4;
                let ticket = self.issue_ticket();
                Ok(RsnApAction::VerifyMessage4Mic {
                    ticket,
                    message4: frame,
                })
            }
            _ => Ok(RsnApAction::None),
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
        self.message3_first = self.message3_replay;
        self.retransmitted = false;
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
        let key = frame.key_frame()?;
        if key.message() != EapolKeyMessage::PairwiseMessage2
            || key.replay_counter() != self.answered_replay
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
        let key = frame.key_frame()?;
        if key.message() != EapolKeyMessage::PairwiseMessage4
            || key.replay_counter() != self.answered_replay
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

/// Whether `replay` is one of the latest [`RSN_AP_REPLAY_WINDOW`] counters
/// sent from `first` up to `latest`.
const fn awaited(first: u64, latest: u64, replay: u64) -> bool {
    if replay < first {
        return false;
    }
    match latest.checked_sub(replay) {
        Some(behind) => behind < RSN_AP_REPLAY_WINDOW,
        None => false,
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
        const {
            if P == 0 {
                panic!("an AP peer table must have a slot");
            }
        }
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
