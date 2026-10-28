//! CCMP packet-number allocation and receive replay owners.

#[cfg(test)]
use oer_ieee80211_wire::ccmp::CCMP_PACKET_NUMBER_MAX;

use core::num::NonZeroU8;

use oer_ieee80211_wire::ccmp::{
    CCMP_HEADER_LEN, CcmpHeader, CcmpKeyId, CcmpPacketNumber, CcmpReceiveSequenceError, GroupKeyId,
};

mod replay;
pub use replay::{CcmpRxProtection, CcmpRxReplayEpoch, CcmpRxReplayError};

/// How far a key's transmit packet number advances per MPDU.
///
/// IEEE Std 802.11-2020 12.5.3.3.2 (PN processing) requires only that the PN of each MPDU
/// under one key increases, and that the fragments of one MSDU take
/// consecutive PNs; one is the ordinary step. A backend or vendor profile
/// that must stay compatible with a peer implementation's numbering supplies
/// its own step: the Espressif `net80211` stack advances by three
/// (`oer-espressif-ieee80211-policy`).
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct CcmpPacketNumberStep(NonZeroU8);

impl CcmpPacketNumberStep {
    /// The ordinary step of one.
    pub const ONE: Self = Self(NonZeroU8::MIN);

    /// A step of `step`; `None` for zero, which would repeat a PN.
    pub const fn new(step: u8) -> Option<Self> {
        match NonZeroU8::new(step) {
            Some(step) => Some(Self(step)),
            None => None,
        }
    }

    pub const fn get(self) -> u8 {
        self.0.get()
    }
}

pub const CCMP_REPLAY_LANE_COUNT: usize = 17;

/// The 48-bit transmit PN space of one key is used up.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CcmpTxPacketNumberError {
    /// The next PN would exceed
    /// [`CCMP_PACKET_NUMBER_MAX`](oer_ieee80211_wire::ccmp::CCMP_PACKET_NUMBER_MAX); the key must be
    /// replaced.
    Exhausted,
}

/// A backend pairwise key and its unique transmit PN owner. Construction
/// performs the actual port installation; an existing key cannot be paired
/// with a fresh counter. Removal consumes this wrapper.
pub struct InstalledCcmpTxKey<K> {
    key: K,
    packet_numbers: CcmpTxPacketNumber,
}

impl<K> InstalledCcmpTxKey<K> {
    pub const fn key(&self) -> &K {
        &self.key
    }
    pub const fn packet_numbers(&self) -> &CcmpTxPacketNumber {
        &self.packet_numbers
    }
    pub fn into_key(self) -> K {
        self.key
    }
}

/// Install new key material and create its PN allocator in the same operation.
/// This does not accept an already installed backend key or an initial PN.
pub fn install_ccmp_pairwise_key<'p, 'id: 'p, P>(
    phase: &mut P,
    association: &<oer_ieee80211_port::InterfaceKeys<'p, 'id, P> as oer_ieee80211_port::KeyTypes<'id>>::Association,
    slot: <oer_ieee80211_port::InterfaceKeys<'p, 'id, P> as oer_ieee80211_port::KeyTypes<'id>>::PairwiseSlot,
    peer: oer_ieee80211_wire::address::IndividualAddress,
    key: oer_ieee80211_wire::ccmp::Ccmp128Key,
    step: CcmpPacketNumberStep,
) -> InstalledCcmpTxKey<<oer_ieee80211_port::InterfaceKeys<'p, 'id, P> as oer_ieee80211_port::KeyTypes<'id>>::PairwiseKey>
where
    P: oer_ieee80211_port::InterfacePhase<'p, 'id>,
{
    InstalledCcmpTxKey {
        key: phase.install_pairwise_key(association, slot, peer, key),
        packet_numbers: CcmpTxPacketNumber::new(step),
    }
}

