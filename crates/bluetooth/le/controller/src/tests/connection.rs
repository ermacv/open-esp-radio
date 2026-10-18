//! The peripheral connection from its indication to its end.

use std::vec::Vec;

use bt_hci::cmd::{Opcode, OpcodeGroup};
use oer_bluetooth_radio::{
    ConnectionEventTiming, DataPduKind, EventId, EventResult, LeInstant, RadioOutcome, ReceivedPdu,
};

use super::{Harness, Request, SET_ADV_ENABLE, SET_EVENT_MASK, SUCCESS};

const DISCONNECT: Opcode = Opcode::new(OpcodeGroup::LINK_CONTROL, 0x0006);
const LTK_REPLY: Opcode = Opcode::new(OpcodeGroup::LE, 0x001a);

/// On-air start of the connection indication.
pub(super) const INDICATION_AT: u64 = 20_000;
/// 30 ms interval, 1 s supervision timeout, 2.5 ms transmit window at offset 0.
pub(super) const INTERVAL: u64 = 30_000;
const TIMEOUT: u64 = 1_000_000;

/// `CONNECT_IND` from a random initiator to the public advertiser, asking for
/// Channel Selection Algorithm  with a 50 ppm central clock.
pub(super) fn connect_ind() -> Vec<u8> {
    let mut pdu = std::vec![0x05 | 0x20 | 0x40, 34];
    pdu.extend_from_slice(&[0xa1, 0xa2, 0xa3, 0xa4, 0xa5, 0xc6]); // InitA
    pdu.extend_from_slice(&[6, 5, 4, 3, 2, 1]); // AdvA
    pdu.extend_from_slice(&0x5a3c_71e9_u32.to_le_bytes()); // AA
    pdu.extend_from_slice(&[0x55, 0x66, 0x77]); // CRCInit
    pdu.push(2); // WinSize
    pdu.extend_from_slice(&0_u16.to_le_bytes()); // WinOffset
    pdu.extend_from_slice(&24_u16.to_le_bytes()); // Interval
    pdu.extend_from_slice(&0_u16.to_le_bytes()); // Latency
    pdu.extend_from_slice(&100_u16.to_le_bytes()); // Timeout
    pdu.extend_from_slice(&[0xff, 0xff, 0xff, 0xff, 0x1f]); // ChM
    pdu.push(7 | (5 << 5)); // Hop, SCA
    pdu
}

impl<const C: usize, const O: usize> super::Bench<C, O> {
    /// A connected peripheral whose first event is in progress.
    pub(super) fn connected() -> (Self, EventId) {
        Self::connected_from(Self::configured())
    }

    pub(super) fn connected_from(mut harness: Self) -> (Self, EventId) {
        // LE Meta plus Disconnection Complete, Encryption Change and Read
        // Remote Version Information Complete.
        let mask = [0x90, 0x08, 0, 0, 0, 0, 0, 0x20];
        assert_eq!(harness.command(SET_EVENT_MASK, &mask), Some(SUCCESS));
        let mut parameters = super::nonconnectable_parameters();
        parameters[4] = 0x00; // ADV_IND
        assert_eq!(
            harness.command(super::SET_ADV_PARAMS, &parameters),
            Some(SUCCESS)
        );
        harness.send(SET_ADV_ENABLE, &[1]);
        let Some(Request::ConfigureConnectable(adv_ind, scan_rsp)) = harness.step().unwrap() else {
            panic!("a connectable set");
        };
        assert_eq!(adv_ind[0] & 0x0f, 0x00);
        assert_eq!(adv_ind[0] & 0x20, 0x20, "Channel Selection Algorithm #2");
        assert_eq!(scan_rsp[..8], [0x04, 6, 6, 5, 4, 3, 2, 1]);
        assert_eq!(harness.status_of(SET_ADV_ENABLE), Some(SUCCESS));
        let Some(Request::Advertise(event)) = harness.step().unwrap() else {
            panic!("an advertising event");
        };
        harness.core.outcome(RadioOutcome::Received {
            id: event.id,
            pdu: ReceivedPdu {
                pdu: &connect_ind(),
                rssi_dbm: -50,
                captured_at: Some(LeInstant::from_micros(INDICATION_AT)),
            },
        });
        let Some(Request::OpenConnection(configuration)) = harness.step().unwrap() else {
            panic!("the connection opens");
        };
        assert_eq!(
            configuration.access_address.0,
            0x5a3c_71e9_u32.to_le_bytes()
        );
        assert_eq!(configuration.crc_init.0, [0x55, 0x66, 0x77]);
        assert_eq!(configuration.created_at.as_micros(), INDICATION_AT);
        let events = harness.drain();
        assert_eq!(events.len(), 1);
        assert_eq!(&events[0][..5], &[0x3e, 19, 0x01, SUCCESS, 0]);

        // Other roles keep the first event's reservation free before it is
        // planned.
        let reserved = harness.core.planned_connections().unwrap()[0];
        // The first event listens across the transmit window.
        let Some(Request::ConnectionEvent(first)) = harness.step().unwrap() else {
            panic!("the first event");
        };
        assert_eq!(
            reserved,
            Some(super::TIMING.reservation(first.window).unwrap())
        );
        let anchor = INDICATION_AT + 352 + 1_250;
        assert_eq!(first.window.start().as_micros(), anchor - 17);
        assert_eq!(
            first.timing,
            ConnectionEventTiming::First {
                transmit_window: oer_bluetooth_radio::RadioDuration::from_micros(2_500),
                timing_guard: oer_bluetooth_radio::RadioDuration::from_micros(16),
            }
        );
        assert_eq!(first.priority, 13);
        // Advertising stops: its event is cancelled and the set removed.
        assert_eq!(harness.step().unwrap(), Some(Request::Cancel(event.id)));
        harness.end(event.id);
        assert_eq!(harness.step().unwrap(), Some(Request::RemoveAdvertising));
        assert!(
            harness.drain().is_empty(),
            "advertising ends without an event"
        );
        (harness, first.id)
    }

    pub(super) fn receive(&mut self, id: EventId, pdu: &[u8]) {
        self.core.outcome(RadioOutcome::Received {
            id,
            pdu: ReceivedPdu {
                pdu,
                rssi_dbm: -40,
                captured_at: None,
            },
        });
    }

