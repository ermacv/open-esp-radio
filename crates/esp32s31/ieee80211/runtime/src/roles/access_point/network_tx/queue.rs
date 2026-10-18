//! Bounded lease storage and per-flow selection over one shared arena.

use crate::{
    datapath::{
        TxBatchDemand,
        software_tx_queue::{IndexedLeaseArena, RoundRobinTxQueues},
    },
    diagnostics::aggregate_tx::NetworkTxRetentionDropReason,
};

use super::*;

mod airtime;

// Any admitted software owner can become power-save traffic after dequeue.
// Retention therefore covers the entire software budget, including release
// tickets kept for rollback. Physical aggregates use an independent pool.
const AP_POWER_SAVE_FRAME_CAPACITY: usize = AP_SOFTWARE_TX_CAPACITY;

// FIFO-only adapters require radio-side regrouping. Owned destination queues
// bypass that regrouping; this arena retains rollback and power-save leases.
// The current AP data path
// negotiates only TID 0, so fifteen unicast peers plus one group/invalid
// frontier are sufficient. Extending AP QoS to more TIDs must raise this
// explicit flow bound; it must not add per-peer payload rings.
const AP_ACTIVE_FLOW_CAPACITY: usize = AP_MAX_CLIENTS + 1;
pub(super) const AP_ACTIVE_FRAME_CAPACITY: usize = AP_POWER_SAVE_FRAME_CAPACITY;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ApTxFlowKey {
    pub(super) destination: [u8; 6],
    association_epoch: u32,
    association_id: u8,
    tid: u8,
}

impl ApTxFlowKey {
    const INVALID_PEER: [u8; 6] = [0; 6];

    pub(super) const fn associated(identity: ApAssociationIdentity) -> Self {
        let association_id = identity.association_id();
        assert!(association_id <= u8::MAX as u16);
        Self {
            destination: identity.address(),
            association_epoch: identity.association_epoch(),
            association_id: association_id as u8,
            tid: oer_esp32s31_ieee80211_ap::protocol::AP_TX_BLOCK_ACK_TID,
        }
    }

    pub(super) fn unbound_from_ethernet(ethernet: &[u8]) -> Self {
        Self::unbound(
            ethernet
                .get(..6)
                .and_then(|bytes| <[u8; 6]>::try_from(bytes).ok())
                .unwrap_or(Self::INVALID_PEER),
        )
    }

    fn unbound(destination: [u8; 6]) -> Self {
        Self {
            destination,
            association_epoch: 0,
            association_id: 0,
            tid: oer_esp32s31_ieee80211_ap::protocol::AP_TX_BLOCK_ACK_TID,
        }
    }

    pub(super) fn association(self) -> Option<ApAssociationIdentity> {
        ApAssociationIdentity::new(
            self.destination,
            u16::from(self.association_id),
            self.association_epoch,
        )
    }

    pub(super) fn is_current(self, engine: &ApEngine<'_>) -> bool {
        match self.association() {
            Some(identity) => engine.association_is_current(identity),
            None if self.destination[0] & 1 != 0 || self.destination == Self::INVALID_PEER => true,
            None => engine
                .peer_status(self.destination)
                .is_none_or(|status| status.phase != ApPeerPhase::Authorized),
        }
    }
}

/// Single owner of AP-retained software leases. A power-save release keeps
/// its original slot through publication so rollback needs no new admission.
///
/// Active scheduling and both power-save policies store only indices into
/// this arena. Their capacities therefore describe classifications of the
/// same bounded retention credits, not three independent arrays of `B` handles.
pub(super) type ApFrameLeaseArena<B> = IndexedLeaseArena<B, AP_ACTIVE_FRAME_CAPACITY>;

/// Intrusive per-flow FIFO links over [`ApFrameLeaseArena`].
pub(super) type ApActiveFrameQueues =
    RoundRobinTxQueues<ApTxFlowKey, AP_ACTIVE_FLOW_CAPACITY, AP_ACTIVE_FRAME_CAPACITY>;

#[must_use = "a buffered release must be restored or completed"]
pub(super) struct BufferedUnicast {
    pub(super) identity: ApAssociationIdentity,
    order: u64,
    frame_index: u8,
}