/// The unique allocator of one installed key's transmit packet numbers.
///
/// A new key has emitted no PN; its first MPDU carries `step`. Each call
/// hands out the next PN exactly once, so a retransmission of the same MPDU
/// reuses the header it was first encoded with instead of allocating again.
/// Once [`CCMP_PACKET_NUMBER_MAX`](oer_ieee80211_wire::ccmp::CCMP_PACKET_NUMBER_MAX) could be exceeded, every later call fails
/// without wrapping or changing state.
#[derive(Debug, Eq, PartialEq)]
pub struct CcmpTxPacketNumber {
    last: core::cell::Cell<CcmpPacketNumber>,
    step: CcmpPacketNumberStep,
}

impl CcmpTxPacketNumber {
    const fn new(step: CcmpPacketNumberStep) -> Self {
        Self {
            last: core::cell::Cell::new(CcmpPacketNumber::ZERO),
            step,
        }
    }

    /// An allocator whose last handed-out PN is `last`: the next MPDU
    /// carries `last + step`. A key whose counter lives on across an owner
    /// change continues from its last PN this way.
    #[cfg(test)]
    const fn continuing_after(step: CcmpPacketNumberStep, last: CcmpPacketNumber) -> Self {
        Self {
            last: core::cell::Cell::new(last),
            step,
        }
    }

    /// The last PN handed out; zero before the first.
    pub fn last(&self) -> CcmpPacketNumber {
        self.last.get()
    }

    pub const fn step(&self) -> CcmpPacketNumberStep {
        self.step
    }

    /// Allocate the PN of the next MPDU.
    pub fn allocate(&self) -> Result<CcmpPacketNumber, CcmpTxPacketNumberError> {
        // `last` is at most CCMP_PACKET_NUMBER_MAX (2^48 - 1) and the step at
        // most 255, so the sum stays far below u64::MAX.
        let next = self
            .last
            .get()
            .value()
            .saturating_add(u64::from(self.step.get()));
        let Some(next) = CcmpPacketNumber::new(next) else {
            return Err(CcmpTxPacketNumberError::Exhausted);
        };
        self.last.set(next);
        Ok(next)
    }

    /// Reserve all fresh MPDU numbers of a batch before handing it to a backend.
    /// On exhaustion no number is consumed, including when only a prefix fits.
    pub fn allocate_batch<const N: usize>(
        &self,
    ) -> Result<[CcmpPacketNumber; N], CcmpTxPacketNumberError> {
        let count = u64::try_from(N).map_err(|_| CcmpTxPacketNumberError::Exhausted)?;
        let distance = count
            .checked_mul(u64::from(self.step.get()))
            .ok_or(CcmpTxPacketNumberError::Exhausted)?;
        let final_number = self
            .last
            .get()
            .value()
            .checked_add(distance)
            .and_then(CcmpPacketNumber::new)
            .ok_or(CcmpTxPacketNumberError::Exhausted)?;
        let mut numbers = [CcmpPacketNumber::ZERO; N];
        let mut current = self.last.get().value();
        for number in &mut numbers {
            current = current.saturating_add(u64::from(self.step.get()));
            *number = CcmpPacketNumber::new(current).ok_or(CcmpTxPacketNumberError::Exhausted)?;
        }
        self.last.set(final_number);
        Ok(numbers)
    }

