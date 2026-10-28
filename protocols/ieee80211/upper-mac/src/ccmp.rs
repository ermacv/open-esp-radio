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

/// A transmit key and its unique PN owner. Construction installs the key,
/// through the lower-MAC port or into its software owner; an existing key
/// cannot be paired with a fresh counter. Removal consumes this wrapper.
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

    /// Number one protected MPDU: the key and its next CCMP packet number,
    /// or `None` once its packet numbers are exhausted. A retransmission
    /// keeps the number of its first transmission and does not call this.
    pub fn number(&self) -> Option<oer_ieee80211_port::NumberedKey<&K>> {
        let packet_number = self.packet_numbers.allocate().ok()?;
        Some(oer_ieee80211_port::NumberedKey {
            key: &self.key,
            packet_number,
        })
    }
}

/// Key material whose only owner is software: installing it means moving it
/// into that owner, as the ESP-NOW peer table owns its peers' local master
/// keys. A lower-MAC port key is not one; it pairs with its packet numbers
/// only through [`install_ccmp_pairwise_key`].
pub trait SoftwareCcmpKey {}

impl<K: SoftwareCcmpKey> InstalledCcmpTxKey<K> {
    /// Move new key material into its software owner together with a new
    /// PN allocator.
    pub const fn install(key: K, step: CcmpPacketNumberStep) -> Self {
        Self {
            key,
            packet_numbers: CcmpTxPacketNumber::new(step),
        }
    }

    /// Replace the key material and keep numbering after the last PN handed
    /// out, so the same key bytes supplied again never repeat a nonce.
    pub fn replace_key(&mut self, key: K) -> K {
        core::mem::replace(&mut self.key, key)
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

/// One strictly increasing receive packet-number frontier: a lane of a
/// pairwise or group key, the management frames of one association, or an
/// ESP-NOW peer's key.
///
/// A packet number is admitted only above the highest one already accepted
/// (IEEE Std 802.11-2020 12.5.3.4.4, PN and replay detection); a repeated or
/// earlier number is refused without changing the frontier. A new key starts
/// a new frontier from its receive sequence, so a frontier never moves back.
#[derive(Debug, Eq, PartialEq)]
pub struct CcmpReplayFrontier {
    highest: CcmpPacketNumber,
}

impl CcmpReplayFrontier {
    /// A frontier whose highest accepted packet number is `initial`.
    pub const fn new(initial: CcmpPacketNumber) -> Self {
        Self { highest: initial }
    }

    /// Check `packet_number` and lend this frontier until commit or
    /// cancellation. The borrow prevents replacement, removal and a second
    /// concurrent admission; no unbound candidate or revision counter is
    /// needed.
    pub fn prepare(
        &mut self,
        packet_number: CcmpPacketNumber,
    ) -> Result<CcmpReplayCandidate<'_>, CcmpReplayed> {
        let highest = self.highest;
        if packet_number <= highest {
            return Err(CcmpReplayed {
                packet_number,
                highest,
            });
        }
        Ok(CcmpReplayCandidate {
            frontier: &mut self.highest,
            packet_number,
        })
    }

    /// Check and advance at once, for a frame published as soon as it is
    /// authenticated.
    pub fn commit_immediate(&mut self, packet_number: CcmpPacketNumber) -> Result<(), CcmpReplayed> {
        self.prepare(packet_number)?.commit();
        Ok(())
    }

    /// The highest accepted packet number.
    pub const fn highest(&self) -> CcmpPacketNumber {
        self.highest
    }
}

impl Default for CcmpReplayFrontier {
    fn default() -> Self {
        Self::new(CcmpPacketNumber::ZERO)
    }
}

/// A packet number that does not exceed its frontier.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CcmpReplayed {
    /// The refused packet number.
    pub packet_number: CcmpPacketNumber,
    /// The highest packet number the frontier had accepted.
    pub highest: CcmpPacketNumber,
}

/// A checked packet number holding its exact replay frontier exclusively.
/// It cannot be copied, transferred to another key/association, or retained
/// across replacement of the borrowed key state. Dropping it cancels the
/// check without advancing the frontier.
#[derive(Debug)]
#[must_use = "commit before publication, or drop to cancel admission"]
pub struct CcmpReplayCandidate<'a> {
    frontier: &'a mut CcmpPacketNumber,
    packet_number: CcmpPacketNumber,
}

impl CcmpReplayCandidate<'_> {
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

/// A [`CcmpReplayCandidate`] of one lane of a key's receive frontiers.
#[derive(Debug)]
#[must_use = "commit before publication, or drop to cancel admission"]
pub struct CcmpRxReplayCandidate<'a> {
    candidate: CcmpReplayCandidate<'a>,
    lane: CcmpReplayLane,
}

impl CcmpRxReplayCandidate<'_> {
    /// The admitted non-QoS or TID lane.
    pub const fn lane(&self) -> CcmpReplayLane {
        self.lane
    }
    /// The already checked packet number.
    pub const fn packet_number(&self) -> CcmpPacketNumber {
        self.candidate.packet_number()
    }
    /// Publish once, advancing only the originally borrowed lane.
    pub fn commit(self) {
        self.candidate.commit();
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
    lanes: [CcmpReplayFrontier; CCMP_REPLAY_LANE_COUNT],
}

impl CcmpRxReplayState {
    pub const fn new(initial: CcmpPacketNumber) -> Self {
        Self {
            lanes: [const { CcmpReplayFrontier::new(CcmpPacketNumber::ZERO) };
                CCMP_REPLAY_LANE_COUNT],
        }
        .starting_at(initial)
    }

    const fn starting_at(mut self, initial: CcmpPacketNumber) -> Self {
        let mut remaining: &mut [CcmpReplayFrontier] = &mut self.lanes;
        while let [lane, rest @ ..] = remaining {
            *lane = CcmpReplayFrontier::new(initial);
            remaining = rest;
        }
        self
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
    pub fn prepare(
        &mut self,
        lane: CcmpReplayLane,
        packet_number: CcmpPacketNumber,
    ) -> Result<CcmpRxReplayCandidate<'_>, CcmpReplayError> {
        let candidate = lane
            .select_mut(&mut self.lanes)
            .ok_or(CcmpReplayError::InvalidTid)?
            .prepare(packet_number)?;
        Ok(CcmpRxReplayCandidate { candidate, lane })
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
        match lane.select(&self.lanes) {
            Some(frontier) => Some(frontier.highest()),
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

impl From<CcmpReplayed> for CcmpReplayError {
    fn from(
        CcmpReplayed {
            packet_number,
            highest,
        }: CcmpReplayed,
    ) -> Self {
        Self::Replayed {
            packet_number,
            highest,
        }
    }
}

#[cfg(test)]
mod tests;
