//! WPA2 handshakes, key/replay transitions and retry publication.
//! Every method borrows the same service and peer storage; no second owner exists.

use super::*;
use oer_ieee80211_mac::security::{SppCapability, SppError};

/// Why an association cannot use the advertised RSN profile.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApAssociationSecurityError {
    InvalidElements,
    Spp(SppError),
}

impl<'peers, S: SppScope> AccessPointService<'peers, S> {
    /// Exact, non-mutating admission predicate used for both first and retry
    /// Association Requests.
    pub fn matches_association_security(
        &self,
        security: ApAssociationSecurityObservation<'_>,
    ) -> bool {
        admit_association_security::<S>(self.security_policy(), security).is_ok()
    }

    /// The station's validated Association security elements under this
    /// BSS's `policy`. WPA2-Personal admits PSK without required management
    /// frame protection and without a PMKID, as it caches none; WPA3-Personal
    /// admits SAE with management frame protection, and a station may name
    /// the PMKIDs it resumes.
    pub(super) fn validated_association_security(
        policy: ApSecurityPolicy,
        security: ApAssociationSecurityObservation<'_>,
    ) -> Result<AdmittedRsn, ApAssociationSecurityError> {
        if !security.privacy
            || security.rsn_ie_count != 1
            || security.rsnxe_count > 1
            || security.rsnxe_count == 0 && security.rsnxe.is_some()
            || security.rsnxe_count == 1 && security.rsnxe.is_none()
        {
            return Err(ApAssociationSecurityError::InvalidElements);
        }
        let rsn = validate_rsn_element(
            security
                .rsn_ie
                .ok_or(ApAssociationSecurityError::InvalidElements)?,
        )
        .map_err(|_| ApAssociationSecurityError::InvalidElements)?;
        let spp = S::admit_ap(
            policy.spp_capability(),
            SppCapability::from_rsn(rsn.capabilities()).map_err(ApAssociationSecurityError::Spp)?,
        )
        .map_err(ApAssociationSecurityError::Spp)?;
        let protection = rsn.management_frame_protection();
        let management = policy.management_protection();
        if management.required() && !protection.capable
            || !management.capable() && protection.required
        {
            return Err(ApAssociationSecurityError::InvalidElements);
        }
        let akm = match policy {
            ApSecurityPolicy::Open => return Err(ApAssociationSecurityError::InvalidElements),
            ApSecurityPolicy::Wpa2Personal if rsn.lists_akm(Akm::Psk) && rsn.pmkid_count() == 0 => {
                AssociationAkm::Psk
            }
            ApSecurityPolicy::Wpa2Personal => {
                return Err(ApAssociationSecurityError::InvalidElements);
            }
            // The BSS advertises hash to element, so an exchange uses it
            // whenever the station's RSNXE announces it as well.
            ApSecurityPolicy::Wpa3Personal if rsn.lists_akm(Akm::Sae) => {
                AssociationAkm::Sae(if security.rsnxe.is_some_and(announces_sae_h2e) {
                    SaePwe::HashToElement
                } else {
                    SaePwe::HuntingAndPecking
                })
            }
            ApSecurityPolicy::Wpa3Personal => {
                return Err(ApAssociationSecurityError::InvalidElements);
            }
        };
        let ies = OwnedAssociationSecurityIes::try_copy(rsn.owned(), security.rsnxe.unwrap_or(&[]))
            .map_err(|_| ApAssociationSecurityError::InvalidElements)?;
        let association = RsnAssociation {
            akm,
            spp,
            management: (management.capable() && protection.capable)
                .then_some(GroupManagementCipher::BipCmac128),
            pmkid: None,
        };
        Ok(AdmittedRsn {
            element: rsn,
            ies,
            association,
        })
    }

    /// Signal that the successful Association Response reached TX complete.
    pub fn begin_wpa2(&self, peer: [u8; 6]) -> Result<ApMlmeAction, ApServiceError> {
        if self.link_protection() != LinkProtection::Ccmp {
            return Err(ApServiceError::SecurityModeMismatch);
        }
        let existing = self.checked_peer(peer)?;
        if existing.phase != ApPeerPhase::Securing {
            return Err(ApServiceError::WrongPeerPhase);
        }
        Ok(ApMlmeAction::BeginWpa2 { peer })
    }

