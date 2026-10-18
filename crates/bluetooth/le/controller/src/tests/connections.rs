//! Several peripheral connections in one Controller.

use std::vec::Vec;

use bt_hci::{
    cmd::{Opcode, OpcodeGroup},
    param::ConnHandle,
};
use oer_bluetooth_radio::{EventId, RadioOutcome, ReceivedPdu};

use super::{Bench, Harness, Pair, Request, SET_ADV_ENABLE, SUCCESS};

const DISCONNECT: Opcode = Opcode::new(OpcodeGroup::LINK_CONTROL, 0x0006);

/// `CONNECT_IND` with access address `access_address`, as the single
/// connection's tests send it otherwise.
fn connect_ind(access_address: u32) -> Vec<u8> {
    let mut pdu = super::connection::connect_ind();
    pdu[14..18].copy_from_slice(&access_address.to_le_bytes());
    pdu
}

/// Advertise again and accept a connection indication with
/// `access_address`; returns its Connection Complete.
fn open_another<const C: usize, const O: usize>(
    bench: &mut Bench<C, O>,
    access_address: u32,
) -> Vec<u8> {
    open_another_with(bench, access_address, |_| 0)
}

/// [`open_another`] with the window offset, in 1.25 ms units, that
/// `offset` derives from the advertising event's anchor in microseconds.
fn open_another_with<const C: usize, const O: usize>(
    bench: &mut Bench<C, O>,
    access_address: u32,
    offset: impl Fn(u64) -> u16,
) -> Vec<u8> {
    bench.send(SET_ADV_ENABLE, &[1]);
    let advertise = loop {
        match bench.step().unwrap() {
            Some(Request::Advertise(event)) => break event,
            Some(Request::ConfigureConnectable(..)) => {
                assert_eq!(bench.status_of(SET_ADV_ENABLE), Some(SUCCESS));
            }
            // Open connections keep their events.
            Some(Request::ConnectionEvent(_)) => {}
            other => panic!("unexpected {other:?}"),
        }
    };
    bench.core.outcome(RadioOutcome::Received {
        id: advertise.id,
        pdu: ReceivedPdu {
            pdu: &{
                let mut pdu = connect_ind(access_address);
                pdu[22..24].copy_from_slice(&offset(advertise.anchor.as_micros()).to_le_bytes());
                pdu
            },
            rssi_dbm: -50,
            captured_at: Some(advertise.anchor),
        },
    });
    let step = bench.step().unwrap();
    let Some(Request::OpenConnection(configuration)) = step else {
        panic!("the connection opens, not {step:?}");
    };
    assert!(configuration.connection.index() < C as u8);
    assert_eq!(configuration.access_address.0, access_address.to_le_bytes());
    let events = bench.drain();
    assert_eq!(events.len(), 1, "{events:x?}");
    events[0].clone()
}

/// Two connections: the first one open with an event in flight, then a
/// second one from a second advertising cycle, with its Connection Complete.
fn two_connections() -> (Pair, EventId, Vec<u8>) {
    let (mut bench, first) = Pair::connected();
    let complete = open_another(&mut bench, 0x5a3c_71ea);
    (bench, first, complete)
}

#[test]
fn a_second_indication_opens_a_second_connection_with_its_own_handle() {
    let (_, _, complete) = two_connections();
    // LE Connection Complete, success, handle 1.
    assert_eq!(&complete[..6], &[0x3e, 19, 0x01, SUCCESS, 1, 0]);
}

#[test]
fn host_commands_and_data_reach_only_the_connection_they_name() {
    let (mut bench, _, _) = two_connections();
    // Host ACL for handle 1 occupies only that connection.
    let data: Vec<u8> = (0..8).collect();
    bench.core.acl(bt_hci::data::AclPacket::new(
        ConnHandle::new(1),
        bt_hci::data::AclPacketBoundary::FirstNonFlushable,
        bt_hci::data::AclBroadcastFlag::PointToPoint,
        &data,
    ));
    let admission = bench.core.acl_admission();
    assert!(!admission.admits(ConnHandle::new(1)));
    assert!(admission.admits(ConnHandle::new(0)));
    // A handle no connection holds is unknown.
    assert_eq!(
        bench.command(DISCONNECT, &[5, 0, 0x13]),
        Some(0x02),
        "Unknown Connection Identifier"
    );
    assert_eq!(bench.command(DISCONNECT, &[0, 0, 0x13]), Some(SUCCESS));
}

