//! IEEE 802.11 sequence numbers (IEEE 802.11-2020 9.2.4.4.2): the checked
//! value of the Sequence Number subfield.
//!
//! Every constructor yields a twelve-bit value, so frame builders need no
//! range validation. The number's modulo-4096 sum, difference and distance
//! are its own operations; the MAC mechanisms' ordering and horizon
//! (`oer_protocols_ieee80211_core_upper_mac::sequence::SequenceSpace`)
//! are built on them.

/// One twelve-bit IEEE 802.11 sequence number.
///
/// Sequence numbers are modular, so the type deliberately has no ordering;
/// window decisions take the MAC's modular arithmetic.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub struct SequenceNumber(oer_portable_wire::U16<12>);

impl SequenceNumber {
    /// Size of the sequence-number space.
    pub const MODULUS: u16 = 4096;
    pub const ZERO: Self = Self(oer_portable_wire::U16::ZERO);
    pub const MAX: Self = Self(oer_portable_wire::U16::constant::<{ Self::MASK }>());

    const MASK: u16 = Self::MODULUS - 1;

    /// A sequence number, or `None` when `value` exceeds twelve bits.
    pub const fn new(value: u16) -> Option<Self> {
        match oer_portable_wire::U16::new(value) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }

    /// The Sequence Number subfield of a Sequence Control field.
    pub const fn from_sequence_control(field: SequenceControl) -> Self {
        Self(field.sequence())
    }

    pub const fn get(self) -> u16 {
        self.0.get()
    }

    /// The number `increment` numbers after this one.
    ///
    /// SOURCE: IEEE Std 802.11-2020, 9.2.4.4.2 (sequence numbers are
    /// assigned modulo 4096).
    #[must_use]
    pub const fn wrapping_add(self, increment: u16) -> Self {
        // u16 wrapping is congruent modulo this power-of-two space.
        Self::residue(self.get().wrapping_add(increment))
    }

    /// The number `decrement` numbers before this one; [`Self::wrapping_add`].
    #[must_use]
    pub const fn wrapping_sub(self, decrement: u16) -> Self {
        Self::residue(self.get().wrapping_sub(decrement))
    }

    /// The count of increments from this number forward to `later`, in
    /// `0..4096`.
    ///
    /// SOURCE: IEEE Std 802.11-2020, 10.24.7 (the modulo-4096 distance of
    /// two sequence numbers).
    pub const fn forward_distance(self, later: Self) -> u16 {
        Self::residue(later.get().wrapping_sub(self.get())).get()
    }

    /// `count` modulo 4096: the number in the low twelve bits of a
    /// sixteen-bit count, read as their field.
    const fn residue(count: u16) -> Self {
        Self(Residue::from_bytes([0; 2]).with_count(count).residue())
    }

    /// Sequence Control of an unfragmented MPDU (fragment number zero).
    pub const fn sequence_control(self) -> SequenceControl {
        SequenceControl::from_bytes([0; 2]).with_sequence(self.0)
    }
}

impl From<SequenceNumber> for u16 {
    fn from(sequence: SequenceNumber) -> Self {
        sequence.get()
    }
}

oer_portable_wire::bitfield! {
    /// A sixteen-bit count and its low twelve bits: its residue modulo the
    /// 4096 sequence numbers.
    /// SOURCE: IEEE Std 802.11-2020, 9.2.4.4.2 (sequence numbers modulo
    /// 4096).
    struct Residue[2] (little) {
        count, with_count: {u16} = 0..16, view;
        residue, with_residue: (u16) = 0..12;
    }
}

oer_portable_wire::bitfield! {
    /// SOURCE: IEEE Std 802.11-2020, 9.2.4.4 (Sequence Control field).
    pub struct SequenceControl[2] (little) {
        pub fragment, with_fragment: u8 = 0..4;
        pub sequence, with_sequence: (u16) = 4..16;
    }
}

#[cfg(test)]
mod tests;

/// A literal test sequence number; panics outside twelve bits.
#[cfg(test)]
pub(crate) const fn seq(value: u16) -> SequenceNumber {
    match SequenceNumber::new(value) {
        Some(sequence) => sequence,
        None => panic!("test sequence number exceeds twelve bits"),
    }
}