    pub(super) fn end_at(&mut self, id: EventId, anchor: Option<u64>) {
        self.core.outcome(RadioOutcome::EventEnded {
            id,
            result: match anchor {
                Some(anchor) => EventResult::Executed {
                    anchor: Some(LeInstant::from_micros(anchor)),
                },
                None => EventResult::NotExecuted,
            },
        });
    }

    fn acknowledge(&mut self) {
        self.core.outcome(RadioOutcome::TransmitAcknowledged(
            oer_bluetooth_radio::ConnectionId::new(0),
        ));
    }
}

#[test]
fn a_connection_answers_version_exchange_and_follows_the_received_anchor() {
    let (mut harness, first) = Harness::connected();
    // LL_VERSION_IND from the central.
    harness.receive(first, &[0x03, 6, 0x0c, 0x0c, 0x5f, 0x00, 0x01, 0x00]);
    let captured = INDICATION_AT + 352 + 1_250 + 700;
    harness.end_at(first, Some(captured));
    assert_eq!(
        harness.step().unwrap(),
        Some(Request::Transmit(
            DataPduKind::Control,
            std::vec![0x0c, 0x0d, 0xff, 0xff, 0x01, 0x00]
        ))
    );
    let Some(Request::ConnectionEvent(second)) = harness.step().unwrap() else {
        panic!("the second event");
    };
    // 30 ms after the received anchor, widened by 30 ms at 550 ppm.
    let widening = 30 * 550 / 1_000 + 63;
    assert_eq!(
        second.window.start().as_micros(),
        captured + INTERVAL - 10 - widening - 1
    );
    assert_eq!(
        second.timing,
        ConnectionEventTiming::Recurring {
            receive_wait: oer_bluetooth_radio::RadioDuration::from_micros(10 + 2 * widening + 2),
            widening: oer_bluetooth_radio::RadioDuration::from_micros(widening),
        }
    );
    assert_eq!(second.interval.as_micros(), INTERVAL);
    assert_eq!(second.priority, 8);
}

#[test]
fn acl_data_flows_both_ways_and_completes_host_packets() {
    let (mut harness, first) = Harness::connected();
    // Central data: a 3-octet L2CAP start fragment.
    harness.receive(first, &[0x02, 3, 0xaa, 0xbb, 0xcc]);
    let received = harness.drain();
    assert_eq!(received, [std::vec![0x00, 0x20, 3, 0, 0xaa, 0xbb, 0xcc]]);
    harness.end_at(first, Some(INDICATION_AT + 2_000));

    // Host data: 30 octets become a start and a continuation fragment.
    assert!(
        harness
            .core
            .acl_admission()
            .admits(bt_hci::param::ConnHandle::new(0))
    );
    let data: Vec<u8> = (0..30).collect();
    let packet = bt_hci::data::AclPacket::new(
        bt_hci::param::ConnHandle::new(0),
        bt_hci::data::AclPacketBoundary::FirstNonFlushable,
        bt_hci::data::AclBroadcastFlag::PointToPoint,
        &data,
    );
    harness.core.acl(packet);
    assert!(
        !harness
            .core
            .acl_admission()
            .admits(bt_hci::param::ConnHandle::new(0))
    );
    let Some(Request::Transmit(DataPduKind::Start, fragment)) = harness.step().unwrap() else {
        panic!("the first fragment");
    };
    assert_eq!(fragment.len(), 27);
    let Some(Request::ConnectionEvent(second)) = harness.step().unwrap() else {
        panic!("the second event");
    };
    harness.acknowledge();
    harness.end_at(second.id, Some(INDICATION_AT + 2_000 + INTERVAL));
    let Some(Request::Transmit(DataPduKind::Continuation, rest)) = harness.step().unwrap() else {
        panic!("the continuation");
    };
    assert_eq!(rest, (27..30).collect::<Vec<u8>>());
    let Some(Request::ConnectionEvent(third)) = harness.step().unwrap() else {
        panic!("the third event");
    };
    harness.acknowledge();
    // Number Of Completed Packets for the handle.
    assert_eq!(harness.drain(), [std::vec![0x13, 5, 1, 0, 0, 1, 0]]);
    assert!(
        harness
            .core
            .acl_admission()
            .admits(bt_hci::param::ConnHandle::new(0))
    );
    harness.end_at(third.id, None);
}

#[test]
fn host_disconnect_terminates_after_the_central_acknowledged() {
    let (mut harness, first) = Harness::connected();
    harness.end_at(first, Some(INDICATION_AT + 2_000));
    assert_eq!(
        harness.command(DISCONNECT, &[0, 0, 0x13]),
        Some(SUCCESS),
        "Command Status"
    );
    assert_eq!(
        harness.step().unwrap(),
        Some(Request::Transmit(
            DataPduKind::Control,
            std::vec![0x02, 0x13]
        ))
    );
    let Some(Request::ConnectionEvent(second)) = harness.step().unwrap() else {
        panic!("the second event");
    };
    harness.acknowledge();
    harness.end_at(second.id, Some(INDICATION_AT + 2_000 + INTERVAL));
    assert_eq!(harness.step().unwrap(), Some(Request::CloseConnection));
    // Disconnection Complete reports the local Host.
    assert_eq!(harness.drain(), [std::vec![0x05, 4, 0, 0, 0, 0x16]]);
    assert!(!harness.core.wants_radio());
    assert!(
        harness
            .core
            .acl_admission()
            .admits(bt_hci::param::ConnHandle::new(0)),
        "data without a connection is dropped"
    );
}

#[test]
fn the_central_terminates_the_connection() {
    let (mut harness, first) = Harness::connected();
    harness.receive(first, &[0x03, 2, 0x02, 0x13]);
    harness.end_at(first, Some(INDICATION_AT + 2_000));
    assert_eq!(harness.step().unwrap(), Some(Request::CloseConnection));
    assert_eq!(harness.drain(), [std::vec![0x05, 4, 0, 0, 0, 0x13]]);
}

