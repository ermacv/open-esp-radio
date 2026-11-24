//! WPA2 handshakes, key/replay transitions and retry publication.
//! Every method borrows the same service and peer storage; no second owner exists.

use super::*;
use oer_protocols_ieee80211_core_security_cipher::aes::SoftwareAes128;
use oer_protocols_ieee80211_core_upper_mac::ccmp::{CcmpTxPacketNumber, InstalledCcmpTxKey};
use oer_protocols_ieee80211_core_upper_mac::rate_control::MpduOutcome;
use oer_protocols_ieee80211_wire::scan::ElementOccurrence;
use oer_protocols_ieee80211_wire::security::{SppCapability, SppError, rsn::rsnx_capabilities};

/// Why an association cannot use the advertised RSN profile.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApAssociationSecurityError {
    InvalidElements,
    RetainedElement(oer_protocols_ieee80211_wire::security::rsn::RsnSyntaxError),
    Spp(SppError),
}

impl ApAssociationSecurityError {
    /// Peer policy rejections may become a response; a retained invariant may not.
    pub(super) fn peer_rejection(self) -> Result<(), ApServiceError> {
        match self {
            Self::RetainedElement(error) => Err(ApServiceError::InvalidAssociationSecurity(error)),
            Self::InvalidElements | Self::Spp(_) => Ok(()),
        }
    }
}

