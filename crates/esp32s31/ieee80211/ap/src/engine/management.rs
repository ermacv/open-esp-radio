//! AP management dispatch, association responses and handshake initiation.
//! The engine retains the single service, key and beacon owners.

use super::*;

impl<'storage> ApEngine<'storage> {
    /// Build handshake message one after the successful Association Response
    /// has reached TX completion.
    pub fn begin_wpa2<const N: usize>(
        &self,
        peer: [u8; 6],
    ) -> Result<RsnTxFrame<N>, ApEngineError> {
        Ok(self.service.begin_wpa2_frame(peer)?)
    }

    /// Encode one AP recipient response to a peer-originated BlockAck action.
    /// Agreement state, hardware publication and TX completion remain owned
    /// by the caller; the engine owns only AP addressing and sequence space.
    pub fn encode_rx_block_ack_response(
        &mut self,
        peer: [u8; 6],
        body: &[u8],
        output: &mut [u8],
    ) -> Result<usize, ApEngineError> {
        let sequence = self.service.next_management_sequence();
        Ok(ApManagementFrame {
            subtype: ApManagementSubtype::Action,
            access_point: self.service.address(),
            peer,
            sequence_number: sequence,
            body,
        }
        .encode(output)?)
    }

    /// Record `peer`'s accepted SAE exchange, ending the pairwise-key epoch
    /// of an earlier association as a new Open System authentication does.
    /// Returns the status its slot allows.
    pub fn accept_sae<H: ApRuntimeHardware>(
        &mut self,
        hardware: &mut H,
        peer: [u8; 6],
        pmk: Pmk,
        pmkid: [u8; AP_PMKID_LEN],
        now: oer_time::Instant,
    ) -> Result<u16, ApEngineError> {
        if self.service.peer_status(peer).is_some() {
            self.security.clear_peer(hardware, peer)?;
        }
        Ok(self.service.authenticate_sae(peer, pmk, pmkid, now)?)
    }

    /// Encode one reply of the SAE responder.
    pub fn encode_sae_reply(
        &mut self,
        reply: &ApSaeReply,
        output: &mut [u8],
    ) -> Result<usize, ApEngineError> {
        let sequence = self.service.next_management_sequence();
        Ok(write_sae_authentication(
            output,
            self.service.address(),
            reply.peer,
            reply.transaction,
            reply.status,
            reply.body(),
            sequence,
        )?)
    }