#[test]
fn six_silent_events_fail_the_establishment() {
    let (mut harness, first) = Harness::connected();
    harness.end_at(first, None);
    for _ in 0..5 {
        let Some(Request::ConnectionEvent(event)) = harness.step().unwrap() else {
            panic!("another event");
        };
        // Before a packet arrives, the transmit window stays uncertain.
        let ConnectionEventTiming::Recurring { receive_wait, .. } = event.timing else {
            panic!("a recurring event");
        };
        assert!(receive_wait.as_micros() > 2_500);
        harness.end_at(event.id, None);
    }
    assert_eq!(harness.step().unwrap(), Some(Request::CloseConnection));
    assert_eq!(harness.drain(), [std::vec![0x05, 4, 0, 0, 0, 0x3e]]);
}

#[test]
fn the_supervision_timeout_ends_a_silent_connection() {
    let (mut harness, first) = Harness::connected();
    let captured = INDICATION_AT + 2_000;
    harness.end_at(first, Some(captured));
    let mut events = 0;
    loop {
        match harness.step().unwrap() {
            Some(Request::ConnectionEvent(event)) => {
                events += 1;
                harness.end_at(event.id, None);
            }
            Some(Request::CloseConnection) => break,
            other => panic!("unexpected {other:?}"),
        }
    }
    assert_eq!(events as u64, TIMEOUT / INTERVAL);
    assert_eq!(harness.drain(), [std::vec![0x05, 4, 0, 0, 0, 0x08]]);
}

#[test]
fn encryption_start_asks_the_host_for_its_key() {
    let (mut harness, first) = Harness::connected();
    // LL_ENC_REQ: Rand, EDIV, SKDm, IVm.
    let mut request = std::vec![0x03, 23, 0x03];
    request.extend_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
    request.extend_from_slice(&0x1234_u16.to_le_bytes());
    request.extend_from_slice(&[9; 8]);
    request.extend_from_slice(&[7; 4]);
    harness.receive(first, &request);
    harness.end_at(first, Some(INDICATION_AT + 2_000));
    let Some(Request::Transmit(DataPduKind::Control, response)) = harness.step().unwrap() else {
        panic!("LL_ENC_RSP");
    };
    // SKDs and IVs come from the random source.
    assert_eq!(response[0], 0x04);
    assert_eq!(&response[1..9], &[1, 2, 3, 4, 5, 6, 7, 8]);
    assert_eq!(&response[9..13], &[1, 2, 3, 4]);
    let Some(Request::ConnectionEvent(second)) = harness.step().unwrap() else {
        panic!("the second event");
    };
    harness.acknowledge();
    // LE Long Term Key Request.
    let events = harness.drain();
    assert_eq!(events.len(), 1);
    assert_eq!(&events[0][..5], &[0x3e, 13, 0x05, 0, 0]);
    assert_eq!(&events[0][5..13], &[1, 2, 3, 4, 5, 6, 7, 8]);
    harness.end_at(second.id, Some(INDICATION_AT + 2_000 + INTERVAL));

    let mut reply = std::vec![0, 0];
    reply.extend_from_slice(&[0x42; 16]);
    assert_eq!(harness.command(LTK_REPLY, &reply), Some(SUCCESS));
    // LL_START_ENC_REQ, still unencrypted.
    assert_eq!(
        harness.step().unwrap(),
        Some(Request::Transmit(DataPduKind::Control, std::vec![0x05]))
    );
}

#[test]
fn a_mic_failure_exits_at_once_without_a_terminate() {
    let (mut harness, first) = Harness::connected();
    let mut request = std::vec![0x03, 23, 0x03];
    request.extend_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
    request.extend_from_slice(&0x1234_u16.to_le_bytes());
    request.extend_from_slice(&[9; 8]);
    request.extend_from_slice(&[7; 4]);
    harness.receive(first, &request);
    harness.end_at(first, Some(INDICATION_AT + 2_000));
    assert!(matches!(
        harness.step().unwrap(),
        Some(Request::Transmit(DataPduKind::Control, _))
    ));
    let Some(Request::ConnectionEvent(second)) = harness.step().unwrap() else {
        panic!("the second event");
    };
    harness.acknowledge();
    harness.drain();
    harness.end_at(second.id, Some(INDICATION_AT + 2_000 + INTERVAL));
    let mut reply = std::vec![0, 0];
    reply.extend_from_slice(&[0x42; 16]);
    assert_eq!(harness.command(LTK_REPLY, &reply), Some(SUCCESS));
    assert_eq!(
        harness.step().unwrap(),
        Some(Request::Transmit(DataPduKind::Control, std::vec![0x05]))
    );
    let Some(Request::ConnectionEvent(third)) = harness.step().unwrap() else {
        panic!("the third event");
    };
    harness.acknowledge();
    // An encrypted LL_START_ENC_RSP whose MIC does not authenticate.
    harness.receive(third.id, &[0x03, 5, 0x9f, 0xcd, 0xa7, 0xf4, 0x49]);
    harness.end_at(third.id, Some(INDICATION_AT + 2_000 + 2 * INTERVAL));
    // No LL_TERMINATE_IND: the connection closes at once.
    assert_eq!(harness.step().unwrap(), Some(Request::CloseConnection));
    assert_eq!(harness.drain(), [std::vec![0x05, 4, 0, 0, 0, 0x3d]]);
}

