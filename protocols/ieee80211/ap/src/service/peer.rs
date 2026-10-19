//! Peer authentication, association, sequence spaces and closure.
//! Every method borrows the same service and peer storage; no second owner exists.

use super::*;
use oer_ieee80211_mac::sequence::SequenceNumber;

impl<'peers, S: SppScope> AccessPointService<'peers, S> {
    pub fn next_management_sequence(&mut self) -> SequenceNumber {
        let sequence = self.next_management_sequence;
        self.next_management_sequence = sequence.next();
        sequence
    }

    /// Consume the non-QoS data sequence space used by the initial EAPOL and
    /// legacy data path. Per-TID sequence spaces are introduced with QoS.
    pub fn next_data_sequence(&mut self) -> SequenceNumber {
        let sequence = self.next_data_sequence;
        self.next_data_sequence = sequence.next();
        sequence
    }

    pub const fn current_data_sequence(&self) -> SequenceNumber {
        self.next_data_sequence
    }

    /// Consume one per-peer/per-TID sequence for protected data or the
    /// bounded Open QoS A-MSDU path. Security mode does not partition the
    /// receiver's IEEE sequence space.
    pub fn next_qos_sequence(&mut self, peer: [u8; 6], tid: u8) -> Option<SequenceNumber> {
        let sequence = self
            .checked_peer_mut(peer)
            .ok()?
            .next_qos_sequences
            .get_mut(usize::from(tid))?;
        let current = *sequence;
        *sequence = current.next();
        Some(current)
    }

    /// Inspect a peer/TID sequence without consuming it during preflight.
    pub fn current_qos_sequence(&self, peer: [u8; 6], tid: u8) -> Option<SequenceNumber> {
        self.checked_peer(peer)
            .ok()?
            .next_qos_sequences
            .get(usize::from(tid))
            .copied()
    }

    pub fn authenticate_open(&mut self, peer: [u8; 6], now: oer_time::Instant) -> ApMlmeAction {
        let status = self.authenticate(peer, now);
        ApMlmeAction::AuthenticationResponse { peer, status }
    }

    /// Record `peer`'s accepted SAE exchange: the station is authenticated
    /// with its PMK, and the security association is cached for a later
    /// association to resume. Returns the status the station's slot allows.
    pub fn authenticate_sae(
        &mut self,
        peer: [u8; 6],
        pmk: Pmk,
        pmkid: [u8; AP_PMKID_LEN],
        now: oer_time::Instant,
    ) -> Result<u16, ApServiceError> {
        if self.security_policy() != ApSecurityPolicy::Wpa3Personal {
            return Err(ApServiceError::SecurityModeMismatch);
        }
        self.storage_mut()
            .pmksa
            .insert(ApPmksa::new(peer, pmk.duplicate(), pmkid));
        let status = self.authenticate(peer, now);
        if status == AP_STATUS_SUCCESS {
            let existing = self.checked_peer_mut(peer)?;
            existing.pmk = Some(pmk);
            existing.pmkid = Some(pmkid);
        }
        Ok(status)
    }

    /// Give `peer` a fresh Authenticated entry, keeping the AID of an entry
    /// it already had.
    fn authenticate(&mut self, peer: [u8; 6], now: oer_time::Instant) -> u16 {
        let (status, changed) = if let Some(index) = self.peer_index(peer) {
            let association_id = self.storage().peers[index]
                .as_ref()
                .expect("peer index resolves an occupied entry")
                .association_id;
            self.advance_peer_generation();
            let association_epoch = self.storage().generation;
            self.storage_mut().management_replay[index] =
                management::ApManagementReplay::inactive();
            self.storage_mut().peers[index] = Some(ApPeer::authenticated(
                peer,
                association_id,
                association_epoch,
                now,
            ));
            (AP_STATUS_SUCCESS, true)
        } else if self.occupied_count() >= self.client_limit.get() {
            (AP_STATUS_TOO_MANY_STATIONS, false)
        } else if let Some(index) = self.storage().peers.iter().position(Option::is_none) {
            let association_id = u16::try_from(index + 1).expect("fifteen AIDs fit u16");
            self.advance_peer_generation();
            let association_epoch = self.storage().generation;
            self.storage_mut().management_replay[index] =
                management::ApManagementReplay::inactive();
            self.storage_mut().peers[index] = Some(ApPeer::authenticated(
                peer,
                association_id,
                association_epoch,
                now,
            ));
            (AP_STATUS_SUCCESS, true)
        } else {
            (AP_STATUS_TOO_MANY_STATIONS, false)
        };
        if changed {
            self.revise_status();
        }
        status
    }

