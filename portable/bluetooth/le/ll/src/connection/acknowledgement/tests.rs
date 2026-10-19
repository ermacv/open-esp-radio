use super::{
    Acknowledgement, Delivery, HeaderFlags, Received, ReceivedHeader, Sent, SequenceNumber,
    Transmission, Transmit,
};

const DATA: u8 = 0b10;
const EMPTY: u8 = 0b01;

/// One side of a connection: its state machine and the PDUs it has queued.
struct Side {
    state: Acknowledgement,
    queued: usize,
    delivered: usize,
    room: bool,
}

impl Side {
    fn new(queued: usize) -> Self {
        Self {
            state: Acknowledgement::new(),
            queued,
            delivered: 0,
            room: true,
        }
    }

    /// Transmit and return the header the peer would receive.
    fn send(&mut self) -> (Transmission, [u8; 2]) {
        let transmission = self.state.transmit(self.queued);
        if transmission.pdu == Transmit::Data {
            self.queued -= 1;
        }
        let (llid, length) = match transmission.pdu {
            Transmit::Data | Transmit::Resend(Sent::Data) => (DATA, 20),
            Transmit::Empty | Transmit::Resend(Sent::Empty) => (EMPTY, 0),
        };
        (transmission, [transmission.flags.apply(llid), length])
    }

    fn take(&mut self, header: [u8; 2]) -> Received {
        let received = self.state.receive(ReceivedHeader::new(header), self.room);
        if received.delivery == Delivery::Deliver {
            self.delivered += 1;
        }
        received
    }
}

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

#[test]
fn a_normal_exchange_delivers_and_acknowledges_every_pdu_in_order() {
    let mut central = Side::new(3);
    let mut peripheral = Side::new(3);
    for _ in 0..3 {
        let (sent, header) = central.send();
        assert_eq!(sent.pdu, Transmit::Data);
        let received = peripheral.take(header);
        assert_eq!(received.delivery, Delivery::Deliver);
        let (answer, header) = peripheral.send();
        assert_eq!(answer.pdu, Transmit::Data);
        let received = central.take(header);
        assert_eq!(received.acknowledged, Some(Sent::Data));
        assert_eq!(received.delivery, Delivery::Deliver);
    }
    assert_eq!((central.delivered, peripheral.delivered), (3, 3));
    // The peripheral's third PDU is acknowledged by the next central packet.
    assert_eq!(peripheral.state.in_flight(), Some(Sent::Data));
    let (sent, header) = central.send();
    assert_eq!(sent.pdu, Transmit::Empty);
    let received = peripheral.take(header);
    assert_eq!(received.acknowledged, Some(Sent::Data));
    assert_eq!(received.delivery, Delivery::Empty);
    assert_eq!(peripheral.state.in_flight(), None);
}

#[test]
fn a_first_packet_acknowledges_nothing() {
    let mut peripheral = Side::new(0);
    let received = peripheral.take([EMPTY, 0]);
    assert_eq!(
        received,
        Received {
            acknowledged: None,
            delivery: Delivery::Empty,
        }
    );
    assert_eq!(peripheral.state.next_expected(), SequenceNumber::ONE);
    assert_eq!(peripheral.state.transmit_sequence(), SequenceNumber::ZERO);
}

#[test]
fn nothing_queued_answers_with_an_empty_pdu() {
    let mut central = Side::new(0);
    let mut peripheral = Side::new(0);
    let (_, header) = central.send();
    peripheral.take(header);
    let (answer, header) = peripheral.send();
    assert_eq!(answer.pdu, Transmit::Empty);
    assert_eq!(
        answer.flags,
        HeaderFlags {
            sn: SequenceNumber::ZERO,
            nesn: SequenceNumber::ONE,
            md: false,
        }
    );
    assert_eq!(header, [0b0000_0101, 0]);
    // The empty PDU is sequenced and acknowledged like any other.
    let received = central.take(header);
    assert_eq!(received.acknowledged, Some(Sent::Empty));
    assert_eq!(received.delivery, Delivery::Empty);
    assert_eq!(central.delivered, 0);
}