    pub fn wpa2_mut(&mut self, peer: [u8; 6]) -> Result<&mut RsnApState, ApServiceError> {
        if self.link_protection() != LinkProtection::Ccmp {
            return Err(ApServiceError::SecurityModeMismatch);
        }
        let existing = self.checked_peer_mut(peer)?;
        existing.wpa2.as_mut().ok_or(ApServiceError::WrongPeerPhase)
    }

    pub fn wpa2_authorized(&self, peer: [u8; 6]) -> Result<bool, ApServiceError> {
        let existing = self.checked_peer(peer)?;
        Ok(existing.wpa2.as_ref().map(RsnApState::phase) == Some(RsnApPhase::Authorized))
    }

    /// Derive a peer's PTK with the suite its Association selected.
    pub fn derive_ptk(&self, peer: [u8; 6], context: PtkContext) -> Result<Ptk, ApServiceError> {
        let akm = self
            .checked_peer(peer)?
            .wpa2
            .as_ref()
            .ok_or(ApServiceError::WrongPeerPhase)?
            .akm();
        Ok(self.peer_pmk(peer)?.derive_ptk(akm, context))
    }

    /// Build Message 1 only after the successful Association Response reached
    /// TX complete. The AP state retains the replay/nonce transaction.
    pub fn begin_wpa2_frame<const N: usize>(
        &self,
        peer: [u8; 6],
    ) -> Result<ApEapolTransmit<N>, ApWpa2Error> {
        if self.link_protection() != LinkProtection::Ccmp {
            return Err(ApServiceError::SecurityModeMismatch.into());
        }
        let existing = self.checked_peer(peer)?;
        let state = existing
            .wpa2
            .as_ref()
            .ok_or(ApServiceError::WrongPeerPhase)?;
        let RsnApAction::Transmit(transmit) = state.message1(false)? else {
            return Err(ApWpa2Error::UnexpectedAction);
        };
        self.bind_eapol(
            build_ap_action_frame(state, transmit, [0; 8], &[])?,
            transmit,
        )
    }

    /// Bind a terminal EAPOL-Key TX completion to the generic finite retry
    /// owner. The context identifies the association, message and replay
    /// counter actually published. RX may have advanced to M3 or completed the
    /// handshake before an older TX returns; that terminal returns resources
    /// without changing the current response window. A new handshake message
    /// replaces the previous response window; completion of a retransmission
    /// keeps the alarm already advanced by the timer edge that produced it.
    pub fn observe_wpa2_transmit(
        &mut self,
        context: ApEapolTxContext,
        acknowledged: bool,
        now: oer_time::Instant,
    ) -> Result<ApEapolTxCompletion, ApWpa2Error> {
        let inactive_timeout = self.inactive_timeout.duration();
        let ApEapolTxContext {
            association,
            transmit,
        } = context;
        let Some(existing) = self.bound_association_mut(association) else {
            return Ok(ApEapolTxCompletion::Superseded);
        };
        if existing.phase != ApPeerPhase::Securing {
            return Ok(ApEapolTxCompletion::Superseded);
        }
        let state = existing
            .wpa2
            .as_ref()
            .ok_or(ApServiceError::WrongPeerPhase)?;
        if state.phase() == RsnApPhase::Authorized || state.phase() == RsnApPhase::Failed {
            return Ok(ApEapolTxCompletion::Superseded);
        }
        let current = state.retry_transmit()?;
        if current.message != transmit.message || current.replay_counter != transmit.replay_counter
        {
            return Ok(ApEapolTxCompletion::Superseded);
        }
        let stage_changed = existing.wpa2_retry.pending_message() != Some(transmit.message);
        let armed = stage_changed || !transmit.retransmission;
        if armed {
            existing.wpa2_retry.cancel();
            let mut alarm = existing.wpa2_retry.arm(transmit, now)?;
            // hostapd extends only the acknowledged initial M1 window. M3
            // retains the short first timeout, then uses the subsequent one.
            if acknowledged
                && transmit.message == oer_ieee80211_rsn::state::RsnTxMessage::PairwiseMessage1
            {
                alarm = existing
                    .wpa2_retry
                    .defer_first_after_ack(now)?
                    .ok_or(ApWpa2Error::UnexpectedAction)?;
            }
            existing.wpa2_retry_alarm = Some(alarm);
        }
        existing.last_activity = now;
        existing.deadline = now.saturating_add(inactive_timeout);
        Ok(if armed {
            ApEapolTxCompletion::WindowArmed
        } else {
            ApEapolTxCompletion::WindowRetained
        })
    }

