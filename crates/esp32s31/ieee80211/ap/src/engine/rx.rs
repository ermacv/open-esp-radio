//! Controlled-port admission, pairwise replay and RX peer binding.
//! The engine retains the single service, key and beacon owners.

use super::*;

impl<'storage> ApEngine<'storage> {
    /// Admit one AP data MPDU against the live controlled port and exact PTK
    /// generation, committing its CCMP PN before Ethernet publication.
    ///
    /// The RX dispatcher calls this only after software BlockAck release and
    /// hardware MIC verification. Keeping replay state beside the installed
    /// key makes PTK install, clear and reinstall the only reset edges.
    pub fn admit_rx_data(&mut self, request: ApRxAdmissionRequest) -> ApRxAdmission {
        let peer = request.peer();
        let rx_peer = match self.resolve_rx_peer(peer) {
            Ok(bound) => bound,
            Err(admission) => return admission,
        };
        if matches!(request.lane(), CcmpReplayLane::Tid(_)) && !rx_peer.qos_supported {
            return ApRxAdmission::rejected(ApRxError::PeerQosMismatch);
        }
        // The exact duplicate slot and association/key generations were
        // captured with this control-plane revision. AID reuse invalidates the
        // RX binding before another frame can inherit the old history.
        let duplicate_owner = rx_peer.duplicate_owner;
        match request.operation() {
            ApRxAdmissionOperation::Ordinary => {
                match (self.service.link_protection(), request.ccmp_header()) {
                    (LinkProtection::Open, None) => ApRxAdmission::authorized(duplicate_owner),
                    (LinkProtection::Ccmp, Some(header)) => {
                        if header.key_id() != CcmpKeyId::PAIRWISE {
                            return ApRxAdmission::rejected(ApRxError::PairwiseKeyId(
                                header.key_id().value(),
                            ));
                        }
                        let Some(binding) = rx_peer.pairwise else {
                            return ApRxAdmission::rejected(ApRxError::KeyGenerationMismatch);
                        };
                        match self.security.commit_bound_pairwise_rx_immediate(
                            binding,
                            request.lane(),
                            header.packet_number(),
                        ) {
                            Ok(()) => ApRxAdmission::authorized(duplicate_owner),
                            Err(error) => rejected_ordinary_rx_security(duplicate_owner, error),
                        }
                    }
                    _ => ApRxAdmission::rejected(ApRxError::SecurityModeMismatch),
                }
            }
            ApRxAdmissionOperation::AuthorizeFragment | ApRxAdmissionOperation::PrepareFragment => {
                let Some(header) = request.ccmp_header() else {
                    return ApRxAdmission::rejected(ApRxError::SecurityModeMismatch);
                };
                if self.service.link_protection() != LinkProtection::Ccmp {
                    return ApRxAdmission::rejected(ApRxError::SecurityModeMismatch);
                }
                if header.key_id() != CcmpKeyId::PAIRWISE {
                    return ApRxAdmission::rejected(ApRxError::PairwiseKeyId(
                        header.key_id().value(),
                    ));
                }
                let Some(binding) = rx_peer.pairwise else {
                    return ApRxAdmission::rejected(ApRxError::KeyGenerationMismatch);
                };
                if matches!(
                    request.operation(),
                    ApRxAdmissionOperation::AuthorizeFragment
                ) {
                    return ApRxAdmission::authorized(duplicate_owner);
                }
                let candidate = match self.security.prepare_bound_pairwise_rx(
                    binding,
                    request.lane(),
                    header.packet_number(),
                ) {
                    Ok(candidate) => candidate,
                    Err(error) => return rejected_rx_security(error),
                };
                ApRxAdmission::prepared(ApRxPreparedReplay {
                    peer,
                    lane: request.lane(),
                    ccmp_header: header,
                    owner: duplicate_owner,
                    candidate: ApRxPreparedCandidate::Hardware(candidate),
                })
            }
            ApRxAdmissionOperation::CommitFragment(prepared) => {
                if self.service.link_protection() != LinkProtection::Ccmp
                    || prepared.peer != peer
                    || prepared.lane != request.lane()
                    || Some(prepared.ccmp_header) != request.ccmp_header()
                {
                    return ApRxAdmission::rejected(ApRxError::SecurityModeMismatch);
                }
                let Some(_binding) = rx_peer.pairwise else {
                    return ApRxAdmission::rejected(ApRxError::KeyGenerationMismatch);
                };
                if duplicate_owner != prepared.owner {
                    return ApRxAdmission::rejected(ApRxError::KeyGenerationMismatch);
                }
                #[cfg(not(test))]
                let ApRxPreparedCandidate::Hardware(candidate) = prepared.candidate;
                #[cfg(test)]
                let candidate = match prepared.candidate {
                    ApRxPreparedCandidate::Hardware(candidate) => candidate,
                    ApRxPreparedCandidate::Model => {
                        return ApRxAdmission::rejected(ApRxError::KeyGenerationMismatch);
                    }
                };
                match self.security.commit_bound_pairwise_rx(candidate) {
                    Ok(()) => ApRxAdmission::authorized(duplicate_owner),
                    Err(error) => rejected_rx_security(error),
                }
            }
        }
    }