impl<'peers, S: SppScope> AccessPointService<'peers, S> {
    /// Exact, non-mutating admission predicate used for both first and retry
    /// Association Requests.
    pub fn matches_association_security(
        &self,
        security: ApAssociationSecurityObservation<'_>,
    ) -> Result<bool, ApServiceError> {
        match admit_association_security::<S>(self.security_policy(), security) {
            Ok(_) => Ok(true),
            Err(error) => error.peer_rejection().map(|()| false),
        }
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
        let (true, ElementOccurrence::Once(rsn_ie)) = (security.privacy(), security.rsn_ie) else {
            return Err(ApAssociationSecurityError::InvalidElements);
        };
        let rsnxe = match security.rsnxe {
            ElementOccurrence::Absent => None,
            ElementOccurrence::Once(rsnxe) => Some(rsnxe),
            ElementOccurrence::Repeated => {
                return Err(ApAssociationSecurityError::InvalidElements);
            }
        };
        let rsn = validate_rsn_element(rsn_ie)
            .map_err(|_| ApAssociationSecurityError::InvalidElements)?;
        let spp = S::admit_ap(
            policy.spp_capability(),
            SppCapability::from_rsn(rsn.capabilities()).map_err(ApAssociationSecurityError::Spp)?,
        )
        .map_err(ApAssociationSecurityError::Spp)?;
        let protection = rsn.management_frame_protection();
        let management = policy.management_protection();
        if management.required() && !protection.capable()
            || !management.capable() && protection.required()
        {
            return Err(ApAssociationSecurityError::InvalidElements);
        }
        let akm = match policy {
            ApSecurityPolicy::Open => return Err(ApAssociationSecurityError::InvalidElements),
            ApSecurityPolicy::Wpa2Personal
                if rsn
                    .lists_akm(Akm::Psk)
                    .map_err(ApAssociationSecurityError::RetainedElement)?
                    && rsn.pmkid_count() == 0 =>
            {
                AssociationAkm::Psk
            }
            ApSecurityPolicy::Wpa2Personal => {
                return Err(ApAssociationSecurityError::InvalidElements);
            }
            // The BSS advertises hash to element, so an exchange uses it
            // whenever the station's RSNXE announces it as well.
            ApSecurityPolicy::Wpa3Personal
                if rsn
                    .lists_akm(Akm::Sae)
                    .map_err(ApAssociationSecurityError::RetainedElement)? =>
            {
                AssociationAkm::Sae(if rsnxe.is_some_and(announces_sae_h2e) {
                    SaePwe::HashToElement
                } else {
                    SaePwe::HuntingAndPecking
                })
            }
            ApSecurityPolicy::Wpa3Personal => {
                return Err(ApAssociationSecurityError::InvalidElements);
            }
        };
        let ies = OwnedAssociationSecurityIes::try_copy(rsn.owned(), rsnxe.unwrap_or(&[]))
            .map_err(|_| ApAssociationSecurityError::InvalidElements)?;
        let association = RsnAssociation {
            akm,
            spp,
            management: (management.capable() && protection.capable())
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

    /// Whether `peer`'s Message 4 verified: its handshake proves it, or
    /// its controlled port is open.
    pub fn wpa2_authorized(&self, peer: [u8; 6]) -> Result<bool, ApServiceError> {
        Ok(match &self.checked_peer(peer)?.handshake {
            ApHandshake::Handshaking(handshake) => handshake.authorization().is_ok(),
            ApHandshake::Authorized(_) => true,
            ApHandshake::Absent | ApHandshake::Associated(_) => false,
        })
    }

    /// Derive a peer's PTK with the suite its Association selected.
    pub fn derive_ptk(&self, peer: [u8; 6], context: PtkContext) -> Result<Ptk, ApServiceError> {
        let akm = match &self.checked_peer(peer)?.handshake {
            ApHandshake::Associated(start) => start.akm(),
            ApHandshake::Handshaking(handshake) => handshake.akm(),
            ApHandshake::Absent | ApHandshake::Authorized(_) => {
                return Err(ApServiceError::WrongPeerPhase);
            }
        };
        Ok(self.peer_pmk(peer)?.derive_ptk(akm, context))
    }

    /// Build Message 1 only after the successful Association Response reached
    /// TX complete: the handshake starts from its checked inputs, built
    /// straight into the holder's variant.
    pub fn begin_wpa2_frame<const N: usize>(
        &mut self,
        peer: [u8; 6],
    ) -> Result<ApEapolTransmit<N>, ApWpa2Error> {
        if self.link_protection() != LinkProtection::Ccmp {
            return Err(ApServiceError::SecurityModeMismatch.into());
        }
        let existing = self.checked_peer_mut(peer)?;
        // A refusal moves nothing: the state is asked before it is taken.
        if !matches!(existing.handshake, ApHandshake::Associated(_)) {
            return Err(ApServiceError::WrongPeerPhase.into());
        }
        let ApHandshake::Associated(start) =
            core::mem::replace(&mut existing.handshake, ApHandshake::Absent)
        else {
            // Not reached: the holder was `Associated` under the same borrow.
            return Err(ApServiceError::WrongPeerPhase.into());
        };
        let (handshake, frame, transmit) = RsnApHandshake::start::<N>(start);
        existing.handshake = ApHandshake::Handshaking(handshake);
        self.bind_eapol(frame, transmit)
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
        outcome: MpduOutcome,
        now: oer_portable_time::Instant,
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
        let current = match &existing.handshake {
            ApHandshake::Absent => return Err(ApServiceError::WrongPeerPhase.into()),
            ApHandshake::Associated(_) | ApHandshake::Authorized(_) => {
                return Ok(ApEapolTxCompletion::Superseded);
            }
            ApHandshake::Handshaking(handshake) => handshake.latest(),
        };
        // Only the latest built transmission owns the response window; none
        // does once the peer is authorized.
        let Some(current) = current else {
            return Ok(ApEapolTxCompletion::Superseded);
        };
        if current.message != transmit.message || current.replay_counter != transmit.replay_counter
        {
            return Ok(ApEapolTxCompletion::Superseded);
        }
        let stage_changed = existing.wpa2_retry.pending_message() != Some(transmit.message);
        let armed = stage_changed || !transmit.retransmission;
        if armed {
            existing.wpa2_retry.cancel();
            let mut alarm = existing.wpa2_retry.arm(transmit.message, now)?;
            // hostapd extends only the acknowledged initial M1 window. M3
            // retains the short first timeout, then uses the subsequent one.
            if outcome == MpduOutcome::Acknowledged
                && transmit.message
                    == oer_protocols_ieee80211_core_security_rsn::state::RsnTxMessage::PairwiseMessage1
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

    pub fn next_wpa2_retry_deadline(&self) -> Option<oer_portable_time::Instant> {
        self.storage()
            .peers
            .iter()
            .flatten()
            .filter_map(|peer| peer.wpa2_retry_alarm.map(|alarm| alarm.deadline))
            .min()
    }

    /// Consume at most one due authenticator retry edge.
    ///
    /// A retransmission takes the handshake's next replay counter; the
    /// station may answer it or an earlier transmission of the same step. A
    /// Message 3 carries as its Key RSC the last packet number `group_key`
    /// has handed out; `group_key` must be the installation of the GTK this
    /// BSS announces, or the call is refused with `GroupKeyMismatch` before
    /// any alarm is consumed.
    pub fn take_due_wpa2_retry<const N: usize, K: ApGroupTransmitKey>(
        &mut self,
        now: oer_portable_time::Instant,
        group_key: &InstalledCcmpTxKey<K>,
    ) -> Result<ApWpa2RetryProgress<N>, ApWpa2Error> {
        let group_packet_numbers = self.group_packet_numbers(group_key)?;
        let Some(peer) = self.storage_mut().peers.iter_mut().flatten().find(|peer| {
            peer.wpa2_retry_alarm
                .is_some_and(|alarm| alarm.deadline <= now)
        }) else {
            return Ok(ApWpa2RetryProgress::None);
        };
        let (peer_address, action) = {
            let alarm = peer
                .wpa2_retry_alarm
                .take()
                .ok_or(ApWpa2Error::UnexpectedAction)?;
            let action = peer.wpa2_retry.on_alarm(alarm, now)?;
            (peer.address, action)
        };
        match action {
            RsnRetryAction::Stale => Ok(ApWpa2RetryProgress::None),
            // The handshake retransmits the message of its phase, the one
            // the retry owner was armed for.
            RsnRetryAction::Transmit {
                message: _,
                next_alarm,
            } => {
                self.checked_peer_mut(peer_address)?.wpa2_retry_alarm = Some(next_alarm);
                let frame = self.retransmit(peer_address, group_packet_numbers)?;
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

    /// Advance the bounded authenticator through Message 2 or Message 4.
    ///
    /// PTK derivation, MIC verification and GTK wrapping are pure bounded
    /// operations of `oer-protocols-ieee80211-core-security-rsn`. Hardware key installation remains an
    /// explicit later edge in the chip AP engine. Message 3 carries as its
    /// Key RSC the last packet number `group_key` has handed out; `group_key`
    /// must be the installation of the GTK its key data names, or the frame
    /// is refused with `GroupKeyMismatch` before any handshake state changes.
    pub fn on_eapol<const N: usize, K: ApGroupTransmitKey>(
        &mut self,
        peer: [u8; 6],
        frame: OwnedEapolFrame<N>,
        group_key: &InstalledCcmpTxKey<K>,
    ) -> Result<ApWpa2Progress<N>, ApWpa2Error> {
        if self.link_protection() != LinkProtection::Ccmp {
            return Err(ApServiceError::SecurityModeMismatch.into());
        }
        let group_packet_numbers = self.group_packet_numbers(group_key)?;
        let index = self.peer_index(peer).ok_or(ApServiceError::UnknownPeer)?;
        match &self.checked_peer(peer)?.handshake {
            ApHandshake::Handshaking(_) => {}
            // A frame before Message 1 answers nothing; one after
            // authorization neither authorizes nor installs again.
            ApHandshake::Associated(_) | ApHandshake::Authorized(_) => {
                return Ok(ApWpa2Progress::None);
            }
            ApHandshake::Absent => return Err(ApServiceError::WrongPeerPhase.into()),
        }
        // Message 3 is built in the step that verifies Message 2, so its key
        // data is built before every received frame. Only a handshake that
        // awaits Message 2 builds it, with the IGTK's current IPN, which the
        // built Message 3 then keeps for its retries.
        let management_ipn = self.igtk().map_or([0; 6], RsnIgtk::packet_number);
        let plain = self.message3_key_data(management_ipn)?;
        let (handshake, pmk) = self.handshake_and_pmk(peer)?;
        let step = match handshake.on_eapol(
            pmk,
            frame,
            Message3Material {
                key_data: &plain,
                group_packet_number: group_packet_numbers.last(),
            },
            &mut SoftwareAes128,
        ) {
            Ok(step) => step,
            // Unsupported, stale and otherwise unauthenticated EAPOL is a
            // peer-local receive reject, not a role-control failure.
            Err(RsnApError::Rejected(_)) => return Ok(ApWpa2Progress::None),
            Err(error) => return Err(error.into()),
        };
        match step {
            RsnApEapol::None => Ok(ApWpa2Progress::None),
            RsnApEapol::Message3(frame, transmit) => {
                *self
                    .storage_mut()
                    .message3_management_ipns
                    .get_mut(index)
                    .ok_or(ApServiceError::PeerStorageInvariant { index })? = management_ipn;
                let existing = self.checked_peer_mut(peer)?;
                // Valid M2 closes the Message-1 response window. Message 3
                // receives a fresh schedule only after its own terminal TX
                // completion.
                existing.wpa2_retry.cancel();
                existing.wpa2_retry_alarm = None;
                Ok(ApWpa2Progress::Transmit(self.bind_eapol(frame, transmit)?))
            }
            RsnApEapol::Authorized => {
                let existing = self.checked_peer_mut(peer)?;
                existing.wpa2_retry.cancel();
                existing.wpa2_retry_alarm = None;
                Ok(ApWpa2Progress::AuthorizePeer)
            }
            // The handshake ended: its machine is dropped and no response
            // window remains.
            RsnApEapol::End(_) => {
                let existing = self.checked_peer_mut(peer)?;
                existing.handshake = ApHandshake::Absent;
                existing.wpa2_retry.cancel();
                existing.wpa2_retry_alarm = None;
                Ok(ApWpa2Progress::DeauthenticatePeer)
            }
        }
    }

    /// The handshake's current message again under the next replay counter:
    /// Message 1, or Message 3 with the key data of its first publication
    /// and the GTK's current packet number as its Key RSC.
    fn retransmit<const N: usize>(
        &mut self,
        peer: [u8; 6],
        group_packet_numbers: &CcmpTxPacketNumber,
    ) -> Result<ApEapolTransmit<N>, ApWpa2Error> {
        let index = self.peer_index(peer).ok_or(ApServiceError::UnknownPeer)?;
        let plain = self.message3_key_data(self.message3_management_ipn(index)?)?;
        let ApHandshake::Handshaking(handshake) = &mut self.checked_peer_mut(peer)?.handshake
        else {
            return Err(ApServiceError::WrongPeerPhase.into());
        };
        match handshake.retransmit::<N, _, _>(
            Message3Material {
                key_data: &plain,
                group_packet_number: group_packet_numbers.last(),
            },
            &mut SoftwareAes128,
        )? {
            RsnApRetransmit::Message1(frame, transmit)
            | RsnApRetransmit::Message3(frame, transmit) => self.bind_eapol(frame, transmit),
            // An authorized handshake cancelled its retry owner.
            RsnApRetransmit::Authorized => Err(ApWpa2Error::UnexpectedAction),
        }
    }

    pub fn pending_ptk(&self, peer: [u8; 6]) -> Result<&Ptk, ApServiceError> {
        match &self.checked_peer(peer)?.handshake {
            ApHandshake::Handshaking(handshake) => handshake.ptk(),
            ApHandshake::Absent | ApHandshake::Associated(_) | ApHandshake::Authorized(_) => None,
        }
        .ok_or(ApServiceError::WrongPeerPhase)
    }

    /// `peer`'s handshake and the PMK of its association, borrowed
    /// together: the BSS's for WPA2-Personal, the station's own for
    /// WPA3-Personal.
    fn handshake_and_pmk(
        &mut self,
        peer: [u8; 6],
    ) -> Result<(&mut RsnApHandshake, &Pmk), ApServiceError> {
        let Self {
            security,
            peer_storage,
            ..
        } = self;
        let ApPeer {
            handshake,
            pmk: station_pmk,
            ..
        } = peer_storage
            .storage
            .peers
            .iter_mut()
            .flatten()
            .find(|existing| existing.address == peer)
            .ok_or(ApServiceError::UnknownPeer)?;
        let pmk = match security {
            AccessPointSecurityMaterial::Open => Err(ApServiceError::SecurityModeMismatch),
            AccessPointSecurityMaterial::Wpa2Personal { pmk, .. } => Ok(&*pmk),
            AccessPointSecurityMaterial::Wpa3Personal { .. } => {
                station_pmk.as_ref().ok_or(ApServiceError::WrongPeerPhase)
            }
        }?;
        let ApHandshake::Handshaking(handshake) = handshake else {
            return Err(ApServiceError::WrongPeerPhase);
        };
        Ok((handshake, pmk))
    }

    /// The IGTK IPN `peer`'s first Message 3 carried, which its retries
    /// repeat.
    fn message3_management_ipn(&self, index: usize) -> Result<[u8; 6], ApServiceError> {
        self.storage()
            .message3_management_ipns
            .get(index)
            .copied()
            .ok_or(ApServiceError::PeerStorageInvariant { index })
    }

    /// The installation of the GTK this BSS announces in Message 3.
    pub fn group_key_installation(&self) -> Result<ApGroupKeyInstallation, ApServiceError> {
        match &self.security {
            AccessPointSecurityMaterial::Open => Err(ApServiceError::SecurityModeMismatch),
            AccessPointSecurityMaterial::Wpa2Personal {
                gtk,
                gtk_generation,
                ..
            }
            | AccessPointSecurityMaterial::Wpa3Personal {
                gtk,
                gtk_generation,
                ..
            } => Ok(ApGroupKeyInstallation {
                key_id: gtk.key_id(),
                generation: *gtk_generation,
            }),
        }
    }

    /// The packet numbers of `group_key` when it is the installation of the
    /// announced GTK. Reading them allocates none.
    fn group_packet_numbers<'k, K: ApGroupTransmitKey>(
        &self,
        group_key: &'k InstalledCcmpTxKey<K>,
    ) -> Result<&'k CcmpTxPacketNumber, ApWpa2Error> {
        if group_key.key().installation() != self.group_key_installation()? {
            return Err(ApWpa2Error::GroupKeyMismatch);
        }
        Ok(group_key.packet_numbers())
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
        management_ipn: [u8; 6],
    ) -> Result<RsnPlainKeyData<RSN_PLAIN_KEY_DATA_CAPACITY>, ApWpa2Error> {
        let policy = self.security_policy();
        let advertised = policy.advertisement();
        let (rsn, rsnx) = (advertised.rsne, advertised.rsnxe);
        let mut elements = [0_u8; 64];
        let length = rsn
            .len()
            .checked_add(rsnx.len())
            .ok_or(RsnFrameError::CapacityExceeded)?;
        let elements = elements
            .get_mut(..length)
            .ok_or(RsnFrameError::CapacityExceeded)?;
        for (destination, source) in elements.iter_mut().zip(rsn.iter().chain(rsnx)) {
            *destination = *source;
        }
        // A retried Message 3 repeats the IPN captured at its first
        // publication. Only WPA2 without management protection is claimed;
        // the WPA3/PMF correctness of that snapshot is not.
        let igtk = match self.igtk() {
            Some(igtk) => Some(RsnIgtk::new(igtk.key_id(), management_ipn, igtk.key())?),
            None => None,
        };
        Ok(RsnPlainKeyData::build(
            elements,
            self.gtk()?,
            igtk.as_ref(),
        )?)
    }

    pub fn authorize(
        &mut self,
        peer: [u8; 6],
        now: oer_portable_time::Instant,
    ) -> Result<(), ApServiceError> {
        if self.link_protection() != LinkProtection::Ccmp {
            return Err(ApServiceError::SecurityModeMismatch);
        }
        let inactive_timeout = self.inactive_timeout.duration();
        let management_protection = self.security_policy().management_protection().required();
        let index = self.peer_index(peer).ok_or(ApServiceError::UnknownPeer)?;
        let existing = self.checked_peer_mut(peer)?;
        let authorized = match &existing.handshake {
            ApHandshake::Handshaking(handshake) => handshake
                .authorization()
                .map_err(|_| ApServiceError::WrongPeerPhase)?,
            // Authorizing again changes no key and reopens no replay state.
            ApHandshake::Authorized(authorized) => *authorized,
            ApHandshake::Absent | ApHandshake::Associated(_) => {
                return Err(ApServiceError::WrongPeerPhase);
            }
        };
        // The handshake and its keys go; the proof stays.
        existing.handshake = ApHandshake::Authorized(authorized);
        existing.phase = ApPeerPhase::Authorized;
        existing.wpa2_retry.cancel();
        existing.wpa2_retry_alarm = None;
        existing.last_activity = now;
        existing.deadline = now.saturating_add(inactive_timeout);
        if management_protection {
            self.storage_mut()
                .management_replay
                .get_mut(index)
                .ok_or(ApServiceError::PeerStorageInvariant { index })?
                .activate();
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
    if (security.information_elements == oer_protocols_ieee80211_wire::scan::ElementList::Truncated)
        || (security.legacy_wpa == oer_protocols_ieee80211_wire::scan::ElementPresence::Present)
    {
        return Err(ApAssociationSecurityError::InvalidElements);
    }
    match policy.link_protection() {
        LinkProtection::Open => (!security.privacy()
            && security.rsn_ie == ElementOccurrence::Absent
            && security.rsnxe == ElementOccurrence::Absent)
            .then_some(AssociationSecurity::Open)
            .ok_or(ApAssociationSecurityError::InvalidElements),
        LinkProtection::Ccmp => {
            AccessPointService::<S>::validated_association_security(policy, security)
                .map(|admitted| AssociationSecurity::Rsn(admitted.association))
        }
    }
}

/// Whether an RSNXE announces SAE hash to element.
fn announces_sae_h2e(rsnxe: &[u8]) -> bool {
    rsnx_capabilities(rsnxe).is_some_and(|capabilities| capabilities.sae_h2e())
}

#[cfg(test)]
mod invariant_tests {
    #![allow(clippy::disallowed_macros)]
    use super::*;
    use oer_protocols_ieee80211_wire::security::rsn::RsnSyntaxError;

    #[test]
    fn retained_syntax_failures_cannot_become_peer_responses() {
        for source in [
            RsnSyntaxError::Malformed,
            RsnSyntaxError::UnsupportedVersion,
        ] {
            assert_eq!(
                ApAssociationSecurityError::RetainedElement(source).peer_rejection(),
                Err(ApServiceError::InvalidAssociationSecurity(source))
            );
        }
        assert_eq!(
            ApAssociationSecurityError::InvalidElements.peer_rejection(),
            Ok(())
        );
    }
}