    pub fn next_wpa2_retry_deadline(&self) -> Option<oer_time::Instant> {
        self.storage()
            .peers
            .iter()
            .flatten()
            .filter_map(|peer| peer.wpa2_retry_alarm.map(|alarm| alarm.deadline))
            .min()
    }

    /// Consume at most one due authenticator retry edge.
    pub fn take_due_wpa2_retry<const N: usize>(
        &mut self,
        now: oer_time::Instant,
    ) -> Result<ApWpa2RetryProgress<N>, ApWpa2Error> {
        let Some(index) = self.storage().peers.iter().position(|peer| {
            peer.as_ref()
                .and_then(|peer| peer.wpa2_retry_alarm)
                .is_some_and(|alarm| alarm.deadline <= now)
        }) else {
            return Ok(ApWpa2RetryProgress::None);
        };
        let (peer_address, action) = {
            let peer = self.storage_mut().peers[index]
                .as_mut()
                .expect("due WPA2 retry belongs to an occupied peer");
            let alarm = peer
                .wpa2_retry_alarm
                .take()
                .expect("due WPA2 retry retains its alarm");
            let action = peer.wpa2_retry.on_alarm(alarm, now)?;
            (peer.address, action)
        };
        match action {
            RsnRetryAction::Stale => Ok(ApWpa2RetryProgress::None),
            RsnRetryAction::Transmit { frame, next_alarm } => {
                self.checked_peer_mut(peer_address)?.wpa2_retry_alarm = Some(next_alarm);
                let frame = match frame.message {
                    oer_ieee80211_rsn::state::RsnTxMessage::PairwiseMessage1 => {
                        let state = self
                            .checked_peer(peer_address)?
                            .wpa2
                            .as_ref()
                            .ok_or(ApServiceError::WrongPeerPhase)?;
                        self.bind_eapol(build_ap_action_frame(state, frame, [0; 8], &[])?, frame)?
                    }
                    oer_ieee80211_rsn::state::RsnTxMessage::PairwiseMessage3 => {
                        let ApWpa2Progress::Transmit(frame) =
                            self.build_pending_transmit(peer_address, frame)?
                        else {
                            return Err(ApWpa2Error::UnexpectedAction);
                        };
                        frame
                    }
                    _ => return Err(ApWpa2Error::UnexpectedAction),
                };
                Ok(ApWpa2RetryProgress::Transmit(frame))
            }
            RsnRetryAction::Exhausted => {
                let peer = self.checked_peer_mut(peer_address)?;
                let close = ApPeerClose {
                    peer: peer.address,
                    kind: ApPeerCloseKind::Wpa2HandshakeTimeout,
                    was_associated: true,
                    maximum_legacy_rate_500kbps: peer.maximum_legacy_rate_500kbps,
                };
                peer.phase = ApPeerPhase::Closing;
                self.revise_status();
                Ok(ApWpa2RetryProgress::Close(close))
            }
        }
    }

