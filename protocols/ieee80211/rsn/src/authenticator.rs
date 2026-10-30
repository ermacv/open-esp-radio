//! Allocation-free RSN authenticator orchestration, the counterpart of
//! [`crate::supplicant`].
//!
//! [`RsnApState`] remains the protocol transition owner. This module joins
//! its transitions to PTK derivation from the association's PMK, Message 2
//! and Message 4 MIC verification, the binding of the station's exact
//! Association security IEs, and the wrap and MIC of Message 3. The access
//! point keeps its peer table, the response windows, the group keys and the
//! Message 3 key data it advertises, and transmission; the SAE side of an
//! access point is [`sae`].
//!
//! Received EAPOL reaches this module; nothing here panics on it.

pub mod sae;

use crate::{
    AssociationSecurityBinding, OwnedEapolFrame, Pmk, Ptk, PtkContext,
    frames::{RsnFrameError, RsnPlainKeyData, RsnTxFrame, build_ap_action_frame},
    state::{RsnApAction, RsnApPhase, RsnApState, RsnStateError, RsnTicket, RsnTransmit},
};
use oer_crypto::rfc3394::KeyWrapError;
use oer_ieee80211_cipher::aes::software_aes128_key_wrap;

/// Why the authenticator refused a step.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RsnAuthenticatorError {
    /// The received EAPOL-Key frame was refused on admission: unsupported,
    /// stale or otherwise unauthenticated peer input, local to that peer.
    Rejected(RsnStateError),
    /// A transition of an admitted frame failed.
    State(RsnStateError),
    Frame(RsnFrameError),
    KeyWrap(KeyWrapError),
    /// Message 3 or Message 4 needs the PTK of a verified Message 2.
    MissingPairwiseKey,
    /// The state machine asked for an action this step does not take.
    UnexpectedAction,
}

impl From<RsnStateError> for RsnAuthenticatorError {
    fn from(error: RsnStateError) -> Self {
        Self::State(error)
    }
}

impl From<oer_ieee80211_wire::eapol::EapolParseError> for RsnAuthenticatorError {
    fn from(error: oer_ieee80211_wire::eapol::EapolParseError) -> Self {
        Self::State(RsnStateError::from(error))
    }
}

impl From<RsnFrameError> for RsnAuthenticatorError {
    fn from(error: RsnFrameError) -> Self {
        Self::Frame(error)
    }
}

impl From<KeyWrapError> for RsnAuthenticatorError {
    fn from(error: KeyWrapError) -> Self {
        Self::KeyWrap(error)
    }
}

/// A verified Message 2 whose security IEs match the association: its PTK,
/// held until the caller supplies Message 3's key data.
pub struct RsnMessage3Preparation {
    ticket: RsnTicket,
    ptk: Ptk,
}

/// What a received EAPOL-Key frame asks of the access point.
pub enum RsnAuthenticatorStep {
    /// Nothing; the frame changed no state the access point acts on.
    None,
    /// Build the first Message 3 from its key data with
    /// [`RsnAuthenticator::complete_message3`].
    PrepareMessage3(RsnMessage3Preparation),
    /// Send the pending Message 3 again, built by
    /// [`RsnAuthenticator::message3`].
    Retransmit(RsnTransmit),
    /// A verified Message 4: the peer is authorized.
    Authorized,
    /// The handshake failed; deauthenticate the peer.
    Deauthenticate,
}

/// The authenticator side of one station's four-way handshake.
pub struct RsnAuthenticator {
    state: RsnApState,
    association_security_binding: Option<AssociationSecurityBinding>,
    pending_ptk: Option<Ptk>,
}

impl RsnAuthenticator {
    /// Start the handshake of an association whose security IEs the PMK
    /// has bound.
    pub const fn new(state: RsnApState, binding: AssociationSecurityBinding) -> Self {
        Self {
            state,
            association_security_binding: Some(binding),
            pending_ptk: None,
        }
    }

    pub const fn state(&self) -> &RsnApState {
        &self.state
    }

    pub const fn phase(&self) -> RsnApPhase {
        self.state.phase()
    }

    /// The PTK of the verified Message 2, until authorization.
    pub const fn pending_ptk(&self) -> Option<&Ptk> {
        self.pending_ptk.as_ref()
    }

    /// Message 1 and the transmission it is.
    pub fn message1<const N: usize>(
        &self,
    ) -> Result<(RsnTxFrame<N>, RsnTransmit), RsnAuthenticatorError> {
        let RsnApAction::Transmit(transmit) = self.state.message1(false)? else {
            return Err(RsnAuthenticatorError::UnexpectedAction);
        };
        Ok((self.message1_frame(transmit)?, transmit))
    }

    /// The Message 1 frame of `transmit`, the original or a retransmission.
    pub fn message1_frame<const N: usize>(
        &self,
        transmit: RsnTransmit,
    ) -> Result<RsnTxFrame<N>, RsnAuthenticatorError> {
        Ok(build_ap_action_frame(&self.state, transmit, [0; 8], &[])?)
    }

