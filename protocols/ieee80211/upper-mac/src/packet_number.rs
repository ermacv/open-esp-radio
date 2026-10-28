//! Typed monotonic packet-number arithmetic and borrowed commit frontiers.
//!
//! CCMP PNs and BIP IPNs use the same rules, but remain distinct value types.

/// A bounded 48-bit packet-number value, ordered by its unsigned integer.
pub trait PacketNumber48: Copy + Ord {
    /// Add a distance, refusing overflow or a result outside the 48-bit domain.
    fn checked_add(self, distance: u64) -> Option<Self>;
}

impl PacketNumber48 for oer_ieee80211_wire::ccmp::CcmpPacketNumber {
    fn checked_add(self, distance: u64) -> Option<Self> {
        self.value().checked_add(distance).and_then(Self::new)
    }
}

/// The key's 48-bit packet-number space cannot accommodate the advance.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PacketNumberExhausted;

/// Advance within the value's domain, without wrapping or changing an owner.
pub fn advance<N: PacketNumber48>(last: N, distance: u64) -> Result<N, PacketNumberExhausted> {
    last.checked_add(distance).ok_or(PacketNumberExhausted)
}

/// One non-copyable frontier, advanced only by committing its borrowed candidate.
#[derive(Debug, Eq, PartialEq)]
pub struct PacketNumberFrontier<N> {
    highest: N,
}

impl<N: PacketNumber48> PacketNumberFrontier<N> {
    pub const fn new(initial: N) -> Self {
        Self { highest: initial }
    }

    pub const fn highest(&self) -> N {
        self.highest
    }

    /// Refuse a repeat or rollback without changing the frontier. Dropping
    /// the returned candidate cancels admission; authentication precedes commit.
    pub fn prepare(
        &mut self,
        packet_number: N,
    ) -> Result<PacketNumberCandidate<'_, N>, Replayed<N>> {
        if packet_number <= self.highest {
            return Err(Replayed {
                packet_number,
                highest: self.highest,
            });
        }
        Ok(PacketNumberCandidate {
            frontier: &mut self.highest,
            packet_number,
        })
    }

    /// Reserve the next transmit number, committing only after protection succeeds.
    pub fn prepare_next(
        &mut self,
        step: core::num::NonZeroU8,
    ) -> Result<PacketNumberCandidate<'_, N>, PacketNumberExhausted> {
        let packet_number = advance(self.highest, u64::from(step.get()))?;
        Ok(PacketNumberCandidate {
            frontier: &mut self.highest,
            packet_number,
        })
    }
}

/// The refused number and the frontier it did not exceed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Replayed<N> {
    pub packet_number: N,
    pub highest: N,
}

/// A checked number exclusively borrowing the frontier it will advance.
#[derive(Debug)]
#[must_use = "commit after authentication/protection, or drop to cancel"]
pub struct PacketNumberCandidate<'a, N> {
    frontier: &'a mut N,
    packet_number: N,
}

impl<N: Copy> PacketNumberCandidate<'_, N> {
    pub const fn packet_number(&self) -> N {
        self.packet_number
    }

    pub fn commit(self) {
        *self.frontier = self.packet_number;
    }
}