#[test]
fn a_connection_update_moves_the_anchor_at_its_instant() {
    let (mut harness, first) = Harness::connected();
    // LL_CONNECTION_UPDATE_IND: WinSize 1, WinOffset 2, Interval 40 (50 ms),
    // Latency 0, Timeout 200, Instant 2.
    let mut update = std::vec![0x03, 12, 0x00, 1];
    update.extend_from_slice(&2_u16.to_le_bytes());
    update.extend_from_slice(&40_u16.to_le_bytes());
    update.extend_from_slice(&0_u16.to_le_bytes());
    update.extend_from_slice(&200_u16.to_le_bytes());
    update.extend_from_slice(&2_u16.to_le_bytes());
    harness.receive(first, &update);
    let captured = INDICATION_AT + 2_000;
    harness.end_at(first, Some(captured));
    // Event 1 keeps the old interval.
    let Some(Request::ConnectionEvent(second)) = harness.step().unwrap() else {
        panic!("event 1");
    };
    let second_anchor = captured + INTERVAL;
    harness.end_at(second.id, Some(second_anchor));
    // Event 2 is the instant: one old interval plus the new window offset,
    // listening across the new transmit window.
    let Some(Request::ConnectionEvent(instant)) = harness.step().unwrap() else {
        panic!("the instant");
    };
    let anchor = second_anchor + INTERVAL + 2 * 1_250;
    let widening = 32 * 550 / 1_000 + 63;
    assert_eq!(
        instant.window.start().as_micros(),
        anchor - 10 - widening - 1
    );
    assert_eq!(
        instant.timing,
        ConnectionEventTiming::Recurring {
            receive_wait: oer_bluetooth_radio::RadioDuration::from_micros(
                10 + 2 * widening + 1_250 + 2
            ),
            widening: oer_bluetooth_radio::RadioDuration::from_micros(widening),
        }
    );
    harness.end_at(instant.id, Some(anchor + 300));
    // LE Connection Update Complete with the new parameters.
    assert_eq!(
        harness.drain(),
        [std::vec![0x3e, 10, 0x03, 0, 0, 0, 40, 0, 0, 0, 200, 0]]
    );
    let Some(Request::ConnectionEvent(after)) = harness.step().unwrap() else {
        panic!("the event after the instant");
    };
    assert!(after.window.start().as_micros() > anchor + 300 + 50_000 - 200);
}

#[test]
fn supervision_follows_the_timeout_a_connection_update_sets() {
    let (mut harness, first) = Harness::connected();
    // LL_CONNECTION_UPDATE_IND: WinSize 1, WinOffset 0, the same 30 ms
    // interval, Latency 0, Timeout 300 (3 s, up from 1 s), Instant 2.
    let mut update = std::vec![0x03, 12, 0x00, 1];
    update.extend_from_slice(&0_u16.to_le_bytes());
    update.extend_from_slice(&24_u16.to_le_bytes());
    update.extend_from_slice(&0_u16.to_le_bytes());
    update.extend_from_slice(&300_u16.to_le_bytes());
    update.extend_from_slice(&2_u16.to_le_bytes());
    harness.receive(first, &update);
    harness.end_at(first, Some(INDICATION_AT + 2_000));
    // 1.5 s of missed events: past the old timeout, within the new one.
    let missed = 3 * TIMEOUT / (2 * INTERVAL);
    for _ in 0..missed {
        match harness.step().unwrap() {
            Some(Request::ConnectionEvent(event)) => harness.end_at(event.id, None),
            other => panic!("the connection is still supervised: {other:?}"),
        }
    }
}

#[test]
fn the_reserved_next_event_follows_a_connection_update() {
    let (mut harness, first) = Harness::connected();
    // LL_CONNECTION_UPDATE_IND: WinSize 1, WinOffset 2, Interval 40 (50 ms
    // instead of 30 ms), Latency 0, Timeout 200, Instant 2.
    let mut update = std::vec![0x03, 12, 0x00, 1];
    update.extend_from_slice(&2_u16.to_le_bytes());
    update.extend_from_slice(&40_u16.to_le_bytes());
    update.extend_from_slice(&0_u16.to_le_bytes());
    update.extend_from_slice(&200_u16.to_le_bytes());
    update.extend_from_slice(&2_u16.to_le_bytes());
    harness.receive(first, &update);
    harness.end_at(first, Some(INDICATION_AT + 2_000));
    let planned = |harness: &Harness| harness.core.planned_connections().unwrap()[0];
    let reserved = |event: &oer_bluetooth_radio::ConnectionEvent| {
        Some(super::TIMING.reservation(event.window).unwrap())
    };
    // Before the instant, the old interval; then, with that event in
    // flight, the instant itself; then the updated interval. Missed events
    // keep the reference, so each reservation is the real next one.
    for event in 1..=3 {
        let expected = planned(&harness);
        let Some(Request::ConnectionEvent(next)) = harness.step().unwrap() else {
            panic!("event {event}");
        };
        assert_eq!(reserved(&next), expected, "event {event}");
        if event < 3 {
            // Event 1 in flight reserves the instant, event 2.
            let in_flight = planned(&harness);
            harness.end_at(next.id, None);
            let Some(Request::ConnectionEvent(after)) = harness.step().unwrap() else {
                panic!("the event after {event}");
            };
            assert_eq!(reserved(&after), in_flight, "after {event}");
            harness.end_at(after.id, None);
        }
    }
}

#[test]
fn a_connection_is_traced_from_its_indication_to_its_close() {
    use oer_bluetooth_trace::{
        AdvertiserStep, AdvertiserStepKind as Step, ConnectionIndication, IndicationVerdict,
        PeripheralClosed, PeripheralEvent, PeripheralEventOp as Op, PeripheralOpened,
    };
    let (mut harness, first) = Harness::connected();
    let steps: std::vec::Vec<Step> = crate::trace::take::<AdvertiserStep>()
        .into_iter()
        .map(|step| step.step)
        .collect();
    assert_eq!(
        steps,
        [
            Step::Configured,
            Step::EventPlanned,
            Step::EventEnded,
            Step::Removed
        ]
    );
    assert_eq!(
        crate::trace::take::<ConnectionIndication>(),
        [ConnectionIndication {
            verdict: IndicationVerdict::Accepted,
        }]
    );
    assert_eq!(
        crate::trace::take::<PeripheralOpened>(),
        [PeripheralOpened {
            first_anchor_low: (INDICATION_AT + 352 + 1_250) as u32,
            interval_units: (INTERVAL / 1_250) as u16,
        }]
    );
    harness.end_at(first, Some(INDICATION_AT + 2_000));
    let event = |op, counter| PeripheralEvent {
        op,
        counter,
        id: first.get(),
    };
    assert_eq!(
        crate::trace::take::<PeripheralEvent>(),
        [
            event(Op::Requested, 0),
            event(Op::Accepted, 0),
            event(Op::Observed, 0)
        ]
    );
    // Reset closes the connection without a reason for the Host.
    assert_eq!(harness.command(super::RESET, &[]), None);
    assert_eq!(harness.step().unwrap(), Some(Request::CloseConnection));
    assert_eq!(
        crate::trace::take::<PeripheralClosed>(),
        [PeripheralClosed { reason: None }]
    );
}

