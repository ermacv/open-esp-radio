//! The MAC's transmit queues: one logical queue per radio peer and TID,
//! over one bounded pool of the network's MSDUs. The network's queue is
//! only their transport: the MAC takes what it published at once, so a
//! queue that cannot go on keeps its own MSDUs and holds no other's.
//!
//! The MAC chooses the queue the next transmission starts from: the highest
//! access category any ready queue holds; of the peers with a ready queue of
//! it, the one that had the least air; and of that peer's queues of it, the
//! one that had the least air. Several TIDs of one peer earn it no more air
//! than one. A queue is ready while it holds an MSDU and waits for nothing
//! ([`TxQueues::block`]). Each attempt that ended is charged once, whole, to
//! its queue and its peer. A transmission of one queue is the quantum: an
//! arena holds one TID, so the standby, once it holds frames, takes from its
//! own queue only.
//!
//! The pool holds every MSDU the network can have out: its buffers bound
//! it. An MSDU the pool cannot take stays in the network's queue.

use oer_protocols_ieee80211_contracts_port::TxEstimate;
use oer_protocols_ieee80211_core_roles_sta::data::WmmUserPriority;
use oer_protocols_ieee80211_core_roles_sta::data::plane::OutboundMsdu;

/// No place: the end of a list.
const END: u8 = u8::MAX;
/// TIDs, hence queues, of one peer.
const TIDS: usize = 8;

/// One logical queue: a peer's TID.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct QueueId(u8);

impl QueueId {
    /// `peer`'s queue of `tid`; `None` past the queues there are.
    pub(crate) const fn new(peer: u8, tid: WmmUserPriority) -> Option<Self> {
        match peer.checked_mul(TIDS as u8) {
            Some(base) => match base.checked_add(tid.value()) {
                Some(id) if (id as usize) < 128 => Some(Self(id)),
                _ => None,
            },
            None => None,
        }
    }

    /// The station's one peer's queue of `tid`.
    pub(crate) const fn of_tid(tid: WmmUserPriority) -> Self {
        Self(tid.value())
    }

    pub(crate) const fn peer(self) -> u8 {
        self.0 >> 3
    }

    pub(crate) fn tid(self) -> WmmUserPriority {
        // Below 8.
        WmmUserPriority::new(self.0 & 7).unwrap_or(WmmUserPriority::UP0)
    }

    const fn bit(self) -> u128 {
        1 << (self.0 & 127)
    }
}

/// The queues of `PEERS` peers over a pool of `POOL` MSDUs (below 255);
/// `PEERS` at most 16.
pub(crate) struct TxQueues<M, const POOL: usize, const PEERS: usize> {
    msdus: [Option<OutboundMsdu<M>>; POOL],
    /// Each place's successor in its list: its queue's, or the free list's.
    next: [u8; POOL],
    free: u8,
    /// Per queue: its head and tail places, and the air its attempts took
    /// against the other queues of its peer and access category.
    heads: [[u8; TIDS]; PEERS],
    tails: [[u8; TIDS]; PEERS],
    served: [[u32; TIDS]; PEERS],
    /// Per peer: the air its attempts took, against the other peers.
    peer_served: [u32; PEERS],
    /// Queues that hold an MSDU, and those that wait.
    active: u128,
    blocked: u128,
    /// The dearest known attempt: what an attempt of unknown cost is
    /// charged.
    dearest: u32,
}