impl BufferedUnicast {
    pub(super) fn frame<'a, B>(&self, arena: &'a ApFrameLeaseArena<B>) -> &'a B {
        arena.get(self.frame_index)
    }

    pub(super) fn complete<B>(self, arena: &mut ApFrameLeaseArena<B>) {
        drop(arena.take(self.frame_index));
    }
}

#[derive(Clone, Copy)]
struct BufferedUnicastIndex {
    identity: ApAssociationIdentity,
    order: u64,
    frame_index: u8,
}

#[must_use = "a buffered release must be restored or completed"]
pub(super) struct BufferedGroup {
    order: u64,
    frame_index: u8,
}

impl BufferedGroup {
    pub(super) fn frame<'a, B>(&self, arena: &'a ApFrameLeaseArena<B>) -> &'a B {
        arena.get(self.frame_index)
    }

    pub(super) fn complete<B>(self, arena: &mut ApFrameLeaseArena<B>) {
        drop(arena.take(self.frame_index));
    }
}

#[derive(Clone, Copy)]
struct BufferedGroupIndex {
    order: u64,
    frame_index: u8,
}

pub(super) struct ApPowerSaveFrameQueue {
    slots: [Option<BufferedUnicastIndex>; AP_POWER_SAVE_FRAME_CAPACITY],
    next_order: u64,
    pub(super) len: usize,
}

impl ApPowerSaveFrameQueue {
    pub(super) const fn new() -> Self {
        Self {
            slots: [const { None }; AP_POWER_SAVE_FRAME_CAPACITY],
            next_order: 0,
            len: 0,
        }
    }

    pub(super) fn push<B>(
        &mut self,
        identity: ApAssociationIdentity,
        frame: B,
        arena: &mut ApFrameLeaseArena<B>,
    ) -> Result<usize, B> {
        let Some(index) = self.slots.iter().position(Option::is_none) else {
            return Err(frame);
        };
        let frame_index = arena.insert(frame)?;
        let order = self.next_order;
        self.next_order = self.next_order.wrapping_add(1);
        self.slots[index] = Some(BufferedUnicastIndex {
            identity,
            order,
            frame_index,
        });
        self.len += 1;
        Ok(index)
    }

    pub(super) fn take_at(&mut self, index: usize) -> Option<BufferedUnicast> {
        let buffered = self.slots.get_mut(index)?.take()?;
        self.len -= 1;
        Some(BufferedUnicast {
            identity: buffered.identity,
            order: buffered.order,
            frame_index: buffered.frame_index,
        })
    }

    pub(super) fn restore(&mut self, buffered: BufferedUnicast) {
        let slot = self
            .slots
            .iter_mut()
            .find(|slot| slot.is_none())
            .expect("a released AP power-save lease always leaves one queue slot");
        *slot = Some(BufferedUnicastIndex {
            identity: buffered.identity,
            order: buffered.order,
            frame_index: buffered.frame_index,
        });
        self.len += 1;
    }

    pub(super) fn oldest_index_for(&self, identity: ApAssociationIdentity) -> Option<usize> {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| {
                let entry = entry.as_ref()?;
                (entry.identity == identity).then_some((index, entry.order))
            })
            .min_by_key(|(_, order)| *order)
            .map(|(index, _)| index)
    }

    pub(super) fn next_releasable_peer_after(
        &self,
        after: Option<[u8; 6]>,
        mut releasable: impl FnMut(ApAssociationIdentity) -> bool,
    ) -> Option<ApAssociationIdentity> {
        self.slots
            .iter()
            .flatten()
            .filter(|entry| releasable(entry.identity))
            .min_by_key(|entry| {
                (
                    after.is_some_and(|last| entry.identity.address() <= last),
                    entry.identity.address(),
                )
            })
            .map(|entry| entry.identity)
    }

    pub(super) fn retain<B>(
        &mut self,
        arena: &mut ApFrameLeaseArena<B>,
        mut keep: impl FnMut(ApAssociationIdentity) -> bool,
    ) {
        for slot in &mut self.slots {
            if slot.as_ref().is_some_and(|entry| !keep(entry.identity)) {
                let removed = slot.take().expect("checked AP power-save entry");
                drop(arena.take(removed.frame_index));
                self.len -= 1;
            }
        }
    }
}

