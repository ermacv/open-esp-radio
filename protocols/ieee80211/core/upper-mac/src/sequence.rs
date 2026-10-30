//! Separate management/non-QoS and per-TID transmit sequence owners, shared
//! by the station, the access point and ESP-NOW.

use oer_ieee80211_wire::sequence::SequenceNumber;

/// The modulo-4096 arithmetic of the sequence space.
///
/// SOURCE: IEEE Std 802.11-2020, 9.2.4.4.2 (sequence numbers wrap modulo
/// 4096) and 10.24.7 (modular sequence distance and the half-space horizon).
pub trait SequenceSpace: Sized {
    /// Half of the sequence-number space: the comparison horizon.
    const HALF_SPACE: u16;

    /// The following sequence number.
    fn next(self) -> Self;
    /// `increment` numbers forward.
    fn wrapping_add(self, increment: u16) -> Self;
    /// `decrement` numbers back.
    fn wrapping_sub(self, decrement: u16) -> Self;
    /// Number of increments from `self` forward to `later`, in `0..4096`.
    fn forward_distance(self, later: Self) -> u16;
    /// Whether `self` strictly precedes `later` in modulo-4096 order: the
    /// forward distance is nonzero and below [`Self::HALF_SPACE`].
    fn precedes(self, later: Self) -> bool;
}

impl SequenceSpace for SequenceNumber {
    const HALF_SPACE: u16 = SequenceNumber::MODULUS / 2;

    fn next(self) -> Self {
        next(self)
    }

    fn wrapping_add(self, increment: u16) -> Self {
        wrapping_add(self, increment)
    }

    fn wrapping_sub(self, decrement: u16) -> Self {
        SequenceNumber::from_low_bits(self.get().wrapping_sub(decrement))
    }

    fn forward_distance(self, later: Self) -> u16 {
        forward_distance(self, later)
    }

    fn precedes(self, later: Self) -> bool {
        let distance = self.forward_distance(later);
        distance != 0 && distance < Self::HALF_SPACE
    }
}

/// The following sequence number, for a constant context.
const fn next(sequence: SequenceNumber) -> SequenceNumber {
    wrapping_add(sequence, 1)
}

/// [`SequenceSpace::wrapping_add`] for a constant context.
// u16 wrapping is congruent modulo this power-of-two space.
pub const fn wrapping_add(sequence: SequenceNumber, increment: u16) -> SequenceNumber {
    SequenceNumber::from_low_bits(sequence.get().wrapping_add(increment))
}

/// [`SequenceSpace::forward_distance`] for a constant context.
pub const fn forward_distance(earlier: SequenceNumber, later: SequenceNumber) -> u16 {
    SequenceNumber::from_low_bits(later.get().wrapping_sub(earlier.get())).get()
}

/// Monotonic twelve-bit owner for one IEEE 802.11 transmit sequence space.
///
/// A sequence number is consumed for every newly encoded MPDU. Hardware
/// retries retain the already encoded header and therefore do not call
/// [`Self::take`].
///
/// This value deliberately represents only one space. Management/non-QoS
/// traffic and every QoS TID have independent counters, owned together by
/// [`TxSequenceCounters`].
///
/// SOURCE(esp32s31): complete `libnet80211.a[ieee80211_ht.o]::
/// ieee80211_ampdu_request` instructions 0x9a..0xa2 load the AddBA Starting
/// Sequence Number from the node's TID-indexed halfword at
/// `(tid + 0x50) * 2 + 0x0e`. The captured open-driver AddBA/action exchange
/// `HIL_OPEN_STA_SEQUENCE_SPACES_2026_07_30` demonstrated why an
/// interface-global counter is wrong: three action frames advanced the TID0
/// SSN before its first QoS MPDU. `libpp.a[pp.o]` keeps a hardware
/// retry attached to the already encoded frame, so retries do not consume a
/// new protocol sequence number.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SequenceCounter {
    next: SequenceNumber,
}

impl SequenceCounter {
    pub const fn new(first: SequenceNumber) -> Self {
        Self { next: first }
    }

    /// Consume the next sequence number, wrapping in the 802.11 twelve-bit
    /// sequence space.
    pub const fn take(&mut self) -> SequenceNumber {
        let sequence = self.next;
        self.next = next(sequence);
        sequence
    }

    pub const fn peek(&self) -> SequenceNumber {
        self.next
    }
}

/// Allocation-free owner of all transmit sequence-number spaces of one peer.
///
/// IEEE 802.11 QoS traffic has one sequence space per TID. Management and
/// non-QoS data use the separate non-QoS space. Keeping the counters behind
/// this owner makes it impossible for an AddBA Action frame to silently
/// advance the SSN advertised for a QoS agreement.
///
/// The same initial value is safe for every entry because the spaces are
/// independent; callers may seed it from per-association entropy so a reset
/// does not reproduce the previous peer epoch's initial values.
#[derive(Debug, Eq, PartialEq)]
pub struct TxSequenceCounters {
    non_qos: SequenceCounter,
    qos: [SequenceCounter; 16],
}

impl TxSequenceCounters {
    pub const QOS_TID_COUNT: u8 = 16;

    pub const fn new(first: SequenceNumber) -> Self {
        let counter = SequenceCounter::new(first);
        Self {
            non_qos: counter,
            qos: [counter; Self::QOS_TID_COUNT as usize],
        }
    }

    /// Borrow the management/non-QoS sequence-number owner.
    pub const fn non_qos_mut(&mut self) -> &mut SequenceCounter {
        &mut self.non_qos
    }

    pub const fn peek_non_qos(&self) -> SequenceNumber {
        self.non_qos.peek()
    }

    pub const fn take_non_qos(&mut self) -> SequenceNumber {
        self.non_qos.take()
    }

    /// Borrow one QoS/TID sequence-number owner.
    pub fn qos_mut(&mut self, tid: u8) -> Option<&mut SequenceCounter> {
        self.qos.get_mut(usize::from(tid))
    }

    pub fn peek_qos(&self, tid: u8) -> Option<SequenceNumber> {
        self.qos.get(usize::from(tid)).map(SequenceCounter::peek)
    }

    pub fn take_qos(&mut self, tid: u8) -> Option<SequenceNumber> {
        self.qos_mut(tid).map(SequenceCounter::take)
    }

    /// Consume a data-frame sequence number from the wire-format-selected
    /// space: `None` for a non-QoS header, or `Some(tid)` for a QoS header.
    pub fn take_data(&mut self, qos_tid: Option<u8>) -> Option<SequenceNumber> {
        match qos_tid {
            Some(tid) => self.take_qos(tid),
            None => Some(self.take_non_qos()),
        }
    }
}

#[cfg(test)]
mod tests;