impl<M, const POOL: usize, const PEERS: usize> TxQueues<M, POOL, PEERS> {
    pub(crate) fn new() -> Self {
        const {
            if POOL >= END as usize {
                panic!("a pool place is a u8 below END");
            }
            if PEERS == 0 || PEERS * TIDS > 128 {
                panic!("one to sixteen peers: a queue is a bit of a u128");
            }
        };
        let mut next = [END; POOL];
        for (place, link) in next.iter_mut().enumerate() {
            // Below END: the pool's size says so.
            *link = u8::try_from(place.saturating_add(1)).unwrap_or(END);
        }
        if let Some(last) = next.last_mut() {
            *last = END;
        }
        Self {
            msdus: core::array::from_fn(|_| None),
            next,
            free: if POOL == 0 { END } else { 0 },
            heads: [[END; TIDS]; PEERS],
            tails: [[END; TIDS]; PEERS],
            served: [[0; TIDS]; PEERS],
            peer_served: [0; PEERS],
            active: 0,
            blocked: 0,
            dearest: 0,
        }
    }

    /// Whether the pool takes another MSDU.
    pub(crate) const fn has_room(&self) -> bool {
        self.free != END
    }

    /// Whether any queue holds an MSDU.
    pub(crate) const fn is_empty(&self) -> bool {
        self.active == 0
    }

    /// Whether a queue that does not wait holds an MSDU.
    pub(crate) const fn has_ready(&self) -> bool {
        self.active & !self.blocked != 0
    }

    /// Whether `queue` holds an MSDU and does not wait.
    pub(crate) const fn ready(&self, queue: QueueId) -> bool {
        self.active & !self.blocked & queue.bit() != 0
    }

    /// Queue `msdu` behind its queue's others.
    ///
    /// # Errors
    /// The MSDU back: the pool is full or `queue` is past the peers.
    pub(crate) fn push(
        &mut self,
        queue: QueueId,
        msdu: OutboundMsdu<M>,
    ) -> Result<(), OutboundMsdu<M>> {
        if usize::from(queue.peer()) >= PEERS {
            return Err(msdu);
        }
        let Some(place) = self.take_free() else {
            return Err(msdu);
        };
        self.place(place, msdu, END);
        match self.tail(queue) {
            END => {
                self.set_head(queue, place);
                self.activated(queue);
            }
            tail => self.link(tail, place),
        }
        self.set_tail(queue, place);
        Ok(())
    }

    /// Put `msdu` back at the head of its queue: the plane did not take it.
    ///
    /// # Errors
    /// The MSDU back: the pool is full, which a place just emptied rules
    /// out, or `queue` is past the peers.
    pub(crate) fn push_front(
        &mut self,
        queue: QueueId,
        msdu: OutboundMsdu<M>,
    ) -> Result<(), OutboundMsdu<M>> {
        if usize::from(queue.peer()) >= PEERS {
            return Err(msdu);
        }
        let Some(place) = self.take_free() else {
            return Err(msdu);
        };
        let head = self.head(queue);
        self.place(place, msdu, head);
        self.set_head(queue, place);
        if head == END {
            self.set_tail(queue, place);
            self.activated(queue);
        }
        Ok(())
    }

    /// The oldest MSDU of `queue`.
    pub(crate) fn pop(&mut self, queue: QueueId) -> Option<OutboundMsdu<M>> {
        let place = self.head(queue);
        if place == END {
            return None;
        }
        let next = self.next.get(usize::from(place)).copied().unwrap_or(END);
        self.set_head(queue, next);
        if next == END {
            self.set_tail(queue, END);
            self.active &= !queue.bit();
        }
        let msdu = self
            .msdus
            .get_mut(usize::from(place))
            .and_then(Option::take);
        self.link(place, self.free);
        self.free = place;
        msdu
    }

    /// The oldest MSDU of any queue: for a stop, which returns them all.
    pub(crate) fn pop_any(&mut self) -> Option<OutboundMsdu<M>> {
        let queue = QueueId(u8::try_from(self.active.trailing_zeros()).ok()?);
        self.pop(queue)
    }

    /// `queue`'s head waits for its window: the queue is not ready until
    /// [`Self::unblock`].
    pub(crate) fn block(&mut self, queue: QueueId) {
        self.blocked |= queue.bit();
    }

