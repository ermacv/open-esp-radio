//! The station's side of a network's queued egress: its air as one account
//! per association ([`StationAir`]), and the ends of the frames it took, as
//! the egress policy hears them ([`Completions`]).
//!
//! Every frame of a station, broadcast included, goes on the air to its
//! access point, so the station has one account per association, keyed by
//! the link epoch the network side reads. A frame's end carries what its
//! transmissions cost by the backend's model ([`TxEstimate`]); one whose
//! cost is not known is charged the backend's ceiling of that model
//! ([`FrameCost`]).
//!
//! A buffer the station gives back and its frame's settlement are two
//! events. [`Completions`] keeps each returned buffer with its end until the
//! end is handed on, and the buffer goes back to its owner only then: so no
//! more frames are unsettled than there are buffers.

use core::num::NonZeroU32;

use oer_portable_memory::QueueNotify;
use oer_protocols_ieee80211_contracts_port::{PhyRate, TxEnd, TxEstimate};
use oer_protocols_ieee80211_core_upper_mac::airtime::AirtimeCandidate;
use oer_protocols_network_port::{LinkSnapshot, LinkState, Ready};

use super::NetworkLink;
use crate::egress::{Exchange, PeerAir, RadioNotice};

/// An association's account: the link epoch it runs under.
pub type StationKey = u64;

/// What the backend's model says of a frame of a given Ethernet length: the
/// least it costs, its least useful grant, and the most it can cost on the
/// path the backend executes, which an egress policy charges where the cost
/// is not known.
pub trait FrameCost {
    /// The frame's least useful grant at the full rate `rate`: the one its
    /// association's last settled frame went at, or before one did, the
    /// association's slowest data rate. [`StationAir`] asks only with a
    /// rate it resolved for a live association.
    fn least(&self, frame_octets: usize, rate: PhyRate) -> NonZeroU32;
    fn ceiling(&self, frame_octets: usize) -> NonZeroU32;
}

/// The station's air over `link`, the network side's view of its link,
/// with `C` charging a frame of unknown cost. It keeps the full rate the
/// association's last settled frame went at, and asks each frame's least
/// grant at it: until a frame of the association that runs now reports one,
/// at that association's slowest data rate, which the link publishes with
/// its epoch.
pub struct StationAir<'a, N, C> {
    link: &'a NetworkLink<N>,
    ceiling: C,
    /// The association and its last reported rate.
    last: Option<(StationKey, PhyRate)>,
}

impl<'a, N, C> StationAir<'a, N, C> {
    pub const fn new(link: &'a NetworkLink<N>, ceiling: C) -> Self {
        Self {
            link,
            ceiling,
            last: None,
        }
    }
}

impl<N: QueueNotify, C: FrameCost> PeerAir for StationAir<'_, N, C> {
    type Key = StationKey;

    /// While the link is up, its association's account, with the frame's
    /// least useful grant at the rate resolved for it. A live link carries
    /// its association's slowest rate, which the lifecycle publishes with
    /// the association; a link up with no rate resolved offers no account,
    /// so no frame is charged at a rate the association was not given.
    fn candidate(&self, ready: &Ready) -> Option<AirtimeCandidate<StationKey>> {
        let (link, floor) = self.link.read();
        if link.state != LinkState::Up {
            return None;
        }
        // Another association's rate says nothing of this one's air; the
        // association's own slowest rate stands until it reports one.
        let rate = self
            .last
            .filter(|(key, _)| *key == link.epoch)
            .map(|(_, rate)| rate)
            .or(floor)?;
        Some(AirtimeCandidate {
            key: link.epoch,
            minimum_micros: self.ceiling.least(ready.length, rate),
        })
    }

    fn ceiling(&self, ready: &Ready) -> NonZeroU32 {
        self.ceiling.ceiling(ready.length)
    }

    /// A report keeps its frame's own epoch, and only one of the
    /// association that runs now counts: a late completion of an earlier
    /// association never changes the current one's rate.
    fn reported(&mut self, key: StationKey, rate: PhyRate) {
        if key == self.link.snapshot().epoch {
            self.last = Some((key, rate));
        }
    }
}