#[test]
fn another_pdu_in_the_event_is_traced_apart_from_a_malformed_indication() {
    use oer_bluetooth_trace::{ConnectionIndication, IndicationVerdict};
    let mut harness = Harness::configured();
    let mut parameters = super::nonconnectable_parameters();
    parameters[4] = 0x00; // ADV_IND
    assert_eq!(
        harness.command(super::SET_ADV_PARAMS, &parameters),
        Some(SUCCESS)
    );
    harness.send(SET_ADV_ENABLE, &[1]);
    assert!(matches!(
        harness.step().unwrap(),
        Some(Request::ConfigureConnectable(_, _))
    ));
    assert_eq!(harness.status_of(SET_ADV_ENABLE), Some(SUCCESS));
    let Some(Request::Advertise(event)) = harness.step().unwrap() else {
        panic!("an advertising event");
    };
    // A SCAN_REQ, then a CONNECT_IND one byte short.
    let mut scan_request = std::vec![0x03, 12];
    scan_request.extend_from_slice(&connect_ind()[2..14]);
    let mut truncated = connect_ind();
    truncated[1] -= 1;
    truncated.pop();
    for pdu in [scan_request, truncated] {
        harness.core.outcome(RadioOutcome::Received {
            id: event.id,
            pdu: ReceivedPdu {
                pdu: &pdu,
                rssi_dbm: -50,
                captured_at: Some(LeInstant::from_micros(INDICATION_AT)),
            },
        });
    }
    assert_eq!(
        crate::trace::take::<ConnectionIndication>(),
        [
            ConnectionIndication {
                verdict: IndicationVerdict::OtherPdu,
            },
            ConnectionIndication {
                verdict: IndicationVerdict::Malformed,
            },
        ]
    );
}

#[test]
fn a_refused_opening_is_traced_closed_with_the_host_s_reason() {
    use oer_bluetooth_trace::{PeripheralClosed, PeripheralOpened};
    let mut harness = Harness::configured();
    let mut parameters = super::nonconnectable_parameters();
    parameters[4] = 0x00; // ADV_IND
    assert_eq!(
        harness.command(super::SET_ADV_PARAMS, &parameters),
        Some(SUCCESS)
    );
    harness.send(SET_ADV_ENABLE, &[1]);
    assert!(matches!(
        harness.step().unwrap(),
        Some(Request::ConfigureConnectable(_, _))
    ));
    assert_eq!(harness.status_of(SET_ADV_ENABLE), Some(SUCCESS));
    let Some(Request::Advertise(event)) = harness.step().unwrap() else {
        panic!("an advertising event");
    };
    harness.core.outcome(RadioOutcome::Received {
        id: event.id,
        pdu: ReceivedPdu {
            pdu: &connect_ind(),
            rssi_dbm: -50,
            captured_at: Some(LeInstant::from_micros(INDICATION_AT)),
        },
    });
    let refused = harness.step_with(Err(oer_bluetooth_radio::RequestError::NoInstance));
    assert!(matches!(refused, Ok(Some(Request::OpenConnection(_)))));
    assert_eq!(crate::trace::take::<PeripheralOpened>().len(), 1);
    // Connection Rejected due to Limited Resources.
    assert_eq!(
        crate::trace::take::<PeripheralClosed>(),
        [PeripheralClosed { reason: Some(0x0d) }]
    );
}

#[test]
fn reset_drops_the_connection_without_disconnection_complete() {
    let (mut harness, first) = Harness::connected();
    assert_eq!(harness.command(super::RESET, &[]), None);
    assert_eq!(harness.step().unwrap(), Some(Request::Cancel(first)));
    harness.end_at(first, None);
    assert_eq!(harness.step().unwrap(), Some(Request::CloseConnection));
    assert_eq!(harness.status_of(super::RESET), Some(SUCCESS));
    assert!(!harness.core.wants_radio());
}

#[test]
fn host_credits_hold_received_data_and_the_next_event() {
    const FLOW_CONTROL: Opcode = Opcode::new(OpcodeGroup::CONTROL_BASEBAND, 0x0031);
    const HOST_BUFFER_SIZE: Opcode = Opcode::new(OpcodeGroup::CONTROL_BASEBAND, 0x0033);
    const HOST_COMPLETED: Opcode = Opcode::new(OpcodeGroup::CONTROL_BASEBAND, 0x0035);
    let mut harness = Harness::configured();
    assert_eq!(harness.command(FLOW_CONTROL, &[0x01]), Some(SUCCESS));
    // 27-octet packets, one ACL buffer.
    assert_eq!(
        harness.command(HOST_BUFFER_SIZE, &[27, 0, 0, 1, 0, 0, 0]),
        Some(SUCCESS)
    );
    let (mut harness, first) = Harness::connected_from(harness);
    harness.receive(first, &[0x02, 1, 0xaa]);
    harness.receive(first, &[0x02, 1, 0xbb]);
    assert_eq!(harness.drain(), [std::vec![0x00, 0x20, 1, 0, 0xaa]]);
    harness.end_at(first, Some(INDICATION_AT + 2_000));
    harness.now = INDICATION_AT + 2_000;
    // Held data keeps the next event from being planned.
    assert_eq!(harness.step().unwrap(), None);
    harness.send(HOST_COMPLETED, &[1, 0, 0, 1, 0]);
    assert_eq!(harness.drain(), [std::vec![0x00, 0x20, 1, 0, 0xbb]]);
    assert!(matches!(
        harness.step().unwrap(),
        Some(Request::ConnectionEvent(_))
    ));
}