    /// A settlement or an agreement may have freed a queue:
    /// every queue is ready again while it holds an MSDU.
    pub(crate) fn unblock(&mut self) {
        self.blocked = 0;
    }

    /// The queue the next transmission starts from, `hold` aside.
    pub(crate) fn select(&self, hold: Option<QueueId>) -> Option<QueueId> {
        let mut ready = self.active & !self.blocked;
        if let Some(hold) = hold {
            ready &= !hold.bit();
        }
        if ready == 0 {
            return None;
        }
        // Only one: no comparison.
        if ready.is_power_of_two() {
            return u8::try_from(ready.trailing_zeros()).ok().map(QueueId);
        }
        // The highest category any ready queue holds.
        let rank = (0..PEERS)
            .flat_map(|peer| WmmUserPriority::ALL.into_iter().map(move |tid| (peer, tid)))
            .filter_map(|(peer, tid)| QueueId::new(u8::try_from(peer).ok()?, tid))
            .filter(|queue| ready & queue.bit() != 0)
            .map(|queue| rank(queue.tid()))
            .max()?;
        let mut best: Option<(u32, u32, QueueId)> = None;
        for peer in 0..PEERS {
            let Some(peer_id) = u8::try_from(peer).ok() else {
                continue;
            };
            if ready & Self::peer_mask(peer_id) == 0 {
                continue;
            }
            let peer_air = self.peer_served.get(peer).copied().unwrap_or(0);
            for tid in WmmUserPriority::ALL {
                let Some(queue) = QueueId::new(peer_id, tid) else {
                    continue;
                };
                if ready & queue.bit() == 0 || self::rank(tid) != rank {
                    continue;
                }
                let air = self.air(queue);
                let better = best.is_none_or(|(best_peer, best_air, _)| {
                    peer_air < best_peer || (peer_air == best_peer && air < best_air)
                });
                if better {
                    best = Some((peer_air, air, queue));
                }
            }
        }
        best.map(|(_, _, queue)| queue)
    }

    /// One attempt of `queue` ended, costing `cost`: its queue and its peer
    /// take it once. An attempt of unknown cost takes the dearest known
    /// one, never nothing.
    pub(crate) fn charge(&mut self, queue: QueueId, cost: TxEstimate) {
        let micros = match cost {
            TxEstimate::Estimated { micros } => {
                self.dearest = self.dearest.max(micros);
                micros
            }
            TxEstimate::Unknown => self.dearest.max(1),
        };
        let peer = usize::from(queue.peer());
        if let Some(served) = self
            .served
            .get_mut(peer)
            .and_then(|tids| tids.get_mut(usize::from(queue.tid().value())))
        {
            *served = served.saturating_add(micros);
        }
        if let Some(served) = self.peer_served.get_mut(peer) {
            *served = served.saturating_add(micros);
        }
        // Accounts are compared with each other only: keep them small.
        if self.peer_served.iter().any(|served| *served > u32::MAX / 2) {
            let least = self.peer_served.iter().copied().min().unwrap_or(0);
            for served in &mut self.peer_served {
                *served = served.saturating_sub(least);
            }
        }
        if let Some(tids) = self.served.get_mut(peer)
            && tids.iter().any(|served| *served > u32::MAX / 2)
        {
            let least = tids.iter().copied().min().unwrap_or(0);
            for served in tids {
                *served = served.saturating_sub(least);
            }
        }
    }

    /// Hand every queued MSDU to `returned`, every queue emptied.
    pub(crate) fn drain(&mut self, mut returned: impl FnMut(OutboundMsdu<M>)) {
        while let Some(msdu) = self.pop_any() {
            returned(msdu);
        }
        self.blocked = 0;
    }

    const fn peer_mask(peer: u8) -> u128 {
        match (peer as u32).checked_mul(TIDS as u32) {
            Some(shift) if shift < 128 => 0xff << shift,
            _ => 0,
        }
    }

