//! Hardware-independent CCMP key material, headers, transmit packet numbers
//! and receive replay ownership.
//!
//! The lower-MAC backend allocates packet numbers below its port; key-slot
//! mapping and hardware publication belong to the concrete MAC backend. This module validates and
//! encodes the public eight-byte header, allocates each key's transmit packet
//! numbers ([`CcmpTxPacketNumber`]) and owns a two-phase receive replay
//! frontier. A caller must prepare replay only after its per-TID BlockAck
//! reorder release, then commit only after cryptographic authentication and
//! peer admission.
//!
//! [`Ccmp128Key`] is the shared zeroizing temporal-key owner. RSN constructs
//! it from a borrowed PTK or GTK and moves it into lower-MAC installation;
//! software CCMP and hardware programming borrow the same owner's bytes.

use core::num::NonZeroU8;

mod key;
mod replay;

pub use key::Ccmp128Key;
pub use replay::{CcmpRxProtection, CcmpRxReplayEpoch, CcmpRxReplayError};

pub const CCMP_HEADER_LEN: usize = 8;
pub const CCMP_PACKET_NUMBER_MAX: u64 = (1_u64 << 48) - 1;
pub const CCMP_REPLAY_LANE_COUNT: usize = 17;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct CcmpPacketNumber(u64);

impl CcmpPacketNumber {
    pub const ZERO: Self = Self(0);

    pub const fn new(value: u64) -> Option<Self> {
        if value <= CCMP_PACKET_NUMBER_MAX {
            Some(Self(value))
        } else {
            None
        }
    }

    pub const fn value(self) -> u64 {
        self.0
    }

    pub const fn from_header_bytes(bytes: [u8; CCMP_HEADER_LEN]) -> Self {
        Self(
            (bytes[0] as u64)
                | ((bytes[1] as u64) << 8)
                | ((bytes[4] as u64) << 16)
                | ((bytes[5] as u64) << 24)
                | ((bytes[6] as u64) << 32)
                | ((bytes[7] as u64) << 40),
        )
    }

