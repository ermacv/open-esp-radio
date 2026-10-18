//! Merge packet-free demand before the optional deficit selection.

use oer_ieee80211_datapath::DestinationTxQueues;

use super::{
    super::airtime::{AccessPointAirtimeError, Accounting},
    *,
};

impl Candidate {
    fn eligible_key(self, engine: &ApEngine<'_>) -> Option<ApTxFlowKey> {
        let key = match self {
            Self::Buffered(identity) => ApTxFlowKey::associated(identity),
            Self::Retained(key) => key,
            Self::Source(destination) => {
                if destination[0] & 1 != 0 {
                    ApTxFlowKey::unbound(destination)
                } else {
                    ApTxFlowKey::associated(engine.admit_downlink(destination).ok()?.identity())
                }
            }
        };
        if let Some(identity) = key.association() {
            engine.association_status(identity).filter(|status| {
                status.power_state == ApPeerPowerState::Active && !status.buffered_release_in_flight
            })?;
        } else if key.destination[0] & 1 == 0
            || engine.group_downlink_disposition() != ApDownlinkDisposition::TransmitNow
        {
            return None;
        }
        Some(key)
    }
}

impl<B: MaterializedTxFrame, N: SoftwareTxFrame> AccessPointNetworkTx<'_, B, N> {
    /// The RR frontier still services sleeping/invalid traffic for classification.
    /// Only radio-eligible heads participate in deficit rounds.
    pub(super) fn select_airtime_candidate(
        &mut self,
        engine: &ApEngine<'_>,
        queues: &dyn DestinationTxQueues<Frame = N>,
        source_len: usize,
        frontier: Option<Candidate>,
    ) -> Result<Option<(Candidate, ApTxFlowKey)>, AccessPointAirtimeError> {
        if frontier.is_none() {
            self.airtime
                .as_mut()
                .expect("enabled deficit selection")
                .select_candidates(engine, core::iter::empty())?;
            return Ok(None);
        }
        if frontier
            .and_then(|candidate| candidate.eligible_key(engine))
            .is_none()
        {
            return Ok(None);
        }
        let mut candidates: [Option<(Candidate, ApTxFlowKey)>; AP_MAX_CLIENTS + 1] =
            [None; AP_MAX_CLIENTS + 1];
        let after = self.last_destination;
        let order = |destination| (after.is_some_and(|last| destination <= last), destination);
        let mut insert = |candidate: Candidate| -> Result<(), AccessPointAirtimeError> {
            let Some(key) = candidate.eligible_key(engine) else {
                return Ok(());
            };
            let peer = Accounting::peer_for_key(engine, key);
            if let Some(existing) = candidates
                .iter_mut()
                .flatten()
                .find(|(_, existing)| Accounting::peer_for_key(engine, *existing) == peer)
            {
                // Buffered prefix precedes retained and producer frames. Different
                // multicast destinations share one account and rotate by address.
                if order(key.destination) < order(existing.1.destination) {
                    *existing = (candidate, key);
                }
            } else {
                let slot = candidates.iter_mut().find(|entry| entry.is_none()).ok_or(
                    AccessPointAirtimeError::Ledger(
                        oer_ieee80211_datapath::airtime::AirtimeError::AccountCapacity,
                    ),
                )?;
                *slot = Some((candidate, key));
            }
            Ok(())
        };
        if self.buffered_unicast.len != 0 {
            for buffered in self.buffered_unicast.slots.iter().flatten() {
                insert(Candidate::Buffered(buffered.identity))?;
            }
        }
        for (key, _) in self.active_frames.heads() {
            insert(Candidate::Retained(key))?;
        }
        // Strictly increasing addresses avoid wrap/revisit. The initial backlog
        // bounds inspection even if a producer concurrently publishes more peers.
        let mut previous = None;
        for _ in 0..source_len {
            let Some((destination, _)) = queues.next_head_after(previous) else {
                break;
            };
            if previous.is_some_and(|last| destination <= last) {
                break;
            }
            previous = Some(destination);
            insert(Candidate::Source(destination))?;
        }
        let keys = candidates.iter().flatten().map(|(_, key)| *key);
        let Some(key) = self
            .airtime
            .as_mut()
            .expect("enabled deficit selection")
            .select_candidates(engine, keys)?
        else {
            return Ok(None);
        };
        let candidate = candidates
            .into_iter()
            .flatten()
            .find(|(_, selected)| *selected == key)
            .expect("reserved head belongs to the merged candidate set")
            .0;
        Ok(Some((candidate, key)))
    }

    /// Unreserved leftovers must compete again after the preceding burst.
    /// A selected or built successor keeps its grant and packet position.
    pub(in super::super) fn reconsider_airtime_frontier(
        &mut self,
    ) -> Result<(), AccessPointDatapathError> {
        if !self
            .airtime
            .as_ref()
            .is_some_and(Accounting::needs_frontier_selection)
        {
            return Ok(());
        }
        let count = usize::from(self.prepared_first.is_some())
            + usize::from(self.prepared_second.is_some());
        let first = self.prepared_first_key;
        let second = self.prepared_second_key.filter(|key| Some(*key) != first);
        let new_flows = [first, second]
            .into_iter()
            .flatten()
            .filter(|key| self.active_frames.len_for(*key) == 0)
            .count();
        if self.frame_arena.remaining_capacity() < count
            || self.active_frames.heads().count() + new_flows > AP_ACTIVE_FLOW_CAPACITY
        {
            return Err(AccessPointAirtimeError::SelectionStorageFull.into());
        }
        if let Some(frame) = self.prepared_second.take() {
            let key = self
                .prepared_second_key
                .take()
                .expect("retained second keeps its key");
            self.restore_active_frame_front(key, frame);
        }
        if let Some(frame) = self.prepared_first.take() {
            let key = self
                .prepared_first_key
                .take()
                .expect("retained first keeps its key");
            self.restore_active_frame_front(key, frame);
        }
        Ok(())
    }
}