    /// Admit an association into an RSN epoch. A WPA3 station uses the PMK of
    /// its accepted SAE exchange, or resumes a cached one by naming its
    /// PMKID; a station that names only unknown PMKIDs, or that neither
    /// authenticated by SAE nor names a PMKID, is refused with status 53.
    pub fn associate_rsn(
        &mut self,
        peer: [u8; 6],
        security: ApAssociationSecurityObservation<'_>,
        capabilities: ApAssociationCapabilities,
        authenticator_nonce: [u8; 32],
        initial_replay_counter: u64,
        now: oer_time::Instant,
    ) -> Result<ApMlmeAction, ApServiceError> {
        if self.link_protection() != LinkProtection::Ccmp {
            return Err(ApServiceError::SecurityModeMismatch);
        }
        if self.checked_peer(peer)?.phase != ApPeerPhase::Authenticated {
            return Err(ApServiceError::WrongPeerPhase);
        }
        let policy = self.security_policy();
        let association_security =
            match if security.malformed_elements || security.legacy_wpa_present {
                Err(ApAssociationSecurityError::InvalidElements)
            } else {
                Self::validated_association_security(policy, security)
            } {
                Ok(admitted) => admitted,
                Err(_) => {
                    return Ok(ApMlmeAction::AssociationResponse {
                        peer,
                        status: AP_STATUS_INVALID_RSN,
                        association_id: None,
                    });
                }
            };
        let akm = association_security.association.akm.akm();
        let resumed = if policy == ApSecurityPolicy::Wpa3Personal {
            match self.resume_sae_pmk(peer, association_security.element.pmkids())? {
                Some(resumed) => Some(resumed),
                None => {
                    return Ok(ApMlmeAction::AssociationResponse {
                        peer,
                        status: AP_STATUS_INVALID_PMKID,
                        association_id: None,
                    });
                }
            }
        } else {
            None
        };
        let association_security_binding = Some(match &resumed {
            Some((pmk, _)) => {
                pmk.bind_association_security_ies(association_security.ies.as_bytes())
            }
            None => self
                .peer_pmk(peer)?
                .bind_association_security_ies(association_security.ies.as_bytes()),
        });
        let access_point = self.address;
        let inactive_timeout = self.inactive_timeout.duration();
        let existing = self.checked_peer_mut(peer)?;
        if capabilities.maximum_legacy_rate_500kbps == 0 {
            return Ok(ApMlmeAction::AssociationResponse {
                peer,
                status: AP_STATUS_UNSUPPORTED_RATES,
                association_id: None,
            });
        }
        let wpa2 = RsnApState::new(
            akm,
            access_point,
            peer,
            authenticator_nonce,
            initial_replay_counter,
        )?;
        existing.phase = ApPeerPhase::Securing;
        existing.wpa2 = Some(wpa2);
        if let Some((pmk, pmkid)) = resumed {
            existing.pmk = Some(pmk);
            existing.pmkid = Some(pmkid);
        }
        existing.association_security_binding = association_security_binding;
        existing.maximum_legacy_rate_500kbps = capabilities.maximum_legacy_rate_500kbps;
        existing.short_preamble = capabilities.short_preamble;
        existing.ht = capabilities.ht;
        existing.qos_supported = capabilities.qos_supported;
        existing.last_activity = now;
        existing.deadline = now.saturating_add(inactive_timeout);
        let association_id = existing.association_id;
        self.revise_status();
        Ok(ApMlmeAction::AssociationResponse {
            peer,
            status: AP_STATUS_SUCCESS,
            association_id: Some(association_id),
        })
    }

