//! The arithmetic of the sequence space and the owner of one transmit
//! sequence space.

use oer_protocols_ieee80211_wire::sequence::SequenceNumber;

/// The modulo-4096 arithmetic of the sequence space.
///
/// SOURCE: IEEE Std 802.11-2020, 9.2.4.4.2 (sequence numbers wrap modulo
/// 4096) and 10.24.7 (modular sequence distance and the half-space horizon).
pub trait SequenceSpace: Sized {
    /// Half of the sequence-number space: the comparison horizon.
    const HALF_SPACE: u16;

    /// The following sequence number.
    fn next(self) -> Self;
    /// Whether `self` strictly precedes `later` in modulo-4096 order: the
    /// forward distance is nonzero and below [`Self::HALF_SPACE`].
    fn precedes(self, later: Self) -> bool;
}

impl SequenceSpace for SequenceNumber {
    const HALF_SPACE: u16 = SequenceNumber::MODULUS / 2;

    fn next(self) -> Self {
        next(self)
    }

    fn precedes(self, later: Self) -> bool {
        let distance = self.forward_distance(later);
        distance != 0 && distance < Self::HALF_SPACE
    }
}

/// The following sequence number, for a constant context.
const fn next(sequence: SequenceNumber) -> SequenceNumber {
    sequence.wrapping_add(1)
}

/// Monotonic twelve-bit owner for one IEEE 802.11 transmit sequence space.
///
/// A sequence number is consumed for every newly encoded MPDU. Hardware
/// retries retain the already encoded header and therefore do not call
/// [`Self::take`].
///
/// This value deliberately represents only one space. Management/non-QoS
/// traffic and every QoS TID have independent counters.
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

#[cfg(test)]
mod tests;
