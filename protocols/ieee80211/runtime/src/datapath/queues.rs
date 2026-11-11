//! The station's transmit queues on the MAC: one logical queue per TID of
//! the link, over one bounded pool of the network's MSDUs. The network's
//! queue is only their transport: the MAC takes what it published at once,
//! so a TID that cannot go on keeps its own MSDUs and holds no other's.
//!
//! The MAC chooses the queue an arena starts from: the highest access
//! category with a ready queue, and within it the queue that has had the
//! least air, by one charge for each attempt that ended. A queue is ready
//! while it holds an MSDU and its TID does not wait for its window. An arena
//! holds one TID, so the standby, once it holds frames, takes from its own
//! queue only; that is the quantum, one PPDU.
//!
//! The pool holds every MSDU the network can have out: its buffers bound
//! it. An MSDU the pool cannot take stays in the network's queue.

use oer_protocols_ieee80211_contracts_port::TxEstimate;
use oer_protocols_ieee80211_core_roles_sta::data::WmmUserPriority;
use oer_protocols_ieee80211_core_roles_sta::data::plane::OutboundMsdu;

/// No place: the end of a list.
const END: u8 = u8::MAX;

/// The queues over a pool of `POOL` MSDUs (below 255).
pub(super) struct TxQueues<M, const POOL: usize> {
    msdus: [Option<OutboundMsdu<M>>; POOL],
    /// Each place's successor in its list: its queue's, or the free list's.
    next: [u8; POOL],
    free: u8,
    heads: [u8; 8],
    tails: [u8; 8],
    /// TIDs whose head waits for their window.
    blocked: u8,
    /// TIDs that hold an MSDU.
    active: u8,
    /// The air each TID's attempts took, in microseconds, against the
    /// others of its access category.
    served: [u32; 8],
    /// Each TID's dearest known attempt: what an attempt of unknown cost is
    /// charged.
    dearest: [u32; 8],
}

