//! Peer-bound data encoding and transmit sequence/key commit boundaries.
//! The engine retains the single service, key and beacon owners.

use super::*;

impl<'storage> ApEngine<'storage> {
    /// Prepare the AP-originated TID-0 ADDBA request for a newly authorized
    /// HT peer: one offer, at once and never repeated, as the vendor access
    /// point. The peer table owns both the negotiation and its deadline.
    pub fn prepare_tx_block_ack_request(
        &mut self,
        peer: [u8; 6],
        now: oer_time::Instant,
        output: &mut [u8],
    ) -> Result<Option<usize>, ApEngineError> {
        self.service.queue_tx_block_ack(
            peer,
            TxBlockAckRetry {
                attempts: 1,
                interval: oer_time::Duration::ZERO,
            },
        )?;
        let Some(request) = self.service.take_tx_block_ack_offer(peer, now)? else {
            return Ok(None);
        };
        let sequence = self.service.next_management_sequence();
        let length = ApManagementFrame {
            subtype: ApManagementSubtype::Action,
            access_point: self.service.address(),
            peer,
            sequence_number: sequence,
            body: &request.body,
        }
        .encode(output)?;
        #[cfg(any(feature = "diagnostics", test))]
        self.observe(ApEngineObservationEvent::TxBlockAckRequestPrepared);
        Ok(Some(length))
    }

    pub fn tx_block_ack_agreement(&self, peer: [u8; 6]) -> Option<OperationalTxBlockAck> {
        self.service.peer_status(peer)?.tx_block_ack
    }

    pub fn bind_aggregate_peer(
        &self,
        peer: [u8; 6],
    ) -> Result<(ApAggregateBinding, ApPeerStatus), ApEngineError> {
        let service_binding = self
            .service
            .bind_peer(peer)
            .ok_or(ApServiceError::UnknownPeer)?;
        let status = self
            .service
            .bound_peer_status(service_binding)
            .ok_or(ApServiceError::UnknownPeer)?;
        let security = self.security.bind_pairwise(peer, status.association_id)?;
        Ok((
            ApAggregateBinding {
                peer: service_binding,
                security,
            },
            status,
        ))
    }

    /// Whether `agreement` is still the peer's operational TX Block Ack
    /// agreement: the same association and an unchanged session generation.
    pub fn tx_block_ack_holds(&self, agreement: crate::ampdu::ApAggregateAgreement) -> bool {
        self.service
            .peer_status(agreement.association.address())
            .is_some_and(|status| {
                status.association_identity() == agreement.association
                    && status.tx_block_ack.is_some()
                    && status.tx_block_ack_generation == agreement.block_ack_generation
            })
    }

    pub fn has_operational_tx_block_ack(&self) -> bool {
        self.service.has_operational_tx_block_ack()
    }

    pub fn operational_tx_block_ack_window(&self, peer: [u8; 6]) -> Option<u16> {
        self.service.operational_tx_block_ack_window(peer)
    }

    /// End a TX Block Ack negotiation whose response is overdue at `now`;
    /// `true` when one was.
    pub fn expire_tx_block_ack(&mut self, now: oer_time::Instant) -> bool {
        let expired = self.service.expire_tx_block_ack(now).is_some();
        if expired {
            #[cfg(any(feature = "diagnostics", test))]
            self.observe(ApEngineObservationEvent::TxBlockAckNegotiationTimeout);
        }
        expired
    }

    /// The deadline of the earliest TX Block Ack negotiation's response.
    pub fn next_tx_block_ack_deadline(&self) -> Option<oer_time::Instant> {
        self.service.next_tx_block_ack_deadline()
    }