#[test]
fn withheld_host_credits_end_the_connection_at_the_supervision_timeout() {
    const FLOW_CONTROL: Opcode = Opcode::new(OpcodeGroup::CONTROL_BASEBAND, 0x0031);
    const HOST_BUFFER_SIZE: Opcode = Opcode::new(OpcodeGroup::CONTROL_BASEBAND, 0x0033);
    let mut harness = Harness::configured();
    assert_eq!(harness.command(FLOW_CONTROL, &[0x01]), Some(SUCCESS));
    assert_eq!(
        harness.command(HOST_BUFFER_SIZE, &[27, 0, 0, 1, 0, 0, 0]),
        Some(SUCCESS)
    );
    let (mut harness, first) = Harness::connected_from(harness);
    harness.receive(first, &[0x02, 1, 0xaa]);
    harness.receive(first, &[0x02, 1, 0xbb]);
    assert_eq!(harness.drain(), [std::vec![0x00, 0x20, 1, 0, 0xaa]]);
    let anchor = INDICATION_AT + 2_000;
    harness.end_at(first, Some(anchor));
    // The credit stays withheld: no event is planned, and the connection
    // survives until the supervision timeout has passed.
    harness.now = anchor + TIMEOUT;
    assert_eq!(harness.step().unwrap(), None);
    harness.now = anchor + TIMEOUT + 1;
    assert_eq!(harness.step().unwrap(), Some(Request::CloseConnection));
    assert_eq!(harness.drain(), [std::vec![0x05, 4, 0, 0, 0, 0x08]]);
}

const LE_SET_EVENT_MASK: Opcode = Opcode::new(OpcodeGroup::LE, 0x0001);
const SET_DATA_LENGTH: Opcode = Opcode::new(OpcodeGroup::LE, 0x0022);

/// `LL_LENGTH_REQ` or `LL_LENGTH_RSP` announcing 251 octets and 2120 us both
/// ways.
fn length_pdu(opcode: u8) -> Vec<u8> {
    std::vec![0x03, 9, opcode, 251, 0, 0x48, 8, 251, 0, 0x48, 8]
}

#[test]
fn data_length_update_lengthens_host_data_fragments() {
    let (mut harness, first) = Harness::connected();
    // The default LE mask plus LE Data Length Change.
    assert_eq!(
        harness.command(LE_SET_EVENT_MASK, &[0x5f, 0, 0, 0, 0, 0, 0, 0]),
        Some(SUCCESS)
    );
    // The central's request: its response keeps the local minimum to send
    // and raises the receive length.
    harness.receive(first, &length_pdu(0x14));
    assert_eq!(
        harness.drain(),
        [std::vec![
            0x3e, 11, 0x07, 0, 0, 27, 0, 0x48, 1, 251, 0, 0x48, 8
        ]]
    );
    harness.end_at(first, Some(INDICATION_AT + 2_000));
    assert_eq!(
        harness.step().unwrap(),
        Some(Request::Transmit(
            DataPduKind::Control,
            std::vec![0x15, 251, 0, 0x48, 8, 27, 0, 0x48, 1]
        ))
    );
    let Some(Request::ConnectionEvent(second)) = harness.step().unwrap() else {
        panic!("the second event");
    };
    harness.acknowledge();
    harness.end_at(second.id, Some(INDICATION_AT + 2_000 + INTERVAL));

    // The Host asks to send longer packets: the local request follows.
    harness.send(SET_DATA_LENGTH, &[0, 0, 251, 0, 0x48, 8]);
    assert_eq!(
        harness.drain(),
        [std::vec![0x0e, 6, 1, 0x22, 0x20, SUCCESS, 0, 0]]
    );
    assert_eq!(
        harness.step().unwrap(),
        Some(Request::Transmit(
            DataPduKind::Control,
            length_pdu(0x14)[2..].to_vec()
        ))
    );
    let Some(Request::ConnectionEvent(third)) = harness.step().unwrap() else {
        panic!("the third event");
    };
    harness.acknowledge();
    harness.receive(third.id, &length_pdu(0x15));
    assert_eq!(
        harness.drain(),
        [std::vec![
            0x3e, 11, 0x07, 0, 0, 251, 0, 0x48, 8, 251, 0, 0x48, 8
        ]]
    );
    harness.end_at(third.id, Some(INDICATION_AT + 2_000 + 2 * INTERVAL));

    // 200 octets of Host data now travel in one fragment.
    let data: Vec<u8> = (0..200).map(|octet| octet as u8).collect();
    harness.core.acl(bt_hci::data::AclPacket::new(
        bt_hci::param::ConnHandle::new(0),
        bt_hci::data::AclPacketBoundary::FirstNonFlushable,
        bt_hci::data::AclBroadcastFlag::PointToPoint,
        &data,
    ));
    assert_eq!(
        harness.step().unwrap(),
        Some(Request::Transmit(DataPduKind::Start, data))
    );
}

#[test]
fn a_peer_without_data_length_update_keeps_the_minimum() {
    let (mut harness, first) = Harness::connected();
    assert_eq!(
        harness.command(LE_SET_EVENT_MASK, &[0x5f, 0, 0, 0, 0, 0, 0, 0]),
        Some(SUCCESS)
    );
    harness.end_at(first, Some(INDICATION_AT + 2_000));
    harness.send(SET_DATA_LENGTH, &[0, 0, 251, 0, 0x48, 8]);
    harness.drain();
    assert!(matches!(
        harness.step().unwrap(),
        Some(Request::Transmit(DataPduKind::Control, _))
    ));
    let Some(Request::ConnectionEvent(second)) = harness.step().unwrap() else {
        panic!("the second event");
    };
    harness.acknowledge();
    // LL_UNKNOWN_RSP for LL_LENGTH_REQ.
    harness.receive(second.id, &[0x03, 2, 0x07, 0x14]);
    assert!(harness.drain().is_empty(), "no Data Length Change");
    harness.end_at(second.id, Some(INDICATION_AT + 2_000 + INTERVAL));
    let data: Vec<u8> = (0..30).collect();
    harness.core.acl(bt_hci::data::AclPacket::new(
        bt_hci::param::ConnHandle::new(0),
        bt_hci::data::AclPacketBoundary::FirstNonFlushable,
        bt_hci::data::AclBroadcastFlag::PointToPoint,
        &data,
    ));
    let Some(Request::Transmit(DataPduKind::Start, fragment)) = harness.step().unwrap() else {
        panic!("the first fragment");
    };
    assert_eq!(fragment.len(), 27);
}