#[test]
fn a_lost_acknowledgement_resends_the_pdu_and_the_peer_drops_the_duplicate() {
    let mut central = Side::new(1);
    let mut peripheral = Side::new(0);
    let (first, header) = central.send();
    assert_eq!(peripheral.take(header).delivery, Delivery::Deliver);
    // The peripheral's answer, which acknowledges the PDU, is lost.
    let _ = peripheral.send();
    // Without an acknowledgement the central resends the same PDU.
    let (again, header) = central.send();
    assert_eq!(again.pdu, Transmit::Resend(Sent::Data));
    assert_eq!(again.flags.sn, first.flags.sn);
    assert_eq!(peripheral.take(header).delivery, Delivery::Duplicate);
    assert_eq!(peripheral.delivered, 1);
    // The peripheral resends its own unacknowledged empty PDU.
    let (answer, header) = peripheral.send();
    assert_eq!(answer.pdu, Transmit::Resend(Sent::Empty));
    let received = central.take(header);
    assert_eq!(received.acknowledged, Some(Sent::Data));
    assert_eq!(central.state.in_flight(), None);
}

#[test]
fn a_negative_acknowledgement_keeps_the_pdu_in_flight() {
    let mut central = Side::new(2);
    let mut peripheral = Side::new(0);
    let (_, header) = central.send();
    // The peripheral missed the PDU (CRC error): nothing is received, and
    // its answer repeats the old NESN.
    let _ = header;
    let (_, nack) = peripheral.send();
    let received = central.take(nack);
    assert_eq!(received.acknowledged, None);
    let (again, header) = central.send();
    assert_eq!(again.pdu, Transmit::Resend(Sent::Data));
    // Another PDU still waits behind the one in flight.
    assert!(again.flags.md);
    assert_eq!(peripheral.take(header).delivery, Delivery::Deliver);
}

#[test]
fn a_receiver_without_room_defers_a_new_pdu_until_the_peer_resends_it() {
    let mut central = Side::new(1);
    let mut peripheral = Side::new(0);
    peripheral.room = false;
    let (_, header) = central.send();
    assert_eq!(peripheral.take(header).delivery, Delivery::Deferred);
    assert_eq!(peripheral.state.next_expected(), SequenceNumber::ZERO);
    let (_, answer) = peripheral.send();
    assert_eq!(central.take(answer).acknowledged, None);
    peripheral.room = true;
    let (again, header) = central.send();
    assert_eq!(again.pdu, Transmit::Resend(Sent::Data));
    assert_eq!(peripheral.take(header).delivery, Delivery::Deliver);
}

#[test]
fn more_data_follows_the_queue_and_either_side_keeps_the_event_open() {
    let mut central = Side::new(2);
    let mut peripheral = Side::new(0);
    let (sent, header) = central.send();
    assert!(sent.flags.md);
    peripheral.take(header);
    let (answer, header) = peripheral.send();
    assert!(!answer.flags.md);
    assert!(peripheral.state.more_data());
    central.take(header);
    assert!(central.state.more_data());

    let (sent, header) = central.send();
    assert_eq!(sent.pdu, Transmit::Data);
    assert!(!sent.flags.md);
    peripheral.take(header);
    let (_, header) = peripheral.send();
    central.take(header);
    assert!(!central.state.more_data());
    assert!(!peripheral.state.more_data());
}

#[test]
fn sequence_numbers_wrap_across_many_exchanges() {
    let mut central = Side::new(9);
    let mut peripheral = Side::new(9);
    let mut central_sn = [0; 2];
    for _ in 0..9 {
        let (sent, header) = central.send();
        central_sn[usize::from(sent.flags.sn.bit())] += 1;
        assert_eq!(peripheral.take(header).delivery, Delivery::Deliver);
        let (_, header) = peripheral.send();
        assert_eq!(central.take(header).acknowledged, Some(Sent::Data));
    }
    assert_eq!(central_sn, [5, 4]);
    assert_eq!((central.delivered, peripheral.delivered), (9, 9));
    assert_eq!(central.state.transmit_sequence(), SequenceNumber::ONE);
    assert_eq!(peripheral.state.next_expected(), SequenceNumber::ONE);
}
