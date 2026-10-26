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
pub struct SequenceNumber(oer_wire::U16<12>);

impl SequenceNumber {
    /// Size of the sequence-number space.
    pub const MODULUS: u16 = 4096;
    /// Half of the sequence-number space: the comparison horizon of
    /// IEEE 802.11-2020 10.24.7.
    pub const HALF_SPACE: u16 = Self::MODULUS / 2;
    pub const ZERO: Self = Self(oer_wire::U16::ZERO);
    pub const MAX: Self = Self(oer_wire::U16::constant::<{ Self::MASK }>());

    const MASK: u16 = Self::MODULUS - 1;

    /// A sequence number, or `None` when `value` exceeds twelve bits.
    pub const fn new(value: u16) -> Option<Self> {
        match oer_wire::U16::new(value) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }

    /// The low twelve bits of `value`, for entropy seeds and free-running
    /// counters whose wider bits are not part of the sequence space.
    pub const fn from_low_bits(value: u16) -> Self {
        Self(oer_wire::U16::from_low_bits(value))
    }

    /// The Sequence Number subfield of a Sequence Control field.
    pub const fn from_sequence_control(sequence_control: u16) -> Self {
        Self(SequenceControl::from_bytes(sequence_control.to_le_bytes()).sequence())
    }

    pub const fn get(self) -> u16 {
        self.0.get()
    }

    /// Sequence Control of an unfragmented MPDU (fragment number zero).
    pub const fn sequence_control(self) -> u16 {
        u16::from_le_bytes(
            SequenceControl::from_bytes([0; 2])
                .with_sequence(self.0)
                .to_bytes(),
        )
    }

    /// Sequence Control with an explicit four-bit fragment number.
    pub const fn sequence_control_with_fragment(self, fragment: u8) -> u16 {
        u16::from_le_bytes(
            SequenceControl::from_bytes([fragment, 0])
                .with_sequence(self.0)
                .to_bytes(),
        )
    }

    /// The following sequence number.
    pub const fn next(self) -> Self {
        self.wrapping_add(1)
    }

    // SOURCE: IEEE Std 802.11-2020, 9.2.4.4.2: sequence numbers wrap
    // modulo 4096; u16 wrapping is congruent modulo this power-of-two space.
    pub const fn wrapping_add(self, increment: u16) -> Self {
        Self::from_low_bits(self.get().wrapping_add(increment))
    }

    // SOURCE: IEEE Std 802.11-2020, 9.2.4.4.2 (modulo-4096 sequence space).
    pub const fn wrapping_sub(self, decrement: u16) -> Self {
        Self::from_low_bits(self.get().wrapping_sub(decrement))
    }

    /// Number of increments from `self` forward to `later`, in `0..4096`.
    pub const fn forward_distance(self, later: Self) -> u16 {
        // SOURCE: IEEE Std 802.11-2020, 10.24.7 (modular sequence distance).
        later.get().wrapping_sub(self.get()) & Self::MASK
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

oer_wire::bitfield! {
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