    pub fn handle_management<H: ApRuntimeHardware>(
        &mut self,
        hardware: &mut H,
        frame: &[u8],
        authenticator_nonce: [u8; 32],
        initial_replay_counter: u64,
        now: oer_time::Instant,
        output: &mut [u8],
    ) -> Result<ApManagementOutcome, ApEngineError> {
        // This direct path has no PMF integrity evidence. Protected requests
        // are handled by the port service, never by this plaintext parser.
        if frame.get(1).is_some_and(|flags| flags & 0x40 != 0) {
            return Ok(ApManagementOutcome::Ignored);
        }
        let Some(request) = parse_ap_management_request(
            &crate::profile::ADVERTISEMENT,
            frame,
            self.service.address(),
        ) else {
            return Ok(ApManagementOutcome::Ignored);
        };
        let retry = frame.get(1).is_some_and(|byte| byte & 0x08 != 0);
        match request {
            ApManagementRequest::Probe { peer, ssid } => {
                if !oer_ieee80211_mac::ap::probe::matches_ssid(self.beacon.advertisement(), ssid) {
                    return Ok(ApManagementOutcome::Ignored);
                }
                // Global admission budget: changing the sender MAC cannot create
                // additional TX credit. Discard excess requests; never queue them
                // or arm a timer to transmit stale discovery responses later.
                if now < self.next_probe_response {
                    return Ok(ApManagementOutcome::Ignored);
                }
                self.advertise_current_protection()?;
                let sequence = self.service.next_management_sequence();
                let len = oer_ieee80211_mac::ap::probe::write_response(
                    self.beacon.advertisement(),
                    peer,
                    sequence,
                    now.as_micros(),
                    output,
                )
                .map_err(ApEngineError::Probe)?;
                self.next_probe_response = now.saturating_add(oer_time::Duration::from_millis(10));
                Ok(ApManagementOutcome::Response {
                    len,
                    begin_wpa2: false,
                })
            }

            ApManagementRequest::OpenAuthentication { peer } => {
                if retry
                    && self
                        .service
                        .peer_status(peer)
                        .is_some_and(|status| status.phase != ApPeerPhase::Authenticated)
                {
                    // libnet80211 routes retry/duplicate management frames as
                    // ordinary receive outcomes; it does not tear down its
                    // station node merely because an earlier response ACK was
                    // lost. Re-emit success while preserving the current WPA2
                    // or authorized key epoch. A non-retry authentication
                    // below still owns the explicit reauthentication reset.
                    let sequence = self.service.next_management_sequence();
                    let len = write_open_authentication_response(
                        output,
                        self.service.address(),
                        peer,
                        0,
                        sequence,
                    )?;
                    #[cfg(any(feature = "diagnostics", test))]
                    self.observe(ApEngineObservationEvent::AuthenticationResponsePrepared);
                    return Ok(ApManagementOutcome::Response {
                        len,
                        begin_wpa2: false,
                    });
                }
                // A peer may restart authentication without first sending a
                // deauthentication frame. End its old pairwise-key epoch
                // before the portable service resets the handshake state;
                // otherwise the stable AID would still own a stale hardware
                // entry and the next authorization could not install its PTK.
                if self.service.peer_status(peer).is_some() {
                    self.security.clear_peer(hardware, peer)?;
                }
                let ApMlmeAction::AuthenticationResponse { status, .. } =
                    self.service.authenticate_open(peer, now)
                else {
                    unreachable!("authenticate_open has one response action")
                };
                let sequence = self.service.next_management_sequence();
                let len = write_open_authentication_response(
                    output,
                    self.service.address(),
                    peer,
                    status,
                    sequence,
                )?;
                #[cfg(any(feature = "diagnostics", test))]
                self.observe(ApEngineObservationEvent::AuthenticationResponsePrepared);
                Ok(ApManagementOutcome::Response {
                    len,
                    begin_wpa2: false,
                })
            }
            // A BSS without SAE ignores SAE authentication as it ignores
            // every other algorithm it does not offer.
            ApManagementRequest::SaeAuthentication {
                peer,
                transaction,
                status,
                body,
            } => {
                if self.service.security_policy() != ApSecurityPolicy::Wpa3Personal
                    || body.len() > output.len()
                {
                    return Ok(ApManagementOutcome::Ignored);
                }
                output[..body.len()].copy_from_slice(body);
                Ok(ApManagementOutcome::SaeAuthentication {
                    peer,
                    transaction,
                    status,
                    len: body.len(),
                })
            }
            ApManagementRequest::Association {
                peer,
                security,
                maximum_legacy_rate_500kbps,
                ht_capabilities,
                qos_supported,
                short_preamble,
            } => {
                let Some(peer_status) = self.service.peer_status(peer) else {
                    // Vendor `hostap_recv_mgmt` treats a peer lookup miss as
                    // an on-air management outcome (including explicit
                    // deauthentication paths), never as a task failure. This
                    // bounded port has no deauthentication response for the
                    // class-2 case yet, so retain the owner and ignore it.
                    return Ok(ApManagementOutcome::Ignored);
                };
                if self.service.matches_association_security(security)
                    && (peer_status.phase == ApPeerPhase::Securing
                        || (self.service.link_protection() == LinkProtection::Open
                            && peer_status.phase == ApPeerPhase::Authorized))
                {
                    // A station can repeat Association Request when the first
                    // response ACK was lost. Preserve the in-flight WPA2
                    // state, retransmit the same successful association and
                    // do not start a second Message-1 transaction.
                    let sequence = self.service.next_management_sequence();
                    let len = write_ht_association_response_frame_for_security(
                        &crate::profile::ADVERTISEMENT,
                        output,
                        self.service.address(),
                        peer,
                        0,
                        peer_status.association_id,
                        sequence,
                        oer_ieee80211_mac::channel::Channel::from_wifi_channel(self.channel),
                        peer_status.ht,
                        self.service.security_policy(),
                        self.required_protection(),
                    )?;
                    #[cfg(any(feature = "diagnostics", test))]
                    self.observe(ApEngineObservationEvent::AssociationResponsePrepared {
                        associated_peers: self.service.associated_count(),
                    });
                    return Ok(ApManagementOutcome::Response {
                        len,
                        begin_wpa2: false,
                    });
                }
                if peer_status.phase != ApPeerPhase::Authenticated {
                    return Ok(ApManagementOutcome::Ignored);
                }
                let capabilities = ApAssociationCapabilities {
                    maximum_legacy_rate_500kbps,
                    short_preamble,
                    ht: ht_capabilities,
                    qos_supported,
                };
                let action = match self.service.link_protection() {
                    LinkProtection::Open => {
                        self.service
                            .associate_open(peer, security, capabilities, now)?
                    }
                    LinkProtection::Ccmp => self.service.associate_rsn(
                        peer,
                        security,
                        capabilities,
                        authenticator_nonce,
                        initial_replay_counter,
                        now,
                    )?,
                };
                let ApMlmeAction::AssociationResponse {
                    status,
                    association_id,
                    ..
                } = action
                else {
                    unreachable!("AP association has one response action")
                };
                let sequence = self.service.next_management_sequence();
                let len = write_ht_association_response_frame_for_security(
                    &crate::profile::ADVERTISEMENT,
                    output,
                    self.service.address(),
                    peer,
                    status,
                    association_id.unwrap_or(0),
                    sequence,
                    oer_ieee80211_mac::channel::Channel::from_wifi_channel(self.channel),
                    ht_capabilities,
                    self.service.security_policy(),
                    self.required_protection(),
                )?;
                if association_id.is_some() {
                    // Recovered `ic_set_sta` evidence gives the legacy B/G
                    // path a software transmit-rate context, represented here
                    // by the peer's negotiated rate and stable AID. Its extra
                    // station-programming calls are HE-only. Pairwise hardware
                    // state is installed later through the AID-owned key slot,
                    // so the current B/G AP has no unevidenced station-table
                    // MMIO operation to imitate.
                }
                #[cfg(any(feature = "diagnostics", test))]
                self.observe(ApEngineObservationEvent::AssociationResponsePrepared {
                    associated_peers: self.service.associated_count(),
                });
                #[cfg(any(feature = "diagnostics", test))]
                if association_id.is_some()
                    && self.service.link_protection() == LinkProtection::Open
                {
                    self.observe(ApEngineObservationEvent::PeerAuthorized {
                        authorized_peers: self.service.authorized_count(),
                    });
                }
                Ok(ApManagementOutcome::Response {
                    len,
                    begin_wpa2: association_id.is_some()
                        && self.service.link_protection() == LinkProtection::Ccmp,
                })
            }
            ApManagementRequest::Disassociation { peer, .. }
            | ApManagementRequest::Deauthentication { peer, .. } => {
                let Some(peer_status) = self.service.peer_status(peer) else {
                    return Ok(ApManagementOutcome::Ignored);
                };
                // Once local timeout/stop teardown owns the peer, its ordered
                // disassociation -> deauthentication -> key clear transaction
                // is authoritative. A peer response racing that transaction
                // must not remove the state below the in-flight TX owner.
                if peer_status.phase == ApPeerPhase::Closing {
                    return Ok(ApManagementOutcome::Ignored);
                }
                self.security.clear_peer(hardware, peer)?;
                self.service.remove_peer(peer)?;
                #[cfg(any(feature = "diagnostics", test))]
                self.observe(ApEngineObservationEvent::PeerRemoved);
                Ok(ApManagementOutcome::PeerRemoved { peer })
            }
            // The direct S31 AP path does not serve PMF. Its port service
            // owns the protected SA Query response path.
            ApManagementRequest::SaQuery { .. } => Ok(ApManagementOutcome::Ignored),
            ApManagementRequest::BlockAck { peer, action } => {
                if self.service.peer_status(peer).is_none() {
                    return Ok(ApManagementOutcome::Ignored);
                }
                if let Some(response) = self.service.on_tx_block_ack_action(peer, action)? {
                    #[cfg(any(feature = "diagnostics", test))]
                    self.observe(ApEngineObservationEvent::TxBlockAckResponseObserved);
                    match response {
                        TxBlockAckResponse::Operational(_) => {
                            #[cfg(any(feature = "diagnostics", test))]
                            self.observe(ApEngineObservationEvent::TxBlockAckOperational);
                        }
                        TxBlockAckResponse::Rejected(_) => {
                            #[cfg(any(feature = "diagnostics", test))]
                            self.observe(ApEngineObservationEvent::TxBlockAckRejected);
                        }
                    }
                }
                Ok(ApManagementOutcome::Ignored)
            }
        }
    }
}