    /// The PMK a WPA3 association of `peer` uses: of the listed PMKIDs, the
    /// first naming its current SAE association or a cached one, or its SAE
    /// association when it lists none.
    fn resume_sae_pmk(
        &self,
        peer: [u8; 6],
        mut pmkids: impl Iterator<Item = [u8; AP_PMKID_LEN]>,
    ) -> Result<Option<(Pmk, [u8; AP_PMKID_LEN])>, ApServiceError> {
        if self.security_policy() != ApSecurityPolicy::Wpa3Personal {
            return Err(ApServiceError::SecurityModeMismatch);
        }
        let pmksa = &self.storage().pmksa;
        let existing = self.checked_peer(peer)?;
        let mut listed = false;
        let found = pmkids.find_map(|pmkid| {
            listed = true;
            if existing.pmkid == Some(pmkid) {
                existing.pmk.as_ref().map(|pmk| (pmk.duplicate(), pmkid))
            } else {
                pmksa
                    .find(peer, pmkid)
                    .map(|cached| (cached.pmk().duplicate(), pmkid))
            }
        });
        if found.is_some() || listed {
            return Ok(found);
        }
        Ok(existing
            .pmk
            .as_ref()
            .zip(existing.pmkid)
            .map(|(pmk, pmkid)| (pmk.duplicate(), pmkid)))
    }

    /// Admit an association into an explicitly Open AP epoch.
    ///
    /// An empty RSN body is the exact contract: a mixed or WPA-capable
    /// request is not silently downgraded. Authorization is immediate and no
    /// authenticator, PTK, GTK or hardware key owner is created.
    pub fn associate_open(
        &mut self,
        peer: [u8; 6],
        security: ApAssociationSecurityObservation<'_>,
        capabilities: ApAssociationCapabilities,
        now: oer_time::Instant,
    ) -> Result<ApMlmeAction, ApServiceError> {
        if self.link_protection() != LinkProtection::Open {
            return Err(ApServiceError::SecurityModeMismatch);
        }
        let security_matches = self.matches_association_security(security);
        let inactive_timeout = self.inactive_timeout.duration();
        let existing = self.checked_peer_mut(peer)?;
        if existing.phase != ApPeerPhase::Authenticated {
            return Err(ApServiceError::WrongPeerPhase);
        }
        if !security_matches {
            return Ok(ApMlmeAction::AssociationResponse {
                peer,
                status: AP_STATUS_INVALID_RSN,
                association_id: None,
            });
        }
        if capabilities.maximum_legacy_rate_500kbps == 0 {
            return Ok(ApMlmeAction::AssociationResponse {
                peer,
                status: AP_STATUS_UNSUPPORTED_RATES,
                association_id: None,
            });
        }
        existing.phase = ApPeerPhase::Authorized;
        existing.wpa2 = None;
        existing.association_security_binding = None;
        existing.pending_ptk = None;
        existing.maximum_legacy_rate_500kbps = capabilities.maximum_legacy_rate_500kbps;
        existing.short_preamble = capabilities.short_preamble;
        existing.ht = capabilities.ht;
        // Ordinary Open MSDUs retain the non-QoS sequence space. The bounded
        // A-MSDU owner uses this peer's independent QoS/TID-0 counter only
        // after validating HT and QoS support for both coalesced leases.
        existing.qos_supported = capabilities.qos_supported;
        existing.tx_block_ack.stop_all();
        existing.last_activity = now;
        existing.deadline = now.saturating_add(inactive_timeout);
        let association_id = existing.association_id;
        self.revise_status();
        Ok(ApMlmeAction::AssociationResponse {
            peer,
            status: AP_STATUS_SUCCESS,
            association_id: Some(association_id),
        })
    }

    pub fn observe_activity(
        &mut self,
        peer: [u8; 6],
        now: oer_time::Instant,
    ) -> Result<(), ApServiceError> {
        let binding = self.bind_peer(peer).ok_or(ApServiceError::UnknownPeer)?;
        self.observe_bound_activity(binding, now)
    }

    /// Refresh activity through a generation-bound O(1) peer identity.
    ///
    /// The RX data path resolves a transmitter once and reuses this capability
    /// across an in-order burst. Slot reuse invalidates the binding before any
    /// replacement peer can inherit the previous activity deadline.
    pub fn observe_bound_activity(
        &mut self,
        binding: ApPeerBinding,
        now: oer_time::Instant,
    ) -> Result<(), ApServiceError> {
        let inactive_timeout = self.inactive_timeout.duration();
        let existing = self
            .bound_peer_mut(binding)
            .ok_or(ApServiceError::UnknownPeer)?;
        if !matches!(
            existing.phase,
            ApPeerPhase::Securing | ApPeerPhase::Authorized
        ) {
            return Err(ApServiceError::WrongPeerPhase);
        }
        existing.last_activity = now;
        existing.deadline = now.saturating_add(inactive_timeout);
        Ok(())
    }