    /// Decode the little-endian six-octet packet number carried by an EAPOL
    /// Key RSC. The two unused high octets must be zero; accepting them would
    /// silently truncate an install-time replay frontier.
    pub const fn from_receive_sequence(
        sequence: [u8; 8],
    ) -> Result<Self, CcmpReceiveSequenceError> {
        if sequence[6] != 0 || sequence[7] != 0 {
            return Err(CcmpReceiveSequenceError::HighBitsSet);
        }
        Ok(Self(
            (sequence[0] as u64)
                | ((sequence[1] as u64) << 8)
                | ((sequence[2] as u64) << 16)
                | ((sequence[3] as u64) << 24)
                | ((sequence[4] as u64) << 32)
                | ((sequence[5] as u64) << 40),
        ))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CcmpReceiveSequenceError {
    HighBitsSet,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CcmpKeyId(u8);

impl CcmpKeyId {
    pub const PAIRWISE: Self = Self(0);

    pub const fn new(value: u8) -> Option<Self> {
        if value <= 3 { Some(Self(value)) } else { None }
    }

    pub const fn value(self) -> u8 {
        self.0
    }

    pub const fn header_bits(self) -> u8 {
        self.0 << 6
    }
}

/// The two-bit wire identifier of a GTK, 0 through 3.
///
/// IEEE 802.11-2020, 12.7.2 (EAPOL-Key frames), defines the GTK KDE's
/// two-bit KeyID. A GTK with ID zero does not name the pairwise key:
/// receive selection distinguishes pairwise and group frame addressing.
/// Hardware slot restrictions belong to the backend, separately from this
/// wire value.
///
/// ```
/// # use oer_ieee80211_mac::ccmp::GroupKeyId;
/// assert_eq!(GroupKeyId::new(0).map(GroupKeyId::get), Some(0));
/// assert_eq!(GroupKeyId::new(1).map(GroupKeyId::get), Some(1));
/// assert_eq!(GroupKeyId::new(3).map(GroupKeyId::get), Some(3));
/// assert!(GroupKeyId::new(4).is_none());
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GroupKeyId(GroupIndex);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum GroupIndex {
    Zero,
    One,
    Two,
    Three,
}

impl GroupKeyId {
    /// `key_id`, when it fits the GTK KDE's two-bit field.
    pub const fn new(key_id: u8) -> Option<Self> {
        match key_id {
            0 => Some(Self(GroupIndex::Zero)),
            1 => Some(Self(GroupIndex::One)),
            2 => Some(Self(GroupIndex::Two)),
            3 => Some(Self(GroupIndex::Three)),
            _ => None,
        }
    }

    /// The identifier.
    pub const fn get(self) -> u8 {
        match self.0 {
            GroupIndex::Zero => 0,
            GroupIndex::One => 1,
            GroupIndex::Two => 2,
            GroupIndex::Three => 3,
        }
    }

    /// The identifier, as a CCMP header names it.
    pub const fn key_id(self) -> CcmpKeyId {
        CcmpKeyId(self.get())
    }

    /// Select the record for this wire ID, independently of hardware slots.
    pub const fn select<T>(self, entries: &[T; 4]) -> &T {
        let [zero, one, two, three] = entries;
        match self.0 {
            GroupIndex::Zero => zero,
            GroupIndex::One => one,
            GroupIndex::Two => two,
            GroupIndex::Three => three,
        }
    }

    /// Select the mutable record for this wire ID.
    pub const fn select_mut<T>(self, entries: &mut [T; 4]) -> &mut T {
        let [zero, one, two, three] = entries;
        match self.0 {
            GroupIndex::Zero => zero,
            GroupIndex::One => one,
            GroupIndex::Two => two,
            GroupIndex::Three => three,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CcmpHeader {
    packet_number: CcmpPacketNumber,
    key_id: CcmpKeyId,
}

impl CcmpHeader {
    pub const fn new(packet_number: CcmpPacketNumber, key_id: CcmpKeyId) -> Self {
        Self {
            packet_number,
            key_id,
        }
    }

    pub const fn parse(bytes: [u8; CCMP_HEADER_LEN]) -> Result<Self, CcmpHeaderError> {
        if bytes[2] != 0 {
            return Err(CcmpHeaderError::ReservedOctet);
        }
        if bytes[3] & 0x20 == 0 {
            return Err(CcmpHeaderError::ExtIvMissing);
        }
        if bytes[3] & 0x1f != 0 {
            return Err(CcmpHeaderError::ReservedKeyBits);
        }
        Ok(Self {
            packet_number: CcmpPacketNumber::from_header_bytes(bytes),
            key_id: CcmpKeyId((bytes[3] >> 6) & 0x03),
        })
    }

    pub const fn encode(self) -> [u8; CCMP_HEADER_LEN] {
        let packet_number = self.packet_number.value();
        [
            packet_number as u8,
            (packet_number >> 8) as u8,
            0,
            self.key_id.header_bits() | 0x20,
            (packet_number >> 16) as u8,
            (packet_number >> 24) as u8,
            (packet_number >> 32) as u8,
            (packet_number >> 40) as u8,
        ]
    }

    pub const fn packet_number(self) -> CcmpPacketNumber {
        self.packet_number
    }

    pub const fn key_id(self) -> CcmpKeyId {
        self.key_id
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CcmpHeaderError {
    ReservedOctet,
    ExtIvMissing,
    ReservedKeyBits,
}

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

/// The 48-bit transmit PN space of one key is used up.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CcmpTxPacketNumberError {
    /// The next PN would exceed [`CCMP_PACKET_NUMBER_MAX`]; the key must be
    /// replaced.
    Exhausted,
}

/// The unique allocator of one installed key's transmit packet numbers.
///
/// A new key has emitted no PN; its first MPDU carries `step`. Each call
/// hands out the next PN exactly once, so a retransmission of the same MPDU
/// reuses the header it was first encoded with instead of allocating again.
/// Once [`CCMP_PACKET_NUMBER_MAX`] could be exceeded, every later call fails
/// without wrapping or changing state.
#[derive(Debug, Eq, PartialEq)]
pub struct CcmpTxPacketNumber {
    last: u64,
    step: CcmpPacketNumberStep,
}

impl CcmpTxPacketNumber {
    pub const fn new(step: CcmpPacketNumberStep) -> Self {
        Self { last: 0, step }
    }

    /// An allocator whose last handed-out PN is `last`: the next MPDU
    /// carries `last + step`. A key whose counter lives on across an owner
    /// change continues from its last PN this way.
    pub const fn continuing_after(step: CcmpPacketNumberStep, last: CcmpPacketNumber) -> Self {
        Self {
            last: last.value(),
            step,
        }
    }

    /// The last PN handed out; zero before the first.
    pub const fn last(&self) -> CcmpPacketNumber {
        CcmpPacketNumber(self.last)
    }

    pub const fn step(&self) -> CcmpPacketNumberStep {
        self.step
    }

    /// Allocate the PN of the next MPDU.
    pub fn allocate(&mut self) -> Result<CcmpPacketNumber, CcmpTxPacketNumberError> {
        let next = self.last + u64::from(self.step.get());
        if next > CCMP_PACKET_NUMBER_MAX {
            return Err(CcmpTxPacketNumberError::Exhausted);
        }
        self.last = next;
        Ok(CcmpPacketNumber(next))
    }

    /// Allocate the PN of the next MPDU and encode its CCMP header.
    pub fn next_header(
        &mut self,
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
    const fn index(self) -> Option<usize> {
        match self {
            Self::NonQos => Some(0),
            Self::Tid(tid) if tid < 16 => Some(tid as usize + 1),
            Self::Tid(_) => None,
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
        let index = lane.index().ok_or(CcmpReplayError::InvalidTid)?;
        let highest = self.highest[index];
        if packet_number <= highest {
            return Err(CcmpReplayError::Replayed {
                packet_number,
                highest,
            });
        }
        Ok(CcmpRxReplayCandidate {
            frontier: &mut self.highest[index],
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
        match lane.index() {
            Some(index) => Some(self.highest[index]),
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

pub const fn ccmp_header(low: u32, high: u32, key_id_bits: u8) -> [u8; CCMP_HEADER_LEN] {
    [
        low as u8,
        (low >> 8) as u8,
        0,
        key_id_bits | 0x20,
        (low >> 16) as u8,
        (low >> 24) as u8,
        high as u8,
        (high >> 8) as u8,
    ]
}

#[cfg(test)]
mod tests;