    /// Allocate the PN of the next MPDU and encode its CCMP header.
    pub fn next_header(
        &self,
        key_id: CcmpKeyId,
    ) -> Result<[u8; CCMP_HEADER_LEN], CcmpTxPacketNumberError> {
        let packet_number = self.allocate()?;
        Ok(CcmpHeader::new(packet_number, key_id).encode())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CcmpReplayLane {
    NonQos,
    Tid(u8),
}

impl CcmpReplayLane {
    /// This lane's entry among the non-QoS lane and the 16 TID lanes; `None`
    /// for a TID above 15.
    const fn select<T>(self, lanes: &[T; CCMP_REPLAY_LANE_COUNT]) -> Option<&T> {
        let [non_qos, tids @ ..] = lanes;
        match self {
            Self::NonQos => Some(non_qos),
            Self::Tid(tid) => match tids.split_at_checked(tid as usize) {
                Some((_, [lane, ..])) => Some(lane),
                _ => None,
            },
        }
    }

    /// The mutable entry of [`Self::select`].
    fn select_mut<T>(self, lanes: &mut [T; CCMP_REPLAY_LANE_COUNT]) -> Option<&mut T> {
        let [non_qos, tids @ ..] = lanes;
        match self {
            Self::NonQos => Some(non_qos),
            Self::Tid(tid) => tids.get_mut(usize::from(tid)),
        }
    }
}

/// A checked packet number holding its exact replay frontier exclusively.
/// It cannot be copied, transferred to another key/association, or retained
/// across replacement of the borrowed key state. Dropping it cancels the
/// check without advancing the frontier.
#[derive(Debug)]
#[must_use = "commit before publication, or drop to cancel admission"]
pub struct CcmpRxReplayCandidate<'a> {
    frontier: &'a mut CcmpPacketNumber,
    lane: CcmpReplayLane,
    packet_number: CcmpPacketNumber,
}

impl CcmpRxReplayCandidate<'_> {
    /// The admitted non-QoS or TID lane.
    pub const fn lane(&self) -> CcmpReplayLane {
        self.lane
    }
    /// The already checked packet number.
    pub const fn packet_number(&self) -> CcmpPacketNumber {
        self.packet_number
    }
    /// Publish once, advancing only the originally borrowed frontier.
    /// No other check or key replacement can have made this token stale.
    pub fn commit(self) {
        *self.frontier = self.packet_number;
    }
}

/// Per-key CCMP receive frontiers for non-QoS and TID 0 through 15.
///
/// This is deliberately not a sliding arrival-order window. The 802.11
/// sequence-space owner must first release frames in order within each TID;
/// only then is a strictly increasing CCMP PN meaningful. Separate lanes keep
/// unrelated access categories from rejecting each other's valid PNs.
#[derive(Debug, Eq, PartialEq)]
pub struct CcmpRxReplayState {
    highest: [CcmpPacketNumber; CCMP_REPLAY_LANE_COUNT],
}

impl CcmpRxReplayState {
    pub const fn new(initial: CcmpPacketNumber) -> Self {
        Self {
            highest: [initial; CCMP_REPLAY_LANE_COUNT],
        }
    }

    pub const fn from_receive_sequence(
        sequence: [u8; 8],
    ) -> Result<Self, CcmpReceiveSequenceError> {
        match CcmpPacketNumber::from_receive_sequence(sequence) {
            Ok(initial) => Ok(Self::new(initial)),
            Err(error) => Err(error),
        }
    }

    /// Check one lane and lend its frontier until commit or cancellation.
    /// The borrow prevents replacement, removal and a second concurrent
    /// admission; no unbound candidate or lane-revision counter is needed.
    pub fn prepare(
        &mut self,
        lane: CcmpReplayLane,
        packet_number: CcmpPacketNumber,
    ) -> Result<CcmpRxReplayCandidate<'_>, CcmpReplayError> {
        let frontier = lane
            .select_mut(&mut self.highest)
            .ok_or(CcmpReplayError::InvalidTid)?;
        let highest = *frontier;
        if packet_number <= highest {
            return Err(CcmpReplayError::Replayed {
                packet_number,
                highest,
            });
        }
        Ok(CcmpRxReplayCandidate {
            frontier,
            lane,
            packet_number,
        })
    }

    /// Check and advance the authenticated packet number under the same
    /// exclusive replay owner, before publishing the frame to its consumer.
    #[inline(always)]
    pub fn commit_immediate(
        &mut self,
        lane: CcmpReplayLane,
        packet_number: CcmpPacketNumber,
    ) -> Result<(), CcmpReplayError> {
        self.prepare(lane, packet_number)?.commit();
        Ok(())
    }

    pub const fn highest(&self, lane: CcmpReplayLane) -> Option<CcmpPacketNumber> {
        match lane.select(&self.highest) {
            Some(highest) => Some(*highest),
            None => None,
        }
    }
}

impl Default for CcmpRxReplayState {
    fn default() -> Self {
        Self::new(CcmpPacketNumber::ZERO)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CcmpReplayError {
    InvalidTid,
    Replayed {
        packet_number: CcmpPacketNumber,
        highest: CcmpPacketNumber,
    },
}

#[cfg(test)]
mod tests;
