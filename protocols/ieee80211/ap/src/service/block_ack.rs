//! Per-peer TX Block Ack negotiation under the AP service owner.
//! Every method borrows the same service and peer storage; no second owner exists.
//!
//! Each peer's agreement is a `TxBlockAckOriginator` of its TID 0. The caller
//! queues an offer with its retry policy once the peer is authorized, takes
//! it when it is due (when it has traffic for the peer, or at once), reports
//! a request that did not leave, routes the peer's actions, and expires the
//! negotiations whose response is overdue.

use super::*;

impl<'peers, S: SppScope> AccessPointService<'peers, S> {
    /// Queue the TID-0 TX Block Ack offer to an authorized HT QoS peer of a
    /// protected BSS with `retry`'s attempts and interval; `false` for a
    /// peer the access point offers no agreement.
    pub fn queue_tx_block_ack(
        &mut self,
        peer: [u8; 6],
        retry: TxBlockAckRetry,
    ) -> Result<bool, ApServiceError> {
        if self.link_protection() == LinkProtection::Open {
            return Ok(false);
        }
        let peer = self.checked_peer_mut(peer)?;
        if peer.phase != ApPeerPhase::Authorized || peer.ht.is_none() || !peer.qos_supported {
            return Ok(false);
        }
        peer.tx_block_ack.queue_initial(retry);
        Ok(true)
    }

    /// Begin the peer's offer when one is due at `now` and no agreement or
    /// negotiation is live; the request owns its action body. Taking it uses
    /// one attempt.
    pub fn take_tx_block_ack_offer(
        &mut self,
        peer: [u8; 6],
        now: oer_time::Instant,
    ) -> Result<Option<AddbaRequest>, ApServiceError> {
        if self.link_protection() == LinkProtection::Open {
            return Ok(None);
        }
        let starting_sequence = self
            .current_qos_sequence(peer, AP_TX_BLOCK_ACK_TID)
            .expect("AP data TID is representable");
        let peer = self.checked_peer_mut(peer)?;
        if peer.phase != ApPeerPhase::Authorized
            || peer.tx_block_ack.operational(AP_TX_BLOCK_ACK_TID).is_some()
            || peer.tx_block_ack.alarm(AP_TX_BLOCK_ACK_TID).is_some()
        {
            return Ok(None);
        }
        let Some(tid) = peer.tx_block_ack.take_pending(now) else {
            return Ok(None);
        };
        Ok(Some(peer.tx_block_ack.begin(
            tid,
            starting_sequence,
            now,
        )?))
    }

    /// The peer's request did not leave at `now`: its negotiation ends and
    /// the next attempt is due after the retry interval.
    pub fn tx_block_ack_offer_failed(
        &mut self,
        peer: [u8; 6],
        now: oer_time::Instant,
    ) -> Result<(), ApServiceError> {
        self.checked_peer_mut(peer)?
            .tx_block_ack
            .transmit_failed(AP_TX_BLOCK_ACK_TID, now);
        Ok(())
    }

    pub fn on_tx_block_ack_action(
        &mut self,
        peer: [u8; 6],
        action: BlockAckAction,
    ) -> Result<Option<TxBlockAckResponse>, ApServiceError> {
        if self.link_protection() == LinkProtection::Open {
            return Ok(None);
        }
        let peer = self.checked_peer_mut(peer)?;
        match action {
            BlockAckAction::AddbaResponse { .. } => {
                // A response may cross the finite negotiation timeout, a
                // DELBA or the stop teardown in either direction, and a peer
                // may repeat it after a lost ACK. Like mac80211 and the
                // station dispatcher, a token which owns no live negotiation
                // is stale: it is dropped rather than applied or turned into
                // a service fault.
                match peer.tx_block_ack.on_response_action(action)? {
                    TxBlockAckResponseDisposition::Matched(matched) => {
                        self.revise_status();
                        Ok(Some(matched.response))
                    }
                    TxBlockAckResponseDisposition::StaleDialogToken(_) => Ok(None),
                }
            }
            // This owner represents only AP-originated TX aggregation. A
            // peer-originated DELBA (`initiator = true`) terminates the
            // independent peer -> AP agreement and must not revoke our
            // AP -> peer session. The recipient clears this TX agreement
            // with `initiator = false`.
            BlockAckAction::Delba {
                tid,
                initiator: false,
                ..
            } if tid == AP_TX_BLOCK_ACK_TID => {
                peer.tx_block_ack.stop(tid);
                self.revise_status();
                Ok(None)
            }
            _ => Ok(None),
        }
    }

    /// End at most one negotiation whose response is overdue at `now`,
    /// queueing its next attempt after the retry interval while attempts
    /// remain; its peer.
    pub fn expire_tx_block_ack(&mut self, now: oer_time::Instant) -> Option<[u8; 6]> {
        self.storage_mut()
            .peers
            .iter_mut()
            .flatten()
            .find_map(|peer| peer.tx_block_ack.expire_next(now).map(|_| peer.address))
    }

    /// The earliest deadline of a negotiation's response.
    pub fn next_tx_block_ack_deadline(&self) -> Option<oer_time::Instant> {
        self.storage()
            .peers
            .iter()
            .flatten()
            .filter_map(|peer| peer.tx_block_ack.earliest_alarm_deadline())
            .min()
    }
}
