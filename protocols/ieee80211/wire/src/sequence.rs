//! IEEE 802.11 sequence numbers (IEEE 802.11-2020 9.2.4.4.2): the checked
//! value of the Sequence Number subfield.
//!
//! Every constructor yields a twelve-bit value, so frame builders need no
//! range validation. Their modulo-4096 arithmetic belongs to the MAC
//! mechanisms (`oer_ieee80211_upper_mac::sequence::SequenceSpace`).

/// One twelve-bit IEEE 802.11 sequence number.
///
/// Sequence numbers are modular, so the type deliberately has no ordering;
/// window decisions take the MAC's modular arithmetic.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub struct SequenceNumber(oer_wire::U16<12>);

impl SequenceNumber {
    /// Size of the sequence-number space.
    pub const MODULUS: u16 = 4096;
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