impl<M, const POOL: usize> TxQueues<M, POOL> {
    pub(super) fn new() -> Self {
        const {
            if POOL >= END as usize {
                panic!("a pool place is a u8 below END");
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
            heads: [END; 8],
            tails: [END; 8],
            blocked: 0,
            active: 0,
            served: [0; 8],
            dearest: [0; 8],
        }
    }

    /// Whether the pool takes another MSDU.
    pub(super) const fn has_room(&self) -> bool {
        self.free != END
    }

    /// Whether any queue holds an MSDU.
    pub(super) const fn is_empty(&self) -> bool {
        self.active == 0
    }

    /// Whether a queue other than a blocked one holds an MSDU.
    pub(super) const fn has_ready(&self) -> bool {
        self.active & !self.blocked != 0
    }

    /// Queue `msdu` behind its TID's others.
    ///
    /// # Errors
    /// The MSDU back: the pool is full.
    pub(super) fn push(
        &mut self,
        tid: WmmUserPriority,
        msdu: OutboundMsdu<M>,
    ) -> Result<(), OutboundMsdu<M>> {
        let Some(place) = self.take_free() else {
            return Err(msdu);
        };
        let Some(slot) = self.msdus.get_mut(usize::from(place)) else {
            return Err(msdu);
        };
        *slot = Some(msdu);
        // The last of its queue.
        if let Some(link) = self.next.get_mut(usize::from(place)) {
            *link = END;
        }
        let queue = queue(tid);
        let tail = self.tails.get(queue).copied().unwrap_or(END);
        if tail == END {
            self.set_head(queue, place);
            self.activated(queue);
        } else if let Some(link) = self.next.get_mut(usize::from(tail)) {
            *link = place;
        }
        if let Some(slot) = self.tails.get_mut(queue) {
            *slot = place;
        }
        Ok(())
    }

    /// Put `msdu` back at the head of its TID's queue: the plane did not
    /// take it.
    ///
    /// # Errors
    /// The MSDU back: the pool is full, which a place just emptied rules
    /// out.
    pub(super) fn push_front(
        &mut self,
        tid: WmmUserPriority,
        msdu: OutboundMsdu<M>,
    ) -> Result<(), OutboundMsdu<M>> {
        let Some(place) = self.take_free() else {
            return Err(msdu);
        };
        let Some(slot) = self.msdus.get_mut(usize::from(place)) else {
            return Err(msdu);
        };
        *slot = Some(msdu);
        let queue = queue(tid);
        let head = self.heads.get(queue).copied().unwrap_or(END);
        if let Some(link) = self.next.get_mut(usize::from(place)) {
            *link = head;
        }
        self.set_head(queue, place);
        if head == END {
            if let Some(slot) = self.tails.get_mut(queue) {
                *slot = place;
            }
            self.activated(queue);
        }
        Ok(())
    }

    /// The oldest MSDU of `tid`'s queue.
    pub(super) fn pop(&mut self, tid: WmmUserPriority) -> Option<OutboundMsdu<M>> {
        let queue = queue(tid);
        let place = self.heads.get(queue).copied().filter(|head| *head != END)?;
        let next = self.next.get(usize::from(place)).copied().unwrap_or(END);
        self.set_head(queue, next);
        if next == END {
            if let Some(slot) = self.tails.get_mut(queue) {
                *slot = END;
            }
            self.active &= !bit(queue);
        }
        let msdu = self
            .msdus
            .get_mut(usize::from(place))
            .and_then(Option::take);
        if let Some(link) = self.next.get_mut(usize::from(place)) {
            *link = self.free;
        }
        self.free = place;
        msdu
    }

    /// The oldest MSDU of any queue: for a stop, which returns them all.
    pub(super) fn pop_any(&mut self) -> Option<OutboundMsdu<M>> {
        let tid = WmmUserPriority::new(self.active.trailing_zeros() as u8)?;
        self.pop(tid)
    }

    /// `tid`'s head waits for its window: the queue is not ready until
    /// [`Self::unblock`].
    pub(super) fn block(&mut self, tid: WmmUserPriority) {
        self.blocked |= bit(queue(tid));
    }

    /// A settlement or an agreement may have moved the windows: every queue
    /// is ready again while it holds an MSDU.
    pub(super) fn unblock(&mut self) {
        self.blocked = 0;
    }

    /// Whether `tid`'s queue holds an MSDU and does not wait.
    pub(super) const fn ready(&self, tid: WmmUserPriority) -> bool {
        (self.active & !self.blocked) & bit(queue(tid)) != 0
    }

    /// The queue the next arena starts from, `hold` aside: the ready queue
    /// of the highest access category, and of those the one that took the
    /// least air.
    pub(super) fn select(&self, hold: Option<WmmUserPriority>) -> Option<WmmUserPriority> {
        let mut ready = self.active & !self.blocked;
        if let Some(hold) = hold {
            ready &= !bit(queue(hold));
        }
        if ready == 0 {
            return None;
        }
        // Only one: no comparison.
        if ready.is_power_of_two() {
            return WmmUserPriority::new(ready.trailing_zeros() as u8);
        }
        let mut best: Option<(u8, u32, WmmUserPriority)> = None;
        for tid in WmmUserPriority::ALL {
            if ready & bit(queue(tid)) == 0 {
                continue;
            }
            let rank = rank(tid);
            let served = self.served.get(queue(tid)).copied().unwrap_or(0);
            let better = best.is_none_or(|(best_rank, best_served, _)| {
                rank > best_rank || (rank == best_rank && served < best_served)
            });
            if better {
                best = Some((rank, served, tid));
            }
        }
        best.map(|(_, _, tid)| tid)
    }

    /// One attempt of `tid` ended, costing `cost`: its queue's account takes
    /// it once. An attempt of unknown cost takes the queue's dearest known
    /// one, never nothing.
    pub(super) fn charge(&mut self, tid: WmmUserPriority, cost: TxEstimate) {
        let queue = queue(tid);
        let (Some(served), Some(dearest)) = (self.served.get(queue), self.dearest.get(queue))
        else {
            return;
        };
        let micros = match cost {
            TxEstimate::Estimated { micros } => micros,
            TxEstimate::Unknown => (*dearest).max(1),
        };
        let (served, dearest) = (*served, (*dearest).max(micros));
        if let Some(slot) = self.dearest.get_mut(queue) {
            *slot = dearest;
        }
        if let Some(slot) = self.served.get_mut(queue) {
            *slot = served.saturating_add(micros);
        }
        // Accounts are compared within a category only: keep them small.
        if self.served.iter().any(|served| *served > u32::MAX / 2) {
            let least = self.served.iter().copied().min().unwrap_or(0);
            for served in &mut self.served {
                *served = served.saturating_sub(least);
            }
        }
    }

    /// Hand every queued MSDU to `returned`, every queue emptied.
    pub(super) fn drain(&mut self, mut returned: impl FnMut(OutboundMsdu<M>)) {
        for tid in WmmUserPriority::ALL {
            while let Some(msdu) = self.pop(tid) {
                returned(msdu);
            }
        }
        self.blocked = 0;
    }

    fn take_free(&mut self) -> Option<u8> {
        let place = self.free;
        if place == END {
            return None;
        }
        self.free = self.next.get(usize::from(place)).copied().unwrap_or(END);
        Some(place)
    }

    fn set_head(&mut self, queue: usize, place: u8) {
        if let Some(slot) = self.heads.get_mut(queue) {
            *slot = place;
        }
    }

    /// The queue starts to hold MSDUs: it takes no credit for the air it
    /// did not ask for while idle, and starts level with the least served
    /// active queue of its category.
    fn activated(&mut self, queue: usize) {
        self.active |= bit(queue);
        let category = WmmUserPriority::new(queue as u8).map(rank);
        let least = WmmUserPriority::ALL
            .into_iter()
            .filter(|tid| {
                let other = self::queue(*tid);
                other != queue && self.active & bit(other) != 0 && Some(rank(*tid)) == category
            })
            .filter_map(|tid| self.served.get(self::queue(tid)).copied())
            .min();
        if let (Some(least), Some(served)) = (least, self.served.get_mut(queue)) {
            *served = (*served).max(least);
        }
    }
}

const fn queue(tid: WmmUserPriority) -> usize {
    tid.value() as usize
}

const fn bit(queue: usize) -> u8 {
    1 << (queue & 7)
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