/// Bounded caller-owned group queue. Entries are pinned network leases, not
/// payload copies; the portable AP owns only the matching advertised count.
pub(super) struct ApGroupFrameQueue {
    slots: [Option<BufferedGroupIndex>; AP_POWER_SAVE_FRAME_CAPACITY],
    next_order: u64,
    len: usize,
}

impl ApGroupFrameQueue {
    pub(super) const fn new() -> Self {
        Self {
            slots: [const { None }; AP_POWER_SAVE_FRAME_CAPACITY],
            next_order: 0,
            len: 0,
        }
    }

    pub(super) fn push<B>(
        &mut self,
        frame: B,
        arena: &mut ApFrameLeaseArena<B>,
    ) -> Result<usize, B> {
        let Some(index) = self.slots.iter().position(Option::is_none) else {
            return Err(frame);
        };
        let frame_index = arena.insert(frame)?;
        let order = self.next_order;
        self.next_order = self.next_order.wrapping_add(1);
        self.slots[index] = Some(BufferedGroupIndex { order, frame_index });
        self.len += 1;
        Ok(index)
    }

    pub(super) fn take_at(&mut self, index: usize) -> Option<BufferedGroup> {
        let buffered = self.slots.get_mut(index)?.take()?;
        self.len -= 1;
        Some(BufferedGroup {
            order: buffered.order,
            frame_index: buffered.frame_index,
        })
    }

    pub(super) fn restore(&mut self, buffered: BufferedGroup) {
        let slot = self
            .slots
            .iter_mut()
            .find(|slot| slot.is_none())
            .expect("a released AP group lease always leaves one queue slot");
        *slot = Some(BufferedGroupIndex {
            order: buffered.order,
            frame_index: buffered.frame_index,
        });
        self.len += 1;
    }

    pub(super) fn oldest_index(&self) -> Option<usize> {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| entry.as_ref().map(|entry| (index, entry.order)))
            .min_by_key(|(_, order)| *order)
            .map(|(index, _)| index)
    }

    pub(super) fn clear<B>(&mut self, arena: &mut ApFrameLeaseArena<B>) -> usize {
        let discarded = self.len;
        for slot in &mut self.slots {
            if let Some(removed) = slot.take() {
                drop(arena.take(removed.frame_index));
            }
        }
        self.len = 0;
        discarded
    }
}