    fn air(&self, queue: QueueId) -> u32 {
        self.served
            .get(usize::from(queue.peer()))
            .and_then(|tids| tids.get(usize::from(queue.tid().value())))
            .copied()
            .unwrap_or(0)
    }

    fn take_free(&mut self) -> Option<u8> {
        let place = self.free;
        if place == END {
            return None;
        }
        self.free = self.next.get(usize::from(place)).copied().unwrap_or(END);
        Some(place)
    }

    fn place(&mut self, place: u8, msdu: OutboundMsdu<M>, next: u8) {
        if let Some(slot) = self.msdus.get_mut(usize::from(place)) {
            *slot = Some(msdu);
        }
        self.link(place, next);
    }

    fn link(&mut self, place: u8, next: u8) {
        if let Some(link) = self.next.get_mut(usize::from(place)) {
            *link = next;
        }
    }

    fn head(&self, queue: QueueId) -> u8 {
        self.heads
            .get(usize::from(queue.peer()))
            .and_then(|tids| tids.get(usize::from(queue.tid().value())))
            .copied()
            .unwrap_or(END)
    }

    fn tail(&self, queue: QueueId) -> u8 {
        self.tails
            .get(usize::from(queue.peer()))
            .and_then(|tids| tids.get(usize::from(queue.tid().value())))
            .copied()
            .unwrap_or(END)
    }

    fn set_head(&mut self, queue: QueueId, place: u8) {
        if let Some(slot) = self
            .heads
            .get_mut(usize::from(queue.peer()))
            .and_then(|tids| tids.get_mut(usize::from(queue.tid().value())))
        {
            *slot = place;
        }
    }

    fn set_tail(&mut self, queue: QueueId, place: u8) {
        if let Some(slot) = self
            .tails
            .get_mut(usize::from(queue.peer()))
            .and_then(|tids| tids.get_mut(usize::from(queue.tid().value())))
        {
            *slot = place;
        }
    }

    /// `queue` starts to hold MSDUs: neither it nor its peer takes credit
    /// for air it did not ask for while idle. Each starts level with the
    /// least served of its kind that holds MSDUs.
    fn activated(&mut self, queue: QueueId) {
        let peer = queue.peer();
        let peer_was_idle = self.active & Self::peer_mask(peer) == 0;
        self.active |= queue.bit();
        let category = rank(queue.tid());
        let least_queue = WmmUserPriority::ALL
            .into_iter()
            .filter(|tid| *tid != queue.tid() && rank(*tid) == category)
            .filter_map(|tid| QueueId::new(peer, tid))
            .filter(|other| self.active & other.bit() != 0)
            .map(|other| self.air(other))
            .min();
        if let (Some(least), Some(served)) = (
            least_queue,
            self.served
                .get_mut(usize::from(peer))
                .and_then(|tids| tids.get_mut(usize::from(queue.tid().value()))),
        ) {
            *served = (*served).max(least);
        }
        if peer_was_idle {
            let least_peer = (0..PEERS)
                .filter(|other| *other != usize::from(peer))
                .filter(|other| {
                    u8::try_from(*other)
                        .is_ok_and(|other| self.active & Self::peer_mask(other) != 0)
                })
                .filter_map(|other| self.peer_served.get(other).copied())
                .min();
            if let (Some(least), Some(served)) =
                (least_peer, self.peer_served.get_mut(usize::from(peer)))
            {
                *served = (*served).max(least);
            }
        }
    }
}

/// An access category's precedence: voice first, then video, best effort,
/// background.
const fn rank(tid: WmmUserPriority) -> u8 {
    use oer_protocols_ieee80211_core_roles_sta::data::WmmAccessCategory as Ac;
    match tid.access_category() {
        Ac::Voice => 3,
        Ac::Video => 2,
        Ac::BestEffort => 1,
        Ac::Background => 0,
    }
}

#[cfg(test)]
mod tests;