#[test]
fn a_scannable_set_answers_scans_but_ignores_connection_indications() {
    let mut harness = Harness::configured();
    let mut parameters = super::nonconnectable_parameters();
    parameters[4] = 0x02; // ADV_SCAN_IND
    assert_eq!(
        harness.command(super::SET_ADV_PARAMS, &parameters),
        Some(SUCCESS)
    );
    harness.send(SET_ADV_ENABLE, &[1]);
    let Some(Request::ConfigureConnectable(adv_scan_ind, scan_rsp)) = harness.step().unwrap()
    else {
        panic!("a response-capable set");
    };
    assert_eq!(adv_scan_ind[..8], [0x06, 6, 6, 5, 4, 3, 2, 1]);
    assert_eq!(scan_rsp[..8], [0x04, 6, 6, 5, 4, 3, 2, 1]);
    assert_eq!(harness.status_of(SET_ADV_ENABLE), Some(SUCCESS));
    let Some(Request::Advertise(event)) = harness.step().unwrap() else {
        panic!("an advertising event");
    };
    // The scan exchange fits the channel spacing; a connection indication
    // needs no room.
    let air = |length: u32| (length - 2) * 8 + 80;
    let scan = air(adv_scan_ind.len() as u32) + 4 + 150 + 176 + 150 + air(scan_rsp.len() as u32);
    assert_eq!(
        event.channel_spacing.as_micros(),
        super::TIMING.preparation_lead.as_micros() + u64::from(scan)
    );
    harness.core.outcome(RadioOutcome::Received {
        id: event.id,
        pdu: ReceivedPdu {
            pdu: &connect_ind(),
            rssi_dbm: -50,
            captured_at: Some(LeInstant::from_micros(INDICATION_AT)),
        },
    });
    harness.end(event.id);
    assert!(matches!(
        harness.step().unwrap(),
        Some(Request::Advertise(_))
    ));
    assert!(harness.drain().is_empty());
}

/// Directed parameters addressed to the random initiator of [`connect_ind`].
fn directed_parameters(adv_kind: u8) -> [u8; 15] {
    let mut parameters = super::nonconnectable_parameters();
    parameters[4] = adv_kind;
    parameters[6] = 0x01; // random peer
    parameters[7..13].copy_from_slice(&[0xa1, 0xa2, 0xa3, 0xa4, 0xa5, 0xc6]);
    parameters
}

#[test]
fn low_duty_directed_advertising_connects_only_its_target() {
    let mut harness = Harness::configured();
    let mask = [0x90, 0x08, 0, 0, 0, 0, 0, 0x20];
    assert_eq!(harness.command(SET_EVENT_MASK, &mask), Some(SUCCESS));
    assert_eq!(
        harness.command(super::SET_ADV_PARAMS, &directed_parameters(0x04)),
        Some(SUCCESS)
    );
    harness.send(SET_ADV_ENABLE, &[1]);
    let Some(Request::ConfigureDirected(pdu)) = harness.step().unwrap() else {
        panic!("a directed set that only receives");
    };
    // ADV_DIRECT_IND with ChSel and a random target: AdvA then TargetA.
    assert_eq!(
        pdu,
        [
            0x01 | 0x20 | 0x80,
            12,
            6,
            5,
            4,
            3,
            2,
            1,
            0xa1,
            0xa2,
            0xa3,
            0xa4,
            0xa5,
            0xc6
        ]
    );
    assert_eq!(harness.status_of(SET_ADV_ENABLE), Some(SUCCESS));

    // Another initiator is ignored.
    let Some(Request::Advertise(event)) = harness.step().unwrap() else {
        panic!("an advertising event");
    };
    let mut stranger = connect_ind();
    stranger[2] = 0xb1;
    harness.core.outcome(RadioOutcome::Received {
        id: event.id,
        pdu: ReceivedPdu {
            pdu: &stranger,
            rssi_dbm: -50,
            captured_at: Some(LeInstant::from_micros(INDICATION_AT)),
        },
    });
    harness.end(event.id);
    assert!(harness.drain().is_empty());

    let Some(Request::Advertise(event)) = harness.step().unwrap() else {
        panic!("advertising continues");
    };
    harness.core.outcome(RadioOutcome::Received {
        id: event.id,
        pdu: ReceivedPdu {
            pdu: &connect_ind(),
            rssi_dbm: -50,
            captured_at: Some(LeInstant::from_micros(INDICATION_AT)),
        },
    });
    assert!(matches!(
        harness.step().unwrap(),
        Some(Request::OpenConnection(_))
    ));
}

#[test]
fn high_duty_directed_advertising_times_out_after_1280_ms() {
    let mut harness = Harness::configured();
    assert_eq!(
        harness.command(super::SET_ADV_PARAMS, &directed_parameters(0x01)),
        Some(SUCCESS)
    );
    harness.send(SET_ADV_ENABLE, &[1]);
    assert!(matches!(
        harness.step().unwrap(),
        Some(Request::ConfigureDirected(_))
    ));
    assert_eq!(harness.status_of(SET_ADV_ENABLE), Some(SUCCESS));
    let mut anchors = Vec::new();
    loop {
        match harness.step().unwrap() {
            Some(Request::Advertise(event)) => {
                anchors.push(event.anchor.as_micros());
                harness.now = event.anchor.as_micros();
                harness.end(event.id);
            }
            Some(Request::RemoveAdvertising) => break,
            other => panic!("unexpected {other:?}"),
        }
    }
    // Events 3.75 ms apart without the advertising delay, for 1.28 s.
    assert!(anchors.windows(2).all(|pair| pair[1] - pair[0] == 3_750));
    assert_eq!(anchors.len(), 1_280_000 / 3_750 + 1);
    // LE Connection Complete reports the Advertising Timeout.
    let events = harness.drain();
    assert_eq!(events.len(), 1);
    assert_eq!(&events[0][..4], &[0x3e, 19, 0x01, 0x3c]);
    assert!(!harness.core.wants_radio());
}

const ARM_MIC_CORRUPTION: Opcode = Opcode::new(OpcodeGroup::VENDOR_SPECIFIC, 0x0001);

#[cfg(feature = "diagnostic-mic-fault")]
#[test]
fn mic_corruption_arms_an_open_connection_before_encryption() {
    let mut harness = Harness::configured();
    assert_eq!(harness.command(ARM_MIC_CORRUPTION, &[0, 0]), Some(0x02));
    let (mut harness, first) = Harness::connected_from(harness);
    assert_eq!(harness.command(ARM_MIC_CORRUPTION, &[1, 0]), Some(0x02));
    assert_eq!(harness.command(ARM_MIC_CORRUPTION, &[0]), Some(0x12));
    assert_eq!(harness.command(ARM_MIC_CORRUPTION, &[0, 0]), Some(SUCCESS));
    // A plaintext data PDU carries no MIC: it is delivered unchanged and the
    // arming waits for encryption.
    harness.receive(first, &[0x02, 1, 0xaa]);
    assert_eq!(harness.drain(), [std::vec![0x00, 0x20, 1, 0, 0xaa]]);
}