/// How a frame the station took ended, for its settlement.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum End {
    /// The station gave its buffer back; the cost its transmissions had and
    /// the rate of the last on the air.
    Returned(TxEnd),
    /// Lost with its buffer when the station took it: nothing comes back.
    Lost,
}

/// One frame's end, with the buffer it gave back if any.
struct Completed<B, T> {
    buffer: Option<B>,
    ticket: T,
    end: End,
}

/// The ends of the frames a station took, `N` at most: one per buffer it
/// can hold. Each buffer `B` comes back to its owner when its end is handed
/// on ([`Self::take_notice`]), with the epoch's change before it.
pub struct Completions<B, T, const N: usize> {
    ended: heapless::Deque<Completed<B, T>, N>,
    /// The link epoch the last notice was handed on under.
    epoch: Option<u64>,
}

impl<B, T, const N: usize> Default for Completions<B, T, N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<B, T, const N: usize> Completions<B, T, N> {
    pub const fn new() -> Self {
        Self {
            ended: heapless::Deque::new(),
            epoch: None,
        }
    }

    /// The station gave `buffer` back, its frame `ticket` ended so (`end`).
    ///
    /// # Errors
    /// The buffer, when `N` ends wait already: more buffers than this holds
    /// were taken, which the composition's sizes exclude.
    pub fn returned(&mut self, buffer: B, ticket: T, end: TxEnd) -> Result<(), B> {
        let completed = Completed {
            buffer: Some(buffer),
            ticket,
            end: End::Returned(end),
        };
        match self.ended.push_back(completed) {
            Ok(()) => Ok(()),
            Err(Completed {
                buffer: Some(buffer),
                ..
            }) => Err(buffer),
            // Pushed with its buffer, so it comes back with it.
            Err(Completed { buffer: None, .. }) => Ok(()),
        }
    }

    /// The station took the frame `ticket` and lost it with its buffer.
    ///
    /// # Errors
    /// `N` ends wait already: the composition's sizes exclude it.
    pub fn lost(&mut self, ticket: T) -> Result<(), T> {
        self.ended
            .push_back(Completed {
                buffer: None,
                ticket,
                end: End::Lost,
            })
            .map_err(|completed| completed.ticket)
    }

    /// The ends waiting to be handed on.
    pub fn len(&self) -> usize {
        self.ended.len()
    }

    /// Whether no end waits.
    pub fn is_empty(&self) -> bool {
        self.ended.is_empty()
    }

    /// Whether [`Self::take_notice`] has something for a link now `link`.
    pub fn waiting(&self, link: LinkSnapshot) -> bool {
        !self.ended.is_empty() || self.epoch.is_some_and(|epoch| epoch != link.epoch)
    }

    /// The next notice for the egress policy, the link now being `link`:
    /// first the end of the association a new epoch replaced, then each
    /// frame's end, with its buffer, which is the owner's again from here.
    pub fn take_notice(
        &mut self,
        link: LinkSnapshot,
    ) -> Option<(RadioNotice<T, StationKey>, Option<B>)> {
        let seen = self.epoch.replace(link.epoch);
        if let Some(old) = seen.filter(|old| *old != link.epoch) {
            return Some((RadioNotice::Left(old), None));
        }
        let Completed {
            buffer,
            ticket,
            end,
        } = self.ended.pop_front()?;
        let notice = match end {
            End::Returned(end) => RadioNotice::Ended(Exchange {
                ticket,
                charged: charged(end.cost),
                rate: end.rate,
            }),
            End::Lost => RadioNotice::Lost(ticket),
        };
        Some((notice, buffer))
    }
}

/// The charge of an end that cost `cost`: an estimate as it is, at least
/// the least nonzero charge, as a settlement is never free; an unknown cost
/// none, which the policy charges its ceiling.
const fn charged(cost: TxEstimate) -> Option<NonZeroU32> {
    match cost {
        TxEstimate::Estimated { micros } => match NonZeroU32::new(micros) {
            Some(micros) => Some(micros),
            None => Some(NonZeroU32::MIN),
        },
        TxEstimate::Unknown => None,
    }
}

#[cfg(test)]
mod tests;
