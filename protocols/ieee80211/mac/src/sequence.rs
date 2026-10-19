//! IEEE 802.11 sequence numbers (IEEE 802.11-2020 9.2.4.4.2) and their
//! modulo-4096 arithmetic.
//!
//! Every constructor yields a twelve-bit value, so frame builders need no
//! range validation and window arithmetic cannot forget to wrap.

/// One twelve-bit IEEE 802.11 sequence number.
///
/// Sequence numbers are modular, so the type deliberately has no ordering;
/// use [`Self::forward_distance`] or [`Self::precedes`] for window decisions.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub struct SequenceNumber(u16);

impl SequenceNumber {
    /// Size of the sequence-number space.
    pub const MODULUS: u16 = 4096;
    /// Half of the sequence-number space: the comparison horizon of
    /// IEEE 802.11-2020 10.24.7.
    pub const HALF_SPACE: u16 = Self::MODULUS / 2;
    pub const ZERO: Self = Self(0);
    pub const MAX: Self = Self(Self::MASK);

    const MASK: u16 = Self::MODULUS - 1;

    /// A sequence number, or `None` when `value` exceeds twelve bits.
    pub const fn new(value: u16) -> Option<Self> {
        if value <= Self::MASK {
            Some(Self(value))
        } else {
            None
        }
    }

    /// The low twelve bits of `value`, for entropy seeds and free-running
    /// counters whose wider bits are not part of the sequence space.
    pub const fn from_low_bits(value: u16) -> Self {
        Self(value & Self::MASK)
    }

    /// The Sequence Number subfield of a Sequence Control field.
    pub const fn from_sequence_control(sequence_control: u16) -> Self {
        Self(sequence_control >> 4)
    }

    pub const fn get(self) -> u16 {
        self.0
    }

    /// Sequence Control of an unfragmented MPDU (fragment number zero).
    pub const fn sequence_control(self) -> u16 {
        self.0 << 4
    }

    /// Sequence Control with an explicit four-bit fragment number.
    pub const fn sequence_control_with_fragment(self, fragment: u8) -> u16 {
        (self.0 << 4) | (fragment as u16 & 0x000f)
    }

    /// The following sequence number.
    pub const fn next(self) -> Self {
        self.wrapping_add(1)
    }

    pub const fn wrapping_add(self, increment: u16) -> Self {
        Self(self.0.wrapping_add(increment) & Self::MASK)
    }

    pub const fn wrapping_sub(self, decrement: u16) -> Self {
        Self(self.0.wrapping_sub(decrement) & Self::MASK)
    }

    /// Number of increments from `self` forward to `later`, in `0..4096`.
    pub const fn forward_distance(self, later: Self) -> u16 {
        later.0.wrapping_sub(self.0) & Self::MASK
    }

    /// Whether `self` strictly precedes `later` in modulo-4096 order: the
    /// forward distance is nonzero and below [`Self::HALF_SPACE`].
    pub const fn precedes(self, later: Self) -> bool {
        let distance = self.forward_distance(later);
        distance != 0 && distance < Self::HALF_SPACE
    }
}

impl From<SequenceNumber> for u16 {
    fn from(sequence: SequenceNumber) -> Self {
        sequence.get()
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