    /// Encode one authenticator EAPOL action as an unprotected AP data MPDU.
    /// The sequence number is consumed only when the complete frame fits.
    pub fn encode_eapol<const N: usize>(
        &mut self,
        peer: [u8; 6],
        frame: &RsnTxFrame<N>,
        output: &mut [u8],
    ) -> Result<usize, ApEngineError> {
        let sequence_number = self.service.current_data_sequence();
        let len = ApDataFrame {
            access_point: self.service.address(),
            destination: peer,
            sequence_number,
            ether_type: 0x888e,
            payload: frame.as_bytes(),
        }
        .encode(output)?;
        // Consume protocol state in ordinary code. Keeping this call inside
        // `debug_assert_eq!` made release builds transmit every AP data MPDU
        // with sequence number zero because the assertion expression is
        // compiled out.
        let consumed_sequence_number = self.service.next_data_sequence();
        debug_assert_eq!(consumed_sequence_number, sequence_number);
        Ok(len)
    }

    /// Capacity-admit one downlink and explicitly retain the More Data bit.
    ///
    /// Open yields a plaintext non-QoS MPDU and no key selector; WPA2 encodes
    /// a zero CCMP placeholder. Neither path advances sequence or PN until
    /// the AP MAC admits every ordinary retry rate and commits the token.
    pub(crate) fn encode_protected_ethernet_with_more_data(
        &self,
        destination: [u8; 6],
        ethernet: &[u8],
        output: &mut [u8],
        more_data: bool,
    ) -> Result<ApPreparedDataFrame, ApEngineError> {
        if self.service.link_protection() == LinkProtection::Open {
            let group = destination[0] & 1 != 0;
            if group {
                if self.service.authorized_count() == 0 {
                    return Err(ApServiceError::WrongPeerPhase.into());
                }
            } else if self.service.peer_status(destination).is_none() {
                return Err(ApServiceError::UnknownPeer.into());
            } else if !self.service.is_authorized(destination) {
                return Err(ApServiceError::WrongPeerPhase.into());
            }
            let sequence_number = self.service.current_data_sequence();
            let length = ApUnprotectedDataFrame {
                access_point: self.service.address(),
                peer: destination,
                sequence_number,
                more_data,
                ethernet,
            }
            .encode(output)?;
            return Ok(ApPreparedDataFrame {
                length,
                peer: destination,
                sequence_number,
                sequence_space: ApDataSequenceSpace::NonQos,
                hardware_key_selector: None,
            });
        }
        let group = destination[0] & 1 != 0;
        let (hardware_key_selector, peer_qos) = if group {
            if self.service.authorized_count() == 0 {
                return Err(ApServiceError::WrongPeerPhase.into());
            }
            (self.security.group_hardware_index()?, false)
        } else {
            if self.service.peer_status(destination).is_none() {
                return Err(ApServiceError::UnknownPeer.into());
            }
            if !self.service.is_authorized(destination) {
                return Err(ApServiceError::WrongPeerPhase.into());
            }
            let status = self
                .service
                .peer_status(destination)
                .ok_or(ApServiceError::UnknownPeer)?;
            (
                self.security.pairwise_hardware_index(destination)?,
                status.qos_supported,
            )
        };
        let sequence_number = if peer_qos {
            self.service
                .current_qos_sequence(destination, oer_ieee80211_ap::AP_TX_BLOCK_ACK_TID)
                .expect("AP TX data TID is representable")
        } else {
            self.service.current_data_sequence()
        };
        let length = ApProtectedDataFrame {
            access_point: self.service.address(),
            peer: destination,
            sequence_number,
            user_priority: 0,
            peer_qos,
            more_data,
            // Complete geometry, peer, QoS and output admission before the
            // monotonic pairwise/group PN owner advances.
            ccmp_header: [0; 8],
            ethernet,
        }
        .encode(output)?;
        Ok(ApPreparedDataFrame {
            length,
            peer: destination,
            sequence_number,
            sequence_space: if peer_qos {
                ApDataSequenceSpace::Qos
            } else {
                ApDataSequenceSpace::NonQos
            },
            hardware_key_selector: Some(hardware_key_selector),
        })
    }

