//! Data-channel sequence and acknowledgement header fields (Core Vol 6 Part B 2.4.1).

/// A one-bit Link Layer sequence number.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct SequenceNumber(bool);

impl SequenceNumber {
    /// Zero, the value at the start of a connection.
    pub const ZERO: Self = Self(false);
    /// One.
    pub const ONE: Self = Self(true);

    /// The following sequence number; one wraps to zero.
    #[must_use]
    pub const fn next(self) -> Self {
        Self(!self.0)
    }

    /// The header bit.
    pub const fn bit(self) -> bool {
        self.0
    }
}

const LLID_MASK: u8 = 0b11;
const LLID_CONTINUATION_OR_EMPTY: u8 = 0b01;
const NESN: u8 = 1 << 2;
const SN: u8 = 1 << 3;
const MD: u8 = 1 << 4;

/// The acknowledgement fields of a data PDU header's first octet.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct HeaderFlags {
    /// Sequence number of the PDU.
    pub sn: SequenceNumber,
    /// Next sequence number the sender expects.
    pub nesn: SequenceNumber,
    /// The sender has more data after this PDU.
    pub md: bool,
}

impl HeaderFlags {
    /// The fields of a header's first octet.
    pub const fn from_octet(octet: u8) -> Self {
        Self {
            sn: SequenceNumber(octet & SN != 0),
            nesn: SequenceNumber(octet & NESN != 0),
            md: octet & MD != 0,
        }
    }

    /// `octet` with these fields; LLID, CP and RFU bits stay.
    #[must_use]
    pub const fn apply(self, octet: u8) -> u8 {
        let mut octet = octet & !(SN | NESN | MD);
        if self.sn.0 {
            octet |= SN;
        }
        if self.nesn.0 {
            octet |= NESN;
        }
        if self.md {
            octet |= MD;
        }
        octet
    }
}

/// The two-octet header of a data PDU received with a valid CRC.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ReceivedHeader {
    /// Acknowledgement fields.
    pub flags: HeaderFlags,
    /// The PDU is empty: LLID `0b01` and no payload.
    pub empty: bool,
}

impl ReceivedHeader {
    /// Read a header in air order: the flags octet, then the length.
    pub const fn new(header: [u8; 2]) -> Self {
        Self {
            flags: HeaderFlags::from_octet(header[0]),
            empty: header[0] & LLID_MASK == LLID_CONTINUATION_OR_EMPTY && header[1] == 0,
        }
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_macros)]
mod tests {
    use super::*;
    const DATA: u8 = 0b10;
    const EMPTY: u8 = 0b01;
    #[test]
    fn header_flags_round_trip_and_keep_the_other_bits() {
        let flags = HeaderFlags {
            sn: SequenceNumber::ONE,
            nesn: SequenceNumber::ZERO,
            md: true,
        };
        let octet = flags.apply(0b1110_0110);
        assert_eq!(HeaderFlags::from_octet(octet), flags);
        assert_eq!(octet & 0b1110_0011, 0b1110_0010);
        let header = ReceivedHeader::new([flags.apply(EMPTY), 0]);
        assert!(header.empty);
        assert!(!ReceivedHeader::new([flags.apply(EMPTY), 1]).empty);
        assert!(!ReceivedHeader::new([flags.apply(DATA), 0]).empty);
    }

    #[test]
    fn sequence_numbers_wrap() {
        assert_eq!(SequenceNumber::ZERO.next(), SequenceNumber::ONE);
        assert_eq!(SequenceNumber::ONE.next(), SequenceNumber::ZERO);
        assert!(!SequenceNumber::default().bit());
    }
}