#[test]
fn a_closed_connection_s_handle_is_not_reused_at_once() {
    let (mut bench, first) = Harness::connected();
    // The central terminates the connection on handle 0.
    bench.receive(first, &[0x03, 2, 0x02, 0x13]);
    bench.end_at(first, Some(super::connection::INDICATION_AT + 2_000));
    assert_eq!(bench.step().unwrap(), Some(Request::CloseConnection));
    assert_eq!(bench.drain(), [std::vec![0x05, 4, 0, 0, 0, 0x13]]);
    // The next connection takes handle 1, so a command still in flight to
    // handle 0 finds no connection.
    let complete = open_another(&mut bench, 0x5a3c_71ea);
    assert_eq!(&complete[..6], &[0x3e, 19, 0x01, SUCCESS, 1, 0]);
}

#[test]
fn a_scan_window_gives_way_when_the_next_connection_event_moves_onto_it() {
    let (mut bench, first) = Harness::connected();
    let anchor = super::connection::INDICATION_AT + 352 + 1_250;
    // Past the first event, a scan window fills the time up to the
    // connection's next event.
    bench.now = anchor + 5_000;
    bench.scan(48, 48, false);
    let Some(Request::Scan(window)) = bench.step().unwrap() else {
        panic!("a scan window");
    };
    let reservation = super::TIMING.reservation(window.window).unwrap();
    let planned = bench.core.planned_connections().unwrap()[0].unwrap();
    assert!(!reservation.overlaps(planned));
    // The central's packet arrives earlier than the nominal anchor, which
    // moves the next event onto the window: the window gives way.
    bench.end_at(first, Some(anchor - 40));
    let planned = bench.core.planned_connections().unwrap()[0].unwrap();
    assert!(reservation.overlaps(planned));
    assert_eq!(bench.step().unwrap(), Some(Request::Cancel(window.id)));
    // The cancelled window holds the radio until its end arrives.
    assert_eq!(bench.step().unwrap(), None);
    bench.end(window.id);
    assert!(matches!(
        bench.step().unwrap(),
        Some(Request::ConnectionEvent(_))
    ));
}

#[test]
fn the_connection_closer_to_its_supervision_timeout_takes_the_other_s_event() {
    let (mut bench, first) = Pair::connected();
    let anchor = super::connection::INDICATION_AT + 352 + 1_250;
    let next = anchor + super::connection::INTERVAL;
    // Advertising resumes past the first connection's first event, and the
    // second connection's first anchor lies on the first one's next anchor.
    bench.now = anchor + 5_000;
    open_another_with(&mut bench, 0x5a3c_71ea, |at| {
        u16::try_from((next - (at + 352 + 1_250)) / 1_250).unwrap()
    });
    let second = loop {
        match bench.step().unwrap() {
            Some(Request::ConnectionEvent(event)) => break event,
            Some(_) => {}
            None => panic!("the second connection's first event"),
        }
    };
    assert_eq!(second.connection.index(), 1);
    // The first connection hears its central and its next event is due: its
    // supervision timeout ends a whole interval before the second's.
    bench.end_at(first, Some(anchor));
    assert_eq!(bench.step().unwrap(), Some(Request::Cancel(second.id)));
    // Until the cancelled event ends, the first connection waits rather
    // than missing its event; other roles' work goes on.
    loop {
        match bench.step().unwrap() {
            Some(Request::ConnectionEvent(event)) => panic!("planned too early: {event:?}"),
            Some(Request::Cancel(id)) => bench.end(id),
            Some(_) => {}
            None => break,
        }
    }
    bench.end(second.id);
    let Some(Request::ConnectionEvent(event)) = bench.step().unwrap() else {
        panic!("the first connection's event");
    };
    assert_eq!(event.connection.index(), 0);
}