    /// Advance the bounded authenticator state through Message 2 or Message 4.
    ///
    /// PTK derivation, MIC verification and GTK wrapping are pure bounded
    /// operations here. Hardware key installation remains an explicit later
    /// edge in the chip AP engine.
    pub fn on_eapol<const N: usize>(
        &mut self,
        peer: [u8; 6],
        frame: OwnedEapolFrame<N>,
    ) -> Result<ApWpa2Progress<N>, ApWpa2Error> {
        if self.link_protection() != LinkProtection::Ccmp {
            return Err(ApServiceError::SecurityModeMismatch.into());
        }
        let action = match self
            .checked_peer_mut(peer)?
            .wpa2
            .as_mut()
            .ok_or(ApServiceError::WrongPeerPhase)?
            .on_frame(frame)
        {
            Ok(action) => action,
            Err(error) if error.is_peer_input_rejection() => {
                // Unsupported, stale and otherwise unauthenticated EAPOL is
                // a peer-local receive reject, not a role-control failure.
                return Ok(ApWpa2Progress::None);
            }
            Err(error) => return Err(error.into()),
        };
        match action {
            RsnApAction::None => Ok(ApWpa2Progress::None),
            RsnApAction::DerivePtk {
                ticket,
                context,
                message2,
            } => self.complete_message2(peer, ticket, context, message2),
            RsnApAction::VerifyMessage4Mic { ticket, message4 } => {
                let valid = {
                    let ptk = self
                        .checked_peer(peer)?
                        .pending_ptk
                        .as_ref()
                        .ok_or(ApWpa2Error::MissingPairwiseKey)?;
                    message4.key_frame().verify_mic(ptk)
                };
                let action = self
                    .checked_peer_mut(peer)?
                    .wpa2
                    .as_mut()
                    .ok_or(ApServiceError::WrongPeerPhase)?
                    .complete_message4_mic(ticket, message4, valid)?;
                match action {
                    RsnApAction::AuthorizePeer => {
                        let existing = self.checked_peer_mut(peer)?;
                        existing.wpa2_retry.cancel();
                        existing.wpa2_retry_alarm = None;
                        Ok(ApWpa2Progress::AuthorizePeer)
                    }
                    RsnApAction::None => Ok(ApWpa2Progress::None),
                    RsnApAction::DeauthenticatePeer => Ok(ApWpa2Progress::DeauthenticatePeer),
                    _ => Err(ApWpa2Error::UnexpectedAction),
                }
            }
            RsnApAction::Transmit(transmit) => self.build_pending_transmit(peer, transmit),
            RsnApAction::DeauthenticatePeer => Ok(ApWpa2Progress::DeauthenticatePeer),
            _ => Err(ApWpa2Error::UnexpectedAction),
        }
    }

    fn complete_message2<const N: usize>(
        &mut self,
        peer: [u8; 6],
        ticket: oer_ieee80211_rsn::state::RsnTicket,
        context: Wpa2StatePtkContext,
        message2: OwnedEapolFrame<N>,
    ) -> Result<ApWpa2Progress<N>, ApWpa2Error> {
        let ptk = self.derive_ptk(
            peer,
            PtkContext {
                authenticator_address: context.authenticator_address,
                supplicant_address: context.supplicant_address,
                authenticator_nonce: context.authenticator_nonce,
                supplicant_nonce: context.supplicant_nonce,
            },
        )?;
        let action = self
            .checked_peer_mut(peer)?
            .wpa2
            .as_mut()
            .ok_or(ApServiceError::WrongPeerPhase)?
            .complete_ptk(ticket, message2, true)?;
        let RsnApAction::VerifyMessage2Mic { ticket, message2 } = action else {
            return Err(ApWpa2Error::UnexpectedAction);
        };
        let valid = message2.key_frame().verify_mic(&ptk);
        // The association commitment is an authenticated semantic binding.
        // Do not let attacker-controlled Key Data decide peer teardown until
        // this exact M2 has passed its PTK-derived MIC.
        let association_security_ies_match = valid
            && self
                .checked_peer(peer)?
                .association_security_binding
                .as_ref()
                .is_some_and(|binding| {
                    self.peer_pmk(peer)
                        .is_ok_and(|pmk| binding.matches(pmk, message2.key_frame().key_data()))
                });
        let action = self
            .checked_peer_mut(peer)?
            .wpa2
            .as_mut()
            .ok_or(ApServiceError::WrongPeerPhase)?
            .complete_message2_mic(ticket, message2, valid)?;
        let ticket = match action {
            RsnApAction::PrepareMessage3 { ticket } => ticket,
            RsnApAction::None => return Ok(ApWpa2Progress::None),
            RsnApAction::DeauthenticatePeer => {
                return Ok(ApWpa2Progress::DeauthenticatePeer);
            }
            _ => return Err(ApWpa2Error::UnexpectedAction),
        };

        if !association_security_ies_match {
            let action = self
                .checked_peer_mut(peer)?
                .wpa2
                .as_mut()
                .ok_or(ApServiceError::WrongPeerPhase)?
                .complete_message3_preparation::<N>(ticket, false)?;
            return match action {
                RsnApAction::DeauthenticatePeer => Ok(ApWpa2Progress::DeauthenticatePeer),
                _ => Err(ApWpa2Error::UnexpectedAction),
            };
        }

        let index = self.peer_index(peer).ok_or(ApServiceError::UnknownPeer)?;
        let management_ipn = self.igtk().map_or([0; 6], RsnIgtk::packet_number);
        self.storage_mut().message3_management_ipns[index] = management_ipn;
        let plain = self.message3_key_data(peer)?;
        let wrapped = software_aes128_key_wrap(ptk.kek(), plain.as_bytes())?;
        let action = self
            .checked_peer_mut(peer)?
            .wpa2
            .as_mut()
            .ok_or(ApServiceError::WrongPeerPhase)?
            .complete_message3_preparation::<N>(ticket, true)?;
        let RsnApAction::Transmit(transmit) = action else {
            return Err(ApWpa2Error::UnexpectedAction);
        };
        let state = self
            .checked_peer(peer)?
            .wpa2
            .as_ref()
            .ok_or(ApServiceError::WrongPeerPhase)?;
        let response =
            build_ap_action_frame(state, transmit, [0; 8], wrapped.as_bytes())?.authenticate(&ptk);
        let existing = self.checked_peer_mut(peer)?;
        existing.pending_ptk = Some(ptk);
        // Valid M2 closes the Message-1 response window. Message 3 receives a
        // fresh schedule only after its own terminal TX completion.
        existing.wpa2_retry.cancel();
        existing.wpa2_retry_alarm = None;
        Ok(ApWpa2Progress::Transmit(
            self.bind_eapol(response, transmit)?,
        ))
    }

