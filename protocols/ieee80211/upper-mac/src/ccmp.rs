//! CCMP packet-number allocation and receive replay owners.

#[cfg(test)]
use oer_ieee80211_wire::ccmp::CCMP_PACKET_NUMBER_MAX;

use oer_ieee80211_wire::ccmp::{
    CCMP_HEADER_LEN, CcmpHeader, CcmpKeyId, CcmpPacketNumber, CcmpPacketNumberStep,
    CcmpReceiveSequenceError, GroupKeyId,
};

mod replay;
pub use replay::{CcmpRxProtection, CcmpRxReplayEpoch, CcmpRxReplayError};

pub const CCMP_REPLAY_LANE_COUNT: usize = 17;

/// The 48-bit transmit PN space of one key is used up.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CcmpTxPacketNumberError {
    /// The next PN would exceed
    /// [`CCMP_PACKET_NUMBER_MAX`](oer_ieee80211_wire::ccmp::CCMP_PACKET_NUMBER_MAX); the key must be
    /// replaced.
    Exhausted,
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
    pub const fn new(step: CcmpPacketNumberStep) -> Self {
        Self {
            last: core::cell::Cell::new(CcmpPacketNumber::ZERO),
            step,
        }
    }

    /// An allocator whose last handed-out PN is `last`: the next MPDU
    /// carries `last + step`. A key whose counter lives on across an owner
    /// change continues from its last PN this way.
    pub const fn continuing_after(step: CcmpPacketNumberStep, last: CcmpPacketNumber) -> Self {
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