    /// Admit the compact complete WPA2 pairwise request used by the saturated
    /// AP ordinary-data leaf.
    pub fn admit_ordinary_pairwise_rx(
        &mut self,
        request: ApOrdinaryPairwiseRxRequest,
    ) -> ApRxAdmission {
        let rx_peer = match self.resolve_rx_peer(request.peer()) {
            Ok(bound) => bound,
            Err(admission) => return admission,
        };
        if matches!(request.lane(), CcmpReplayLane::Tid(_)) && !rx_peer.qos_supported {
            return ApRxAdmission::rejected(ApRxError::PeerQosMismatch);
        }
        let Some(binding) = rx_peer.pairwise else {
            return ApRxAdmission::rejected(ApRxError::KeyGenerationMismatch);
        };
        match self.security.commit_bound_pairwise_rx_immediate(
            binding,
            request.lane(),
            request.ccmp_header().packet_number(),
        ) {
            Ok(()) => ApRxAdmission::authorized(rx_peer.duplicate_owner),
            Err(error) => rejected_ordinary_rx_security(rx_peer.duplicate_owner, error),
        }
    }

    /// Admit one complete ordinary data MPDU and apply its peer activity
    /// through the exact binding resolved by the same transaction.
    ///
    /// The result keeps protocol admission independent from portable service
    /// errors: a caller may finish the already-committed replay/publication
    /// transaction and then surface the activity error at its role boundary.
    pub fn admit_ordinary_pairwise_rx_with_activity(
        &mut self,
        request: ApOrdinaryPairwiseRxRequest,
        state: ApPeerPowerState,
        now: oer_time::Instant,
    ) -> (
        ApRxAdmission,
        Result<Option<ApPowerSaveAction>, ApEngineError>,
    ) {
        let admission = self.admit_ordinary_pairwise_rx(request);
        if admission.authorized_owner().is_none() {
            return (admission, Ok(None));
        }
        let Some(binding) = self.rx_peer else {
            return (admission, Err(ApServiceError::UnknownPeer.into()));
        };
        let activity = self
            .service
            .observe_bound_data_power_state(binding.peer, state, now)
            .map(Some)
            .map_err(ApEngineError::from);
        (admission, activity)
    }

    /// Resolve one AP-specific peer/security context around the role-neutral
    /// data dispatcher. The common burst revalidates two affine bindings in
    /// O(1); only a peer switch or control-plane revision scans the bounded
    /// portable table.
    fn resolve_rx_peer(&mut self, peer: [u8; 6]) -> Result<ApRxPeerBinding, ApRxAdmission> {
        let status_revision = self.service.status_revision();
        if let Some(binding) = self.rx_peer
            && binding.peer.address() == peer
            && binding.status_revision == status_revision
        {
            return Ok(binding);
        }

        let Some(service_binding) = self.service.bind_peer(peer) else {
            self.rx_peer = None;
            return Err(ApRxAdmission::unauthorized());
        };
        let Some(status) = self
            .service
            .bound_peer_status(service_binding)
            .filter(|status| status.phase == ApPeerPhase::Authorized)
        else {
            self.rx_peer = None;
            return Err(ApRxAdmission::unauthorized());
        };
        let pairwise = match self.service.link_protection() {
            LinkProtection::Open => None,
            LinkProtection::Ccmp => Some(
                self.security
                    .bind_pairwise(peer, status.association_id)
                    .map_err(rejected_rx_security)?,
            ),
        };
        let Some(mut duplicate_owner) =
            ApRxDuplicateOwner::new(status.association_id, status.association_epoch)
        else {
            self.rx_peer = None;
            return Err(ApRxAdmission::rejected(ApRxError::KeyGenerationMismatch));
        };
        if let Some(pairwise) = pairwise {
            duplicate_owner = duplicate_owner.with_key_generation(pairwise.generation());
        }
        let binding = ApRxPeerBinding {
            peer: service_binding,
            pairwise,
            duplicate_owner,
            qos_supported: status.qos_supported,
            status_revision,
        };
        self.rx_peer = Some(binding);
        Ok(binding)
    }
}