    /// Commit one ordinary AP frame only after protection admission.
    pub(crate) fn commit_prepared_data(
        &mut self,
        prepared: ApPreparedDataFrame,
        output: &mut [u8],
    ) -> Result<ApProtectedFrame, ApEngineError> {
        if let Some(selector) = prepared.hardware_key_selector {
            let group = prepared.peer[0] & 1 != 0;
            let ccmp_header = if group {
                if self.security.group_hardware_index()? != selector {
                    return Err(ApSecurityError::WrongPeer.into());
                }
                self.security.next_group_tx_ccmp_header()?
            } else {
                if self.security.pairwise_hardware_index(prepared.peer)? != selector {
                    return Err(ApSecurityError::WrongPeer.into());
                }
                self.security.next_pairwise_tx_ccmp_header(prepared.peer)?
            };
            let ccmp_offset = match prepared.sequence_space {
                ApDataSequenceSpace::Qos => IEEE80211_QOS_DATA_HEADER_LEN,
                ApDataSequenceSpace::NonQos => IEEE80211_LEGACY_DATA_HEADER_LEN,
            };
            output[ccmp_offset..ccmp_offset + ccmp_header.len()].copy_from_slice(&ccmp_header);
        }
        let consumed_sequence_number = match prepared.sequence_space {
            ApDataSequenceSpace::Qos => self
                .service
                .next_qos_sequence(prepared.peer, oer_ieee80211_ap::AP_TX_BLOCK_ACK_TID)
                .expect("AP TX data TID is representable"),
            ApDataSequenceSpace::NonQos => self.service.next_data_sequence(),
        };
        debug_assert_eq!(consumed_sequence_number, prepared.sequence_number);
        Ok(ApProtectedFrame {
            length: prepared.length,
            hardware_key_selector: prepared.hardware_key_selector,
        })
    }

    /// Encode exactly two ordered AP network leases as one prepared QoS
    /// A-MSDU without consuming sequence or CCMP ownership.
    ///
    /// `Ok(None)` is a non-mutating admission miss: the caller may transmit
    /// the first lease normally and retain the second without reordering. A
    /// WPA2 epoch additionally requires the exact TID-0 BlockAck agreement to
    /// have negotiated A-MSDU support. Open HT peers use ordinary ACK policy.
    /// Only the AP MAC may turn the returned value into a committed frame,
    /// after its ordinary initial/retry rate series passes protection policy.
    pub(crate) fn encode_amsdu_ethernet_pair(
        &self,
        first: &[u8],
        second: &[u8],
        output: &mut [u8],
    ) -> Result<Option<ApPreparedAmsduFrame>, ApEngineError> {
        let Some(destination) = first
            .get(..6)
            .and_then(|bytes| <[u8; 6]>::try_from(bytes).ok())
        else {
            return Ok(None);
        };
        if destination[0] & 1 != 0
            || second.get(..6) != Some(destination.as_slice())
            || first.len() < 14
            || second.len() < 14
        {
            return Ok(None);
        }
        let Some(status) = self.service.peer_status(destination) else {
            return Ok(None);
        };
        if status.phase != ApPeerPhase::Authorized || !status.qos_supported || status.ht.is_none() {
            return Ok(None);
        }
        let protected = self.service.link_protection() != LinkProtection::Open;
        if protected
            && !status.tx_block_ack.is_some_and(|agreement| {
                agreement.tid == oer_ieee80211_ap::AP_TX_BLOCK_ACK_TID && agreement.amsdu
            })
        {
            return Ok(None);
        }

        let sequence_number = self
            .service
            .current_qos_sequence(destination, oer_ieee80211_ap::AP_TX_BLOCK_ACK_TID)
            .expect("AP A-MSDU TID is representable");
        let ethernet_frames = [first, second];
        let placeholder_ccmp = protected.then_some([0; 8]);
        let length = match (ApAmsduFrame {
            access_point: self.service.address(),
            peer: destination,
            sequence_number,
            user_priority: oer_ieee80211_ap::AP_TX_BLOCK_ACK_TID,
            more_data: false,
            ccmp_header: placeholder_ccmp,
            ethernet_frames: &ethernet_frames,
        })
        .encode(output)
        {
            Ok(length) => length,
            Err(
                ApDataFrameError::OutputTooSmall { .. }
                | ApDataFrameError::AmsduTooLong { .. }
                | ApDataFrameError::EthernetFrameTooShort,
            ) => return Ok(None),
            Err(error) => return Err(error.into()),
        };

        let hardware_key_selector = if protected {
            Some(self.security.pairwise_hardware_index(destination)?)
        } else {
            None
        };
        Ok(Some(ApPreparedAmsduFrame {
            length,
            peer: destination,
            sequence_number,
            hardware_key_selector,
        }))
    }