    /// Coalesced equivalent of [`Self::observe_bound_activity`] for admitted
    /// data frames whose only role is keeping an already-associated peer live.
    pub fn observe_bound_data_activity(
        &mut self,
        binding: ApPeerBinding,
        now: oer_time::Instant,
    ) -> Result<(), ApServiceError> {
        let inactive_timeout = self.inactive_timeout.duration();
        let refresh_margin = oer_time::Duration::from_micros(inactive_timeout.as_micros() / 2);
        let existing = self
            .bound_peer_mut(binding)
            .ok_or(ApServiceError::UnknownPeer)?;
        if !matches!(
            existing.phase,
            ApPeerPhase::Securing | ApPeerPhase::Authorized
        ) {
            return Err(ApServiceError::WrongPeerPhase);
        }
        if existing.deadline <= now.saturating_add(refresh_margin) {
            existing.last_activity = now;
            existing.deadline = now.saturating_add(inactive_timeout);
        }
        Ok(())
    }

    pub fn next_peer_deadline(&self) -> Option<oer_time::Instant> {
        self.storage()
            .peers
            .iter()
            .flatten()
            .filter(|peer| peer.phase != ApPeerPhase::Closing)
            .map(|peer| peer.deadline)
            .min()
    }

    pub fn begin_due_peer_close(&mut self, now: oer_time::Instant) -> Option<ApPeerClose> {
        let index = self.storage().peers.iter().position(|peer| {
            peer.as_ref()
                .is_some_and(|peer| peer.phase != ApPeerPhase::Closing && peer.deadline <= now)
        })?;
        let peer = self.storage_mut().peers[index].as_mut()?;
        let was_associated = matches!(peer.phase, ApPeerPhase::Securing | ApPeerPhase::Authorized);
        let close = ApPeerClose {
            peer: peer.address,
            kind: if was_associated {
                ApPeerCloseKind::InactivityTimeout
            } else {
                ApPeerCloseKind::AuthenticationTimeout
            },
            was_associated,
            maximum_legacy_rate_500kbps: peer.maximum_legacy_rate_500kbps,
        };
        peer.phase = ApPeerPhase::Closing;
        self.revise_status();
        Some(close)
    }

    pub fn begin_wpa2_failure_close(
        &mut self,
        peer_address: [u8; 6],
    ) -> Result<ApPeerClose, ApServiceError> {
        let peer = self.checked_peer_mut(peer_address)?;
        if peer.phase != ApPeerPhase::Securing {
            return Err(ApServiceError::WrongPeerPhase);
        }
        peer.wpa2_retry.cancel();
        peer.wpa2_retry_alarm = None;
        let close = ApPeerClose {
            peer: peer.address,
            kind: ApPeerCloseKind::Wpa2HandshakeFailure,
            was_associated: true,
            maximum_legacy_rate_500kbps: peer.maximum_legacy_rate_500kbps,
        };
        peer.phase = ApPeerPhase::Closing;
        self.revise_status();
        Ok(close)
    }

    pub fn begin_stop_peer(&mut self) -> Option<ApPeerClose> {
        let index = self.storage().peers.iter().position(|peer| {
            peer.as_ref()
                .is_some_and(|peer| peer.phase != ApPeerPhase::Closing)
        })?;
        let peer = self.storage_mut().peers[index].as_mut()?;
        let was_associated = matches!(peer.phase, ApPeerPhase::Securing | ApPeerPhase::Authorized);
        let close = ApPeerClose {
            peer: peer.address,
            kind: ApPeerCloseKind::AccessPointStop,
            was_associated,
            maximum_legacy_rate_500kbps: peer.maximum_legacy_rate_500kbps,
        };
        peer.phase = ApPeerPhase::Closing;
        self.revise_status();
        Some(close)
    }

    pub fn remove_peer(&mut self, peer: [u8; 6]) -> Result<ApMlmeAction, ApServiceError> {
        let index = self.peer_index(peer).ok_or(ApServiceError::UnknownPeer)?;
        self.storage_mut().peers[index] = None;
        self.storage_mut().management_replay[index] = management::ApManagementReplay::inactive();
        self.advance_peer_generation();
        self.revise_status();
        Ok(ApMlmeAction::PeerRemoved { peer })
    }
}