#[cfg(not(feature = "diagnostic-mic-fault"))]
#[test]
fn mic_corruption_is_an_unknown_command_without_the_diagnostic_feature() {
    let (mut harness, _first) = Harness::connected();
    assert_eq!(harness.command(ARM_MIC_CORRUPTION, &[0, 0]), Some(0x01));
}

#[test]
fn an_aborted_connection_event_keeps_its_data_without_an_anchor() {
    let (mut harness, first) = Harness::connected();
    harness.core.outcome(RadioOutcome::Received {
        id: first,
        pdu: ReceivedPdu {
            pdu: &[0x02, 3, 0xaa, 0xbb, 0xcc],
            rssi_dbm: -40,
            captured_at: None,
        },
    });
    assert_eq!(
        harness.drain(),
        [std::vec![0, 0x20, 3, 0, 0xaa, 0xbb, 0xcc]]
    );
    harness.core.outcome(RadioOutcome::EventEnded {
        id: first,
        result: EventResult::Aborted,
    });
    let data = [1, 2, 3];
    harness.core.acl(bt_hci::data::AclPacket::new(
        bt_hci::param::ConnHandle::new(0),
        bt_hci::data::AclPacketBoundary::FirstNonFlushable,
        bt_hci::data::AclBroadcastFlag::PointToPoint,
        &data,
    ));
    assert_eq!(
        harness.step().unwrap(),
        Some(Request::Transmit(DataPduKind::Start, data.to_vec()))
    );
    let Some(Request::ConnectionEvent(second)) = harness.step().unwrap() else {
        panic!("connection continues")
    };
    // An aborted event cannot replace the first planned connection anchor.
    let first_anchor = INDICATION_AT + 352 + 1_250;
    assert!(second.window.start().as_micros() > first_anchor + INTERVAL - 1_000);
    assert!(second.window.start().as_micros() < first_anchor + INTERVAL);
}

#[test]
fn a_connection_indication_without_a_capture_does_not_open_a_reference() {
    let mut harness = Harness::configured();
    let mut params = super::nonconnectable_parameters();
    params[4] = 0;
    assert_eq!(
        harness.command(super::SET_ADV_PARAMS, &params),
        Some(SUCCESS)
    );
    harness.send(SET_ADV_ENABLE, &[1]);
    harness.step().unwrap();
    harness.drain();
    let Some(Request::Advertise(event)) = harness.step().unwrap() else {
        panic!("advertise")
    };
    harness.core.outcome(RadioOutcome::Received {
        id: event.id,
        pdu: ReceivedPdu {
            pdu: &connect_ind(),
            rssi_dbm: -40,
            captured_at: None,
        },
    });
    harness.core.outcome(RadioOutcome::EventEnded {
        id: event.id,
        result: EventResult::Executed { anchor: None },
    });
    assert!(harness.drain().is_empty());
    assert!(matches!(
        harness.step().unwrap(),
        Some(Request::Advertise(_))
    ));
}

#[test]
fn failed_recurring_anchor_restores_the_ll_owner_and_allows_reset_close() {
    let (mut harness, first) = Harness::connected();
    harness.end_at(first, Some(u64::MAX - 10_000));
    let first_error = harness.step().unwrap_err();
    assert_eq!(first_error.role, crate::PlanningRole::Peripheral);
    assert_eq!(
        first_error.calculation,
        crate::PlanningCalculation::Recurrence
    );
    assert_eq!(harness.step(), Err(first_error));
    harness.now = u64::MAX;
    harness.send(super::RESET, &[]);
    assert_eq!(harness.step().unwrap(), Some(Request::CloseConnection));
    assert_eq!(harness.status_of(super::RESET), Some(SUCCESS));
}

#[test]
fn failed_first_connection_geometry_keeps_the_indication_until_reset() {
    use crate::{PlanningCalculation, PlanningOperation, PlanningRole};

    for (captured, calculation) in [
        (u64::MAX - 100, PlanningCalculation::FirstAnchor),
        (u64::MAX - 5_000, PlanningCalculation::WindowGeometry),
    ] {
        let mut harness = Harness::configured();
        let mut params = super::nonconnectable_parameters();
        params[4] = 0;
        assert_eq!(
            harness.command(super::SET_ADV_PARAMS, &params),
            Some(SUCCESS)
        );
        harness.send(SET_ADV_ENABLE, &[1]);
        assert!(matches!(
            harness.step().unwrap(),
            Some(Request::ConfigureConnectable(_, _))
        ));
        assert_eq!(harness.status_of(SET_ADV_ENABLE), Some(SUCCESS));
        let Some(Request::Advertise(event)) = harness.step().unwrap() else {
            panic!("advertising event");
        };
        harness.core.outcome(RadioOutcome::Received {
            id: event.id,
            pdu: ReceivedPdu {
                pdu: &connect_ind(),
                rssi_dbm: -40,
                captured_at: Some(LeInstant::from_micros(captured)),
            },
        });
        let expected = crate::EpochExhausted {
            role: PlanningRole::Peripheral,
            operation: PlanningOperation::ConnectionIndication,
            calculation,
        };
        assert_eq!(harness.step(), Err(expected));
        assert_eq!(harness.step(), Err(expected));
        assert!(
            harness.drain().is_empty(),
            "an invalid first window cannot announce a connection"
        );
        // Control teardown requires no future radio coordinate and retains
        // the already admitted advertising event's exact identity.
        harness.now = u64::MAX;
        harness.send(super::RESET, &[]);
        assert_eq!(harness.step().unwrap(), Some(Request::Cancel(event.id)));
        harness.end(event.id);
        assert_eq!(harness.step().unwrap(), Some(Request::RemoveAdvertising));
        assert_eq!(harness.status_of(super::RESET), Some(SUCCESS));
        assert!(!harness.core.wants_radio());
    }
}