    /// Commit the monotonic security/sequence state of one prepared A-MSDU.
    /// The caller must complete TX-protection admission before this edge.
    pub(crate) fn commit_prepared_amsdu(
        &mut self,
        prepared: ApPreparedAmsduFrame,
        output: &mut [u8],
    ) -> Result<ApProtectedFrame, ApEngineError> {
        if let Some(selector) = prepared.hardware_key_selector {
            if self.security.pairwise_hardware_index(prepared.peer)? != selector {
                return Err(ApSecurityError::WrongPeer.into());
            }
            let ccmp_header = self.security.next_pairwise_tx_ccmp_header(prepared.peer)?;
            output
                [IEEE80211_QOS_DATA_HEADER_LEN..IEEE80211_QOS_DATA_HEADER_LEN + ccmp_header.len()]
                .copy_from_slice(&ccmp_header);
        }
        let consumed = self
            .service
            .next_qos_sequence(prepared.peer, oer_ieee80211_ap::AP_TX_BLOCK_ACK_TID)
            .expect("AP A-MSDU TID is representable");
        debug_assert_eq!(consumed, prepared.sequence_number);
        Ok(ApProtectedFrame {
            length: prepared.length,
            hardware_key_selector: prepared.hardware_key_selector,
        })
    }

    /// AP-specific adapter from a network allocation to the role-neutral
    /// retained A-MPDU backing contract.
    ///
    /// Saturated AP TX executes this leaf once per MPDU (roughly 150,000
    /// calls per 16-second BA16 HIL interval). The PSRAM-code profile keeps
    /// only this measured synchronous encoder leaf in the semantic hot-text
    /// class; the linker, not the portable AP model, selects physical SRAM.
    #[cfg_attr(
        target_arch = "riscv32",
        unsafe(link_section = ".hot.text.open_radio_ap_tx_encode")
    )]
    #[inline(never)]
    pub fn encode_aggregate_ethernet_in_place(
        &mut self,
        binding: ApAggregateBinding,
        storage: &mut [u8],
        ethernet_offset: usize,
        ethernet_length: usize,
    ) -> Result<ApAggregateFrame, ApEngineError> {
        let status = self
            .service
            .bound_peer_status(binding.peer)
            .ok_or(ApServiceError::UnknownPeer)?;
        if status.phase != ApPeerPhase::Authorized
            || !status.qos_supported
            || status.tx_block_ack.is_none()
        {
            return Err(ApServiceError::WrongPeerPhase.into());
        }
        let peer = binding.peer();
        let hardware_key_selector = binding.security.hardware_index();
        let sequence_number = self
            .service
            .current_qos_sequence(peer, oer_ieee80211_ap::AP_TX_BLOCK_ACK_TID)
            .expect("AP TX data TID is representable");
        let encoded = ApProtectedDataFrame {
            access_point: self.service.address(),
            peer,
            sequence_number,
            user_priority: oer_ieee80211_ap::AP_TX_BLOCK_ACK_TID,
            peer_qos: true,
            more_data: false,
            ccmp_header: [0; 8],
            ethernet: &[],
        }
        .encode_in_place(storage, ethernet_offset, ethernet_length)?;
        let ccmp_header = self
            .security
            .next_bound_pairwise_tx_ccmp_header(binding.security)?;
        let ccmp_offset = encoded.offset + IEEE80211_QOS_DATA_HEADER_LEN;
        storage[ccmp_offset..ccmp_offset + ccmp_header.len()].copy_from_slice(&ccmp_header);
        let consumed = self
            .service
            .next_qos_sequence(peer, oer_ieee80211_ap::AP_TX_BLOCK_ACK_TID)
            .expect("AP TX data TID is representable");
        debug_assert_eq!(consumed, sequence_number);
        Ok(ApAggregateFrame {
            encoded,
            hardware_key_selector,
            sequence_number,
        })
    }
}