impl<'observer, B, N> AccessPointNetworkTx<'observer, B, N>
where
    B: MaterializedTxFrame,
    N: SoftwareTxFrame,
{
    fn push_active_frame(&mut self, key: ApTxFlowKey, frame: N) -> Result<(), N> {
        let frame_index = self.frame_arena.insert(frame)?;
        if self.active_frames.push_back(key, frame_index).is_err() {
            return Err(self.frame_arena.take(frame_index));
        }
        Ok(())
    }

    fn push_active_frame_front(&mut self, key: ApTxFlowKey, frame: N) -> Result<(), N> {
        let frame_index = self.frame_arena.insert(frame)?;
        if self.active_frames.push_front(key, frame_index).is_err() {
            return Err(self.frame_arena.take(frame_index));
        }
        Ok(())
    }

    pub(super) fn restore_active_pair_front(&mut self, key: ApTxFlowKey, first: N, second: N) {
        // `push_front` reverses insertion order. Restore the younger frame
        // first so the next scheduler turn observes the original prefix.
        self.restore_active_frame_front(key, second);
        self.restore_active_frame_front(key, first);
    }

    pub(super) fn restore_active_frame_front(&mut self, key: ApTxFlowKey, frame: N) {
        self.push_active_frame_front(key, frame)
            .unwrap_or_else(|_| panic!("AP rollback lost its bounded arena credit"));
    }

    fn pop_active_key(&mut self, key: ApTxFlowKey) -> Option<N> {
        self.active_frames
            .pop_key(key)
            .map(|index| self.frame_arena.take(index))
    }

    fn observe_network_claim(&self, frame: &N) {
        #[cfg(any(feature = "diagnostics", test))]
        if let Some(observer) = self.observer {
            observer.observe_access_point_network_claim(frame.as_slice());
        }
        #[cfg(not(any(feature = "diagnostics", test)))]
        let _ = frame;
    }

    pub(super) fn discard_retention(&self, reason: NetworkTxRetentionDropReason, frame: N) {
        #[cfg(any(feature = "diagnostics", test))]
        if let Some(observer) = self.observer {
            observer.observe_access_point_retention_drop(reason);
        }
        #[cfg(not(any(feature = "diagnostics", test)))]
        let _ = reason;
        drop(frame);
    }

    pub(super) fn retain_active_frame(
        &mut self,
        engine: &mut ApEngine<'_>,
        frame: N,
    ) -> Result<(), AccessPointDatapathError> {
        self.observe_network_claim(&frame);
        let Some((key, frame)) = self.retain_power_save(engine, frame)? else {
            return Ok(());
        };
        // The production owned endpoint shares this arena's admission bound.
        // Other sources can still overload bounded radio retention.
        if let Err(frame) = self.push_active_frame(key, frame) {
            self.discard_retention(NetworkTxRetentionDropReason::ActiveQueueFull, frame);
        }
        Ok(())
    }

    /// Aggregation demand of the next network batch.
    ///
    /// A staged successor fixes its destination. Otherwise the batch is
    /// complete as soon as any destination holds its own target, so a peer
    /// without a Block Ack agreement or group traffic is never delayed for
    /// another peer's aggregate. An owner still unclassified in a FIFO source
    /// has no known destination and is never delayed either.
    pub(in super::super) fn batch_demand(
        &self,
        target_for: impl Fn([u8; 6]) -> usize,
        network: &impl SelectedBurstMaterializer<SoftwareFrame = N, PhysicalFrame = B>,
    ) -> TxBatchDemand {
        if self.prepared_group_release.is_some()
            || self.prepared_buffered_release.is_some()
            || self.awake_buffered_peer.is_some()
        {
            return TxBatchDemand::single(self.prepared_frame_count().max(1));
        }
        let queues = network.destination_queues();
        let source_for = |destination| queues.map_or(0, |queues| queues.pending_for(destination));
        let unclassified = if queues.is_some() {
            0
        } else {
            network.queue_len()
        };
        let retained_for = |destination| {
            self.active_frames
                .heads()
                .filter(|(key, _)| key.destination == destination)
                .map(|(key, _)| self.active_frames.len_for(key))
                .sum::<usize>()
        };
        if let Some(destination) = self.prepared_destination() {
            let staged = self.prepared_standby.as_ref().map_or_else(
                || self.prepared_frame_count(),
                |batch| batch.admitted + retained_for(destination),
            );
            return TxBatchDemand {
                target: target_for(destination),
                ready: staged + source_for(destination) + unclassified,
            };
        }

        let demand_for = |destination| TxBatchDemand {
            target: target_for(destination),
            ready: retained_for(destination) + source_for(destination),
        };
        let mut fullest: Option<TxBatchDemand> = None;
        let mut consider = |demand: TxBatchDemand| {
            if demand.complete() {
                return Some(demand);
            }
            if fullest.is_none_or(|fullest| demand.ready > fullest.ready) {
                fullest = Some(demand);
            }
            None
        };
        for (key, _) in self.active_frames.heads() {
            if let Some(complete) = consider(demand_for(key.destination)) {
                return complete;
            }
        }
        if let Some(queues) = queues {
            // Strictly increasing addresses bound one pass over the source.
            let mut previous = None;
            while let Some((destination, _)) = queues.next_head_after(previous) {
                if previous.is_some_and(|last| destination <= last) {
                    break;
                }
                previous = Some(destination);
                if let Some(complete) = consider(demand_for(destination)) {
                    return complete;
                }
            }
        }
        if unclassified != 0 {
            return TxBatchDemand::single(unclassified);
        }
        fullest.unwrap_or(TxBatchDemand::single(0))
    }

    pub(super) fn take_matching_active_or_network(
        &mut self,
        engine: &mut ApEngine<'_>,
        key: ApTxFlowKey,
        network: &impl SelectedBurstMaterializer<SoftwareFrame = N, PhysicalFrame = B>,
    ) -> Result<Option<N>, AccessPointDatapathError> {
        if !key.is_current(engine) {
            while let Some(frame) = self.pop_active_key(key) {
                drop(frame);
            }
            return Ok(None);
        }
        if key
            .association()
            .and_then(|identity| engine.association_status(identity))
            .is_some_and(|status| {
                status.power_state == ApPeerPowerState::Active
                    && status.buffered_unicast_frames != 0
            })
        {
            // Finish the retained PS prefix (including its in-flight head)
            // before claiming younger frames for a new aggregate.
            return Ok(None);
        }
        while let Some(frame) = self.pop_active_key(key) {
            // A retained owner may outlive a peer's transition to sleep.
            // Reclassify it before encoding, just like a source dequeue.
            if let Some((current, frame)) = self.retain_power_save(engine, frame)? {
                if current == key {
                    return Ok(Some(frame));
                }
                drop(frame);
            }
        }
        if let Some(queues) = network.destination_queues() {
            // Select before dequeue. A continuously publishing other peer
            // cannot consume retention slots or extend this finite visit.
            for _ in 0..queues.pending_for(key.destination) {
                let Some(frame) = queues.try_take_for(key.destination) else {
                    break;
                };
                self.observe_network_claim(&frame);
                let Some((frame_key, frame)) = self.retain_power_save(engine, frame)? else {
                    continue;
                };
                if frame_key == key {
                    return Ok(Some(frame));
                }
                // Radio association eligibility changed. Never retarget a
                // selected request to a different generation or aggregate.
                drop(frame);
            }
            return Ok(None);
        }
        // A FIFO compatibility source needs bounded regrouping. Reserve the
        // retention slot before touching its next owner, including nonmatches.
        for _ in 0..network.queue_len() {
            if self.frame_arena.remaining_capacity() == 0 {
                break;
            }
            let Some(frame) = network.try_take() else {
                break;
            };
            self.observe_network_claim(&frame);
            let Some((frame_key, frame)) = self.retain_power_save(engine, frame)? else {
                continue;
            };
            if frame_key == key {
                return Ok(Some(frame));
            }
            if TX_PHASE_TELEMETRY
                && self
                    .prepared_standby
                    .as_ref()
                    .is_some_and(|batch| batch.admission.peer() == key.destination)
            {
                let batch = self
                    .prepared_standby
                    .as_mut()
                    .expect("the checked AP standby remains owned");
                batch.mismatch_claims.increment();
            }
            if let Err(frame) = self.push_active_frame(frame_key, frame) {
                self.discard_retention(NetworkTxRetentionDropReason::ActiveQueueFull, frame);
            }
        }
        Ok(None)
    }

    pub(super) fn take_scheduled_active_or_network(
        &mut self,
        engine: &mut ApEngine<'_>,
        network: &impl SelectedBurstMaterializer<SoftwareFrame = N, PhysicalFrame = B>,
    ) -> Result<Option<ApTxSelection<N>>, AccessPointDatapathError> {
        if let Some(accounting) = self.airtime.as_ref() {
            accounting.require_selection_idle()?;
            if accounting.selects_peers() && network.destination_queues().is_none() {
                return Err(
                    super::airtime::AccessPointAirtimeError::DestinationQueuesRequired.into(),
                );
            }
        }
        let result = (|| {
            // Old generations must release their slots, not merely disappear from
            // selection until the AP stops. This is bounded by retained capacity.
            loop {
                let stale = self
                    .active_frames
                    .heads()
                    .find_map(|(key, _)| (!key.is_current(engine)).then_some(key));
                let Some(stale) = stale else { break };
                while let Some(frame) = self.pop_active_key(stale) {
                    drop(frame);
                }
            }
            self.refresh_awake_demand(engine);
            let after = self.last_destination;
            let order = |destination| (after.is_some_and(|last| destination <= last), destination);
            let retained = self
                .active_frames
                .heads()
                .map(|(key, _)| key)
                .min_by_key(|key| order(key.destination))
                .map(Candidate::Retained);
            let queues = network.destination_queues();
            let source = queues
                .and_then(|queues| queues.next_head_after(after))
                .map(|(destination, _)| Candidate::Source(destination));
            // One turn per destination. Buffered packets precede newer retained
            // and producer packets for the same station. The source lock is gone
            // before any radio policy or affine release operation runs.
            let mut selected = [
                self.awake_buffered_peer.map(Candidate::Buffered),
                retained,
                source,
            ]
            .into_iter()
            .flatten()
            .min_by_key(|candidate| order(candidate.destination()));
            let scheduled = if self
                .airtime
                .as_ref()
                .is_some_and(super::airtime::Accounting::selects_peers)
            {
                self.select_airtime_candidate(
                    engine,
                    queues.expect("deficit selection requires destination inspection"),
                    network.queue_len(),
                    selected,
                )?
            } else {
                None
            };
            if let Some((candidate, _)) = scheduled {
                selected = Some(candidate);
            }
            let reserved_key = if let Some((_, key)) = scheduled {
                Some(key)
            } else if let Some(accounting) = self.airtime.as_mut() {
                let key = match selected {
                    Some(Candidate::Retained(key)) => Some(key),
                    Some(Candidate::Source(destination)) => Some(
                        engine
                            .admit_downlink(destination)
                            .ok()
                            .map(|peer| ApTxFlowKey::associated(peer.identity()))
                            .unwrap_or_else(|| ApTxFlowKey::unbound(destination)),
                    ),
                    _ => None,
                }
                .filter(|key| {
                    key.association().is_none_or(|identity| {
                        engine
                            .association_status(identity)
                            .is_some_and(|status| status.power_state == ApPeerPowerState::Active)
                    })
                });
                if let Some(key) = key {
                    accounting.select(engine, key)?;
                }
                key
            } else {
                None
            };
            let frame = match selected {
                Some(Candidate::Buffered(identity)) => {
                    if scheduled.is_none()
                        && let Some(accounting) = self.airtime.as_mut()
                    {
                        accounting.select(engine, ApTxFlowKey::associated(identity))?;
                    }
                    let release = self.select_awake_buffered_release(engine, identity);
                    if !matches!(release, Ok(Some(_)))
                        && let Some(accounting) = self.airtime.as_mut()
                    {
                        accounting.cancel_selection()?;
                    }
                    return release.map(|release| release.map(ApTxSelection::Buffered));
                }
                Some(Candidate::Retained(key)) => {
                    self.last_destination = Some(key.destination);
                    self.pop_active_key(key)
                        .expect("retained candidate owns its head")
                }
                Some(Candidate::Source(destination)) => {
                    self.last_destination = Some(destination);
                    let Some(frame) = queues
                        .expect("selected destination source")
                        .try_take_for(destination)
                    else {
                        return Ok(None);
                    };
                    self.observe_network_claim(&frame);
                    frame
                }
                None => {
                    if self
                        .airtime
                        .as_ref()
                        .is_some_and(super::airtime::Accounting::selects_peers)
                    {
                        // A producer may publish after the empty snapshot. Leave
                        // that head for the next selection instead of bypassing
                        // deficit arbitration through an opaque FIFO claim.
                        return Ok(None);
                    }
                    // Reserve rollback room before claiming an opaque FIFO head.
                    if self.airtime.is_some()
                        && self.frame_arena.remaining_capacity() == 0
                        && network.queue_len() != 0
                    {
                        return Err(
                            super::airtime::AccessPointAirtimeError::SelectionStorageFull.into(),
                        );
                    }
                    // An opaque FIFO cannot expose candidates without claiming.
                    let Some(frame) = network.try_take() else {
                        return Ok(None);
                    };
                    self.observe_network_claim(&frame);
                    frame
                }
            };
            let Some((key, frame)) = self.retain_power_save(engine, frame)? else {
                return Ok(None);
            };
            if reserved_key.is_some_and(|reserved| reserved != key) {
                return Err(super::airtime::AccessPointAirtimeError::SelectedPeerChanged.into());
            }
            if reserved_key.is_none()
                && let Some(accounting) = self.airtime.as_mut()
                && let Err(error) = accounting.select(engine, key)
            {
                // A selected source packet needs bounded rollback storage just as
                // materialization does. Existing retained heads already leave a slot.
                self.restore_active_frame_front(key, frame);
                return Err(error.into());
            }
            Ok(Some(ApTxSelection::Frame(key, frame)))
        })();
        if !matches!(result, Ok(Some(_)))
            && let Some(accounting) = self.airtime.as_mut()
        {
            accounting.cancel_selection()?;
        }
        result
    }
}

#[derive(Clone, Copy)]
enum Candidate {
    Buffered(ApAssociationIdentity),
    Retained(ApTxFlowKey),
    Source([u8; 6]),
}

impl Candidate {
    fn destination(self) -> [u8; 6] {
        match self {
            Self::Buffered(identity) => identity.address(),
            Self::Retained(key) => key.destination,
            Self::Source(destination) => destination,
        }
    }
}