#[test]
fn a_new_connection_shares_the_host_buffers_another_one_holds() {
    const FLOW_CONTROL: Opcode = Opcode::new(OpcodeGroup::CONTROL_BASEBAND, 0x0031);
    const HOST_BUFFER_SIZE: Opcode = Opcode::new(OpcodeGroup::CONTROL_BASEBAND, 0x0033);
    const HOST_COMPLETED: Opcode = Opcode::new(OpcodeGroup::CONTROL_BASEBAND, 0x0035);
    let mut bench = Pair::configured();
    assert_eq!(bench.command(FLOW_CONTROL, &[0x01]), Some(SUCCESS));
    // 27-octet packets, one ACL buffer.
    assert_eq!(
        bench.command(HOST_BUFFER_SIZE, &[27, 0, 0, 1, 0, 0, 0]),
        Some(SUCCESS)
    );
    let (mut bench, first) = Pair::connected_from(bench);
    // The first connection's data takes the Host's only buffer.
    bench.receive(first, &[0x02, 1, 0xaa]);
    assert_eq!(bench.drain(), [std::vec![0x00, 0x20, 1, 0, 0xaa]]);
    let anchor = super::connection::INDICATION_AT + 352 + 1_250;
    bench.end_at(first, Some(anchor));
    bench.now = anchor + 5_000;
    open_another(&mut bench, 0x5a3c_71ea);
    let second = loop {
        match bench.step().unwrap() {
            Some(Request::ConnectionEvent(event)) if event.connection.index() == 1 => break event,
            Some(_) => {}
            None => panic!("the second connection's event"),
        }
    };
    // The second connection opening returns no buffer: its data waits for
    // the first one's.
    bench.receive(second.id, &[0x02, 1, 0xcc]);
    assert!(bench.drain().is_empty());
    // A pair naming a handle no connection holds returns nothing and
    // keeps no other pair from counting.
    bench.send(HOST_COMPLETED, &[2, 9, 0, 1, 0, 0, 0, 1, 0]);
    assert_eq!(bench.drain(), [std::vec![0x01, 0x20, 1, 0, 0xcc]]);
}

#[test]
fn a_second_connection_event_waits_for_output_room_for_both() {
    // Two connections, but an output with room for one event's data only.
    let (mut bench, _) = Bench::<2, { super::OUTPUT }>::connected();
    let anchor = super::connection::INDICATION_AT + 352 + 1_250;
    bench.now = anchor + 5_000;
    // The first connection's first event is still in flight: the second
    // connection opens, but its event waits for that one to end.
    open_another(&mut bench, 0x5a3c_71ea);
    loop {
        match bench.step().unwrap() {
            Some(Request::ConnectionEvent(event)) => {
                panic!("an event planned without room: {event:?}")
            }
            Some(_) => {}
            None => break,
        }
    }
}

#[test]
fn data_delivered_without_flow_control_holds_no_host_buffer() {
    const FLOW_CONTROL: Opcode = Opcode::new(OpcodeGroup::CONTROL_BASEBAND, 0x0031);
    const HOST_BUFFER_SIZE: Opcode = Opcode::new(OpcodeGroup::CONTROL_BASEBAND, 0x0033);
    let (mut bench, first) = Harness::connected();
    bench.receive(first, &[0x02, 1, 0xaa]);
    assert_eq!(bench.drain(), [std::vec![0x00, 0x20, 1, 0, 0xaa]]);
    // Flow control starts with one buffer: the packet delivered before it
    // is not the Host's to complete, so the next one still goes.
    assert_eq!(bench.command(FLOW_CONTROL, &[0x01]), Some(SUCCESS));
    assert_eq!(
        bench.command(HOST_BUFFER_SIZE, &[27, 0, 0, 1, 0, 0, 0]),
        Some(SUCCESS)
    );
    bench.receive(first, &[0x02, 1, 0xbb]);
    assert_eq!(bench.drain(), [std::vec![0x00, 0x20, 1, 0, 0xbb]]);
}