    fn build_pending_transmit<const N: usize>(
        &self,
        peer: [u8; 6],
        transmit: oer_ieee80211_rsn::state::RsnTransmit,
    ) -> Result<ApWpa2Progress<N>, ApWpa2Error> {
        let existing = self.checked_peer(peer)?;
        let state = existing
            .wpa2
            .as_ref()
            .ok_or(ApServiceError::WrongPeerPhase)?;
        let ptk = existing
            .pending_ptk
            .as_ref()
            .ok_or(ApWpa2Error::MissingPairwiseKey)?;
        let plain = self.message3_key_data(peer)?;
        let wrapped = software_aes128_key_wrap(ptk.kek(), plain.as_bytes())?;
        let response =
            build_ap_action_frame(state, transmit, [0; 8], wrapped.as_bytes())?.authenticate(ptk);
        Ok(ApWpa2Progress::Transmit(
            self.bind_eapol(response, transmit)?,
        ))
    }

    pub fn pending_ptk(&self, peer: [u8; 6]) -> Result<&Ptk, ApServiceError> {
        self.checked_peer(peer)?
            .pending_ptk
            .as_ref()
            .ok_or(ApServiceError::WrongPeerPhase)
    }

    pub fn gtk(&self) -> Result<&RsnGtk, ApServiceError> {
        match &self.security {
            AccessPointSecurityMaterial::Open => Err(ApServiceError::SecurityModeMismatch),
            AccessPointSecurityMaterial::Wpa2Personal { gtk, .. }
            | AccessPointSecurityMaterial::Wpa3Personal { gtk, .. } => Ok(gtk),
        }
    }

    /// The IGTK of a BSS that protects management frames, with the current
    /// allocated IPN frontier delivered to newly joining peers in Message 3.
    pub fn igtk(&self) -> Option<&RsnIgtk> {
        match &self.security {
            AccessPointSecurityMaterial::Wpa3Personal { igtk, .. } => Some(igtk),
            AccessPointSecurityMaterial::Open
            | AccessPointSecurityMaterial::Wpa2Personal { .. } => None,
        }
    }

    /// Message 3's key data: this BSS's RSN element and RSNXE as advertised,
    /// the GTK and, when management frames are protected, the IGTK.
    fn message3_key_data(
        &self,
        peer: [u8; 6],
    ) -> Result<RsnPlainKeyData<RSN_PLAIN_KEY_DATA_CAPACITY>, ApWpa2Error> {
        let policy = self.security_policy();
        let advertised = policy.advertisement();
        let (rsn, rsnx) = (advertised.rsne, advertised.rsnxe);
        let mut elements = [0_u8; 64];
        elements[..rsn.len()].copy_from_slice(rsn);
        elements[rsn.len()..rsn.len() + rsnx.len()].copy_from_slice(rsnx);
        let index = self.peer_index(peer).ok_or(ApServiceError::UnknownPeer)?;
        let igtk = self.igtk().map(|igtk| {
            RsnIgtk::new(
                igtk.key_id(),
                self.storage().message3_management_ipns[index],
                igtk.key(),
            )
            .expect("the existing IGTK key identifier is valid")
        });
        Ok(RsnPlainKeyData::build(
            &elements[..rsn.len() + rsnx.len()],
            self.gtk()?,
            igtk.as_ref(),
        )?)
    }

