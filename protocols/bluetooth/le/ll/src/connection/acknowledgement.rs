//! Link Layer acknowledgement and flow control of one connection.
//!
//! The Core Specification (Vol 6, Part B, 4.5.9) gives each side of a
//! connection two one-bit counters, both zero when the connection starts:
//! `transmitSeqNum`, sent as the header's SN, and `nextExpectedSeqNum`, sent as
//! NESN. For every received data PDU with a valid CRC:
//!
//! - an SN equal to `nextExpectedSeqNum` marks a new PDU. A receiver with
//!   room for it advances `nextExpectedSeqNum` and delivers it; one without
//!   room leaves the counter, so the peer resends the PDU (flow control).
//!   Any other SN marks a resent PDU, which is dropped;
//! - an NESN different from `transmitSeqNum` acknowledges the PDU in flight:
//!   `transmitSeqNum` advances and the next PDU may be sent. An equal NESN
//!   leaves the PDU in flight, and it is resent unchanged.
//!
//! A PDU with a CRC error is not passed to [`Acknowledgement::receive`]: its
//! header cannot be trusted, nothing changes and the next transmission
//! resends the PDU in flight. With nothing queued the Link Layer answers
//! with an empty PDU, which is sequenced and acknowledged like any other.
//! The MD bit announces that the sender has another PDU after this one; the
//! connection event continues while either side's latest MD bit is set
//! (4.5.6).
//!
//! [`Acknowledgement`] is a synchronous state machine without time, buffers
//! or I/O, so a backend can run it in its time-critical context. It is what
//! a radio port whose `LinkAcknowledgement` is `Software` uses; a backend
//! whose hardware keeps SN and NESN, such as the ESP32-S31, does not.
//! The caller keeps the payloads: it holds the PDU in flight until
//! [`Received::acknowledged`] releases it and resends it when
//! [`Acknowledgement::transmit`] says so.

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

/// The PDU in flight.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Sent {
    /// A data or control PDU the caller holds.
    Data,
    /// An empty PDU.
    Empty,
}

/// What a received PDU means for the receiver.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Delivery {
    /// A new PDU with payload: deliver it.
    Deliver,
    /// A new empty PDU: nothing to deliver.
    Empty,
    /// A resent PDU that was already taken: drop it.
    Duplicate,
    /// A new PDU without room to take it: drop it unacknowledged, and the
    /// peer resends it.
    Deferred,
}

/// The result of one reception.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Received {
    /// The PDU in flight that the peer acknowledged: release it.
    pub acknowledged: Option<Sent>,
    /// What to do with the received PDU.
    pub delivery: Delivery,
}

/// What to transmit next.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Transmit {
    /// Resend the unacknowledged PDU in flight unchanged.
    Resend(Sent),
    /// Send the first queued PDU; it is in flight until acknowledged.
    Data,
    /// Nothing is queued: send an empty PDU.
    Empty,
}

/// The next transmission and its header fields.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Transmission {
    /// The PDU.
    pub pdu: Transmit,
    /// SN, NESN and MD of its header.
    pub flags: HeaderFlags,
}

/// Acknowledgement and flow-control state of one connection.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Acknowledgement {
    transmit_sequence: SequenceNumber,
    next_expected: SequenceNumber,
    in_flight: Option<Sent>,
    sent_more_data: bool,
    received_more_data: bool,
}

impl Default for Acknowledgement {
    fn default() -> Self {
        Self::new()
    }
}

impl Acknowledgement {
    /// The state at the start of a connection: both counters zero and
    /// nothing in flight.
    pub const fn new() -> Self {
        Self {
            transmit_sequence: SequenceNumber::ZERO,
            next_expected: SequenceNumber::ZERO,
            in_flight: None,
            sent_more_data: false,
            received_more_data: false,
        }
    }

    /// `transmitSeqNum`: the SN of the PDU in flight or of the next one.
    pub const fn transmit_sequence(&self) -> SequenceNumber {
        self.transmit_sequence
    }

    /// `nextExpectedSeqNum`: the SN of the next new PDU from the peer.
    pub const fn next_expected(&self) -> SequenceNumber {
        self.next_expected
    }

    /// The PDU sent and not yet acknowledged.
    pub const fn in_flight(&self) -> Option<Sent> {
        self.in_flight
    }

    /// Take one PDU received with a valid CRC. `room` tells whether the
    /// receiver can take a new PDU with payload; an empty PDU needs none.
    pub fn receive(&mut self, header: ReceivedHeader, room: bool) -> Received {
        let flags = header.flags;
        self.received_more_data = flags.md;
        let acknowledged = if flags.nesn == self.transmit_sequence {
            None
        } else {
            // A peer acknowledging nothing in flight is ignored.
            let released = self.in_flight.take();
            if released.is_some() {
                self.transmit_sequence = self.transmit_sequence.next();
            }
            released
        };
        let delivery = if flags.sn != self.next_expected {
            Delivery::Duplicate
        } else if header.empty {
            self.next_expected = self.next_expected.next();
            Delivery::Empty
        } else if room {
            self.next_expected = self.next_expected.next();
            Delivery::Deliver
        } else {
            Delivery::Deferred
        };
        Received {
            acknowledged,
            delivery,
        }
    }

    /// Choose the next transmission. `queued` counts the PDUs the caller
    /// has waiting, not counting the one in flight.
    pub fn transmit(&mut self, queued: usize) -> Transmission {
        let (pdu, more_data) = match self.in_flight {
            Some(sent) => (Transmit::Resend(sent), queued > 0),
            None if queued > 0 => {
                self.in_flight = Some(Sent::Data);
                (Transmit::Data, queued > 1)
            }
            None => {
                self.in_flight = Some(Sent::Empty);
                (Transmit::Empty, false)
            }
        };
        self.sent_more_data = more_data;
        Transmission {
            pdu,
            flags: HeaderFlags {
                sn: self.transmit_sequence,
                nesn: self.next_expected,
                md: more_data,
            },
        }
    }

    /// Whether the latest exchange announced more data from either side,
    /// which keeps the connection event open.
    pub const fn more_data(&self) -> bool {
        self.sent_more_data || self.received_more_data
    }
}

#[cfg(test)]
mod tests;