#[test]
fn held_data_of_two_connections_waits_for_host_credits_without_loss() {
    const FLOW_CONTROL: Opcode = Opcode::new(OpcodeGroup::CONTROL_BASEBAND, 0x0031);
    const HOST_BUFFER_SIZE: Opcode = Opcode::new(OpcodeGroup::CONTROL_BASEBAND, 0x0033);
    let mut bench = Pair::configured();
    assert_eq!(bench.command(FLOW_CONTROL, &[0x01]), Some(SUCCESS));
    assert_eq!(
        bench.command(HOST_BUFFER_SIZE, &[27, 0, 0, 1, 0, 0, 0]),
        Some(SUCCESS)
    );
    let (mut bench, first) = Pair::connected_from(bench);
    // Both connections' events are in flight while nothing is held.
    open_another(&mut bench, 0x5a3c_71ea);
    let second = loop {
        match bench.step().unwrap() {
            Some(Request::ConnectionEvent(event)) if event.connection.index() == 1 => break event,
            Some(_) => {}
            None => panic!("the second connection's event"),
        }
    };
    // Each event fills up: the Host's one buffer takes the first packet,
    // and every other one waits, none lost.
    for octet in 0..8 {
        bench.receive(first, &[0x02, 1, octet]);
    }
    assert_eq!(bench.drain().len(), 1);
    for octet in 0..8 {
        bench.receive(second.id, &[0x02, 1, octet]);
    }
    assert!(bench.drain().is_empty());
}

#[test]
fn scanning_is_served_while_a_connection_waits_for_output_room() {
    const SET_SCAN_ENABLE: Opcode = Opcode::new(OpcodeGroup::LE, 0x000c);
    let (mut bench, first) = Bench::<2, { super::OUTPUT }>::connected();
    let anchor = super::connection::INDICATION_AT + 352 + 1_250;
    let interval = super::connection::INTERVAL;
    // A second connection opens while the first one's event is in flight,
    // its anchors half an interval from the first one's, so neither event
    // takes the other's.
    bench.now = anchor + 5_000;
    open_another_with(&mut bench, 0x5a3c_71ea, |at| {
        let earliest = at + 352 + 1_250;
        let middle = anchor + interval / 2;
        let target = middle + earliest.saturating_sub(middle).div_ceil(interval) * interval;
        u16::try_from((target - earliest) / 1_250).unwrap()
    });
    // The first connection hears its central; then the second one's event
    // is in flight, and the output has no room for the first one's next.
    bench.end_at(first, Some(anchor));
    loop {
        match bench.step().unwrap() {
            Some(Request::ConnectionEvent(event)) if event.connection.index() == 1 => break,
            Some(Request::ConnectionEvent(event)) => {
                let observed = event.window.start().as_micros() + 100;
                bench.end_at(event.id, Some(observed));
            }
            other => panic!("unexpected {other:?}"),
        }
    }
    // The advertising event that took the second indication gives way to
    // the connections' planned events, then advertising ends.
    loop {
        match bench.step().unwrap() {
            Some(Request::Cancel(id)) => bench.end(id),
            Some(Request::RemoveAdvertising) => {}
            None => break,
            other => panic!("unexpected {other:?}"),
        }
    }
    // Scanning still configures and stops while that connection waits.
    bench.scan(48, 48, false);
    assert_eq!(bench.command(SET_SCAN_ENABLE, &[0, 0]), None);
    loop {
        match bench.step().unwrap() {
            Some(Request::RemoveScanner) => break,
            Some(Request::ConnectionEvent(event)) => panic!("planned without room: {event:?}"),
            Some(Request::Cancel(id)) => bench.end(id),
            Some(_) => {}
            None => panic!("the scanner is never removed"),
        }
    }
    assert_eq!(bench.status_of(SET_SCAN_ENABLE), Some(SUCCESS));
}