    /// Advance through Message 2 or Message 4 with the association's `pmk`.
    ///
    /// A Message 2 derives the PTK and verifies its MIC; only then may its
    /// Key Data, the station's security IEs, decide the outcome, and a
    /// mismatch ends the handshake.
    pub fn on_frame<const N: usize>(
        &mut self,
        pmk: &Pmk,
        frame: OwnedEapolFrame<N>,
    ) -> Result<RsnAuthenticatorStep, RsnAuthenticatorError> {
        let action = self
            .state
            .on_frame(frame)
            .map_err(RsnAuthenticatorError::Rejected)?;
        match action {
            RsnApAction::None => Ok(RsnAuthenticatorStep::None),
            RsnApAction::DerivePtk {
                ticket,
                context,
                message2,
            } => self.on_message2(pmk, ticket, context, message2),
            RsnApAction::VerifyMessage4Mic { ticket, message4 } => {
                let valid = {
                    let ptk = self
                        .pending_ptk
                        .as_ref()
                        .ok_or(RsnAuthenticatorError::MissingPairwiseKey)?;
                    message4.key_frame()?.verify_mic(ptk)
                };
                match self.state.complete_message4_mic(ticket, message4, valid)? {
                    RsnApAction::AuthorizePeer => Ok(RsnAuthenticatorStep::Authorized),
                    RsnApAction::None => Ok(RsnAuthenticatorStep::None),
                    RsnApAction::DeauthenticatePeer => Ok(RsnAuthenticatorStep::Deauthenticate),
                    _ => Err(RsnAuthenticatorError::UnexpectedAction),
                }
            }
            RsnApAction::Transmit(transmit) => Ok(RsnAuthenticatorStep::Retransmit(transmit)),
            RsnApAction::DeauthenticatePeer => Ok(RsnAuthenticatorStep::Deauthenticate),
            _ => Err(RsnAuthenticatorError::UnexpectedAction),
        }
    }

    fn on_message2<const N: usize>(
        &mut self,
        pmk: &Pmk,
        ticket: RsnTicket,
        context: crate::state::PtkContext,
        message2: OwnedEapolFrame<N>,
    ) -> Result<RsnAuthenticatorStep, RsnAuthenticatorError> {
        let ptk = pmk.derive_ptk(
            self.state.akm(),
            PtkContext {
                authenticator_address: context.authenticator_address,
                supplicant_address: context.supplicant_address,
                authenticator_nonce: context.authenticator_nonce,
                supplicant_nonce: context.supplicant_nonce,
            },
        );
        let RsnApAction::VerifyMessage2Mic { ticket, message2 } =
            self.state.complete_ptk(ticket, message2, true)?
        else {
            return Err(RsnAuthenticatorError::UnexpectedAction);
        };
        let key_frame = message2.key_frame()?;
        let valid = key_frame.verify_mic(&ptk);
        // The association commitment is an authenticated semantic binding.
        // Do not let attacker-controlled Key Data decide peer teardown until
        // this exact M2 has passed its PTK-derived MIC.
        let association_security_ies_match = valid
            && self
                .association_security_binding
                .as_ref()
                .is_some_and(|binding| binding.matches(pmk, key_frame.key_data()));
        let ticket = match self.state.complete_message2_mic(ticket, message2, valid)? {
            RsnApAction::PrepareMessage3 { ticket } => ticket,
            RsnApAction::None => return Ok(RsnAuthenticatorStep::None),
            RsnApAction::DeauthenticatePeer => return Ok(RsnAuthenticatorStep::Deauthenticate),
            _ => return Err(RsnAuthenticatorError::UnexpectedAction),
        };
        if !association_security_ies_match {
            return match self
                .state
                .complete_message3_preparation::<N>(ticket, false)?
            {
                RsnApAction::DeauthenticatePeer => Ok(RsnAuthenticatorStep::Deauthenticate),
                _ => Err(RsnAuthenticatorError::UnexpectedAction),
            };
        }
        Ok(RsnAuthenticatorStep::PrepareMessage3(
            RsnMessage3Preparation { ticket, ptk },
        ))
    }

    /// Wrap `key_data` under the prepared PTK and build the first Message 3.
    /// The PTK becomes pending only once the frame exists.
    pub fn complete_message3<const N: usize, const K: usize>(
        &mut self,
        preparation: RsnMessage3Preparation,
        key_data: &RsnPlainKeyData<K>,
    ) -> Result<(RsnTxFrame<N>, RsnTransmit), RsnAuthenticatorError> {
        let RsnMessage3Preparation { ticket, ptk } = preparation;
        let wrapped = software_aes128_key_wrap(ptk.kek(), key_data.as_bytes())?;
        let RsnApAction::Transmit(transmit) = self
            .state
            .complete_message3_preparation::<N>(ticket, true)?
        else {
            return Err(RsnAuthenticatorError::UnexpectedAction);
        };
        let frame = build_ap_action_frame(&self.state, transmit, [0; 8], wrapped.as_bytes())?
            .authenticate(&ptk);
        self.pending_ptk = Some(ptk);
        Ok((frame, transmit))
    }

    /// The pending Message 3 of `transmit` with `key_data`, for a
    /// retransmission.
    pub fn message3<const N: usize, const K: usize>(
        &self,
        transmit: RsnTransmit,
        key_data: &RsnPlainKeyData<K>,
    ) -> Result<RsnTxFrame<N>, RsnAuthenticatorError> {
        let ptk = self
            .pending_ptk
            .as_ref()
            .ok_or(RsnAuthenticatorError::MissingPairwiseKey)?;
        let wrapped = software_aes128_key_wrap(ptk.kek(), key_data.as_bytes())?;
        Ok(
            build_ap_action_frame(&self.state, transmit, [0; 8], wrapped.as_bytes())?
                .authenticate(ptk),
        )
    }

    /// The peer is authorized: drop the handshake's PTK and IE binding.
    pub fn forget_handshake_keys(&mut self) {
        self.association_security_binding = None;
        self.pending_ptk = None;
    }
}

#[cfg(test)]
mod tests;