    pub fn authorize(
        &mut self,
        peer: [u8; 6],
        now: oer_time::Instant,
    ) -> Result<(), ApServiceError> {
        if self.link_protection() != LinkProtection::Ccmp {
            return Err(ApServiceError::SecurityModeMismatch);
        }
        let inactive_timeout = self.inactive_timeout.duration();
        let management_protection = self.security_policy().management_protection().required();
        let index = self.peer_index(peer).ok_or(ApServiceError::UnknownPeer)?;
        let existing = self.checked_peer_mut(peer)?;
        if existing.wpa2.as_ref().map(RsnApState::phase) != Some(RsnApPhase::Authorized) {
            return Err(ApServiceError::WrongPeerPhase);
        }
        existing.phase = ApPeerPhase::Authorized;
        existing.association_security_binding = None;
        existing.pending_ptk = None;
        existing.wpa2_retry.cancel();
        existing.wpa2_retry_alarm = None;
        existing.last_activity = now;
        existing.deadline = now.saturating_add(inactive_timeout);
        if management_protection {
            self.storage_mut().management_replay[index].activate();
        }
        self.revise_status();
        Ok(())
    }

    /// The PMK of `peer`'s association: the BSS's for WPA2-Personal, the
    /// station's own for WPA3-Personal.
    pub(super) fn peer_pmk(&self, peer: [u8; 6]) -> Result<&Pmk, ApServiceError> {
        match &self.security {
            AccessPointSecurityMaterial::Open => Err(ApServiceError::SecurityModeMismatch),
            AccessPointSecurityMaterial::Wpa2Personal { pmk, .. } => Ok(pmk),
            AccessPointSecurityMaterial::Wpa3Personal { .. } => self
                .checked_peer(peer)?
                .pmk
                .as_ref()
                .ok_or(ApServiceError::WrongPeerPhase),
        }
    }
}

/// A station's admitted RSN Association elements and what they negotiate.
pub(super) struct AdmittedRsn {
    pub(super) element: ValidatedRsnElement,
    pub(super) ies: OwnedAssociationSecurityIes,
    pub(super) association: RsnAssociation,
}

/// The security a station's Association Request negotiates with a BSS
/// offering `policy`, or the typed reason the BSS refuses it.
///
/// An Open BSS admits only a request without Privacy, RSN element or RSNXE.
/// WPA2-Personal admits PSK without required management frame protection
/// and without a PMKID, as it caches none; WPA3-Personal admits SAE with
/// management frame protection, and a station may name the PMKIDs it
/// resumes.
pub fn admit_association_security<S: SppScope>(
    policy: ApSecurityPolicy,
    security: ApAssociationSecurityObservation<'_>,
) -> Result<AssociationSecurity, ApAssociationSecurityError> {
    if security.malformed_elements || security.legacy_wpa_present {
        return Err(ApAssociationSecurityError::InvalidElements);
    }
    match policy.link_protection() {
        LinkProtection::Open => (!security.privacy
            && security.rsn_ie_count == 0
            && security.rsn_ie.is_none()
            && security.rsnxe_count == 0
            && security.rsnxe.is_none())
        .then_some(AssociationSecurity::Open)
        .ok_or(ApAssociationSecurityError::InvalidElements),
        LinkProtection::Ccmp => {
            AccessPointService::<S>::validated_association_security(policy, security)
                .map(|admitted| AssociationSecurity::Rsn(admitted.association))
        }
    }
}

/// Whether an RSNXE announces SAE hash to element (bit 5 of its first
/// capability octet).
fn announces_sae_h2e(rsnxe: &[u8]) -> bool {
    rsnxe
        .get(2)
        .is_some_and(|capabilities| capabilities & (1 << 5) != 0)
}
