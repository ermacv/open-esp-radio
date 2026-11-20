//! A connection through the model: its scripted central, its transmit
//! chain and its receive nodes, its withdraw and its closing.

#![allow(
    clippy::disallowed_methods,
    clippy::disallowed_macros,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "a test: its harness polls with a no-op waker and fails by panicking"
)]

mod harness;

use harness::{Model, at, connection_event, installed, link, running};
use oer_protocols_bluetooth_le_contracts_port::{
    ConnEvent, Connection, EventEnd, EventPlace, LePort, MoreData, NotExecuted, PduHandle, PduLen,
    RoleRef, RoleRights, TxEnd, TxPlace,
};
use oer_protocols_bluetooth_le_testing_model::{Ack, Heard, Role, Turn};
use oer_protocols_bluetooth_le_wire::DataPduKind;

/// One connection of two event places, three transmit places and two
/// receive nodes.
type Port<'id> = Model<'id, 0, 0, 1, 2, 3, 2, 0>;

/// What a round gave: the ends in their order, each with the payloads of
/// its receptions, the transmit ends, and the places back.
struct Gave<'id> {
    ends: Vec<(EventEnd, Vec<Vec<u8>>)>,
    events: Vec<ConnEvent<'id>>,
    transmitted: Vec<(TxEnd, TxPlace<'id>)>,
    held: Vec<PduHandle<'id>>,
}

/// One round: every end, every reception taken and held, every transmit
/// end.
fn round<'id>(port: &mut Port<'id>) -> Gave<'id> {
    let mut gave = Gave {
        ends: Vec::new(),
        events: Vec::new(),
        transmitted: Vec::new(),
        held: Vec::new(),
    };
    let mut round = port.round();
    while let Some(mut ended) = round.ended() {
        let end = ended.end();
        let mut payloads = Vec::new();
        while let Some(pdu) = ended.received() {
            payloads.push(pdu.view().payload.to_vec());
            gave.held.push(pdu.take());
        }
        let EventPlace::Connection(place) = ended.place() else {
            panic!("a connection's place");
        };
        gave.ends.push((end, payloads));
        gave.events.push(place);
    }
    while let Some(end) = round.transmitted() {
        gave.transmitted.push(end);
    }
    let _rest = round.close();
    gave
}

/// Release every held PDU in one round.
fn release<'id>(port: &mut Port<'id>, held: Vec<PduHandle<'id>>) {
    let mut round = port.round();
    for handle in held {
        round.release(handle);
    }
    let _rest = round.close();
}

fn central(sends: &[u8], ack: Ack) -> Turn {
    Turn::Central {
        heard: Heard::Heard,
        ack,
        sends: sends.to_vec(),
    }
}

/// A data PDU of `payload`, header first, as the central sends it.
fn data(payload: &[u8]) -> Vec<u8> {
    let mut pdu = vec![0x02, u8::try_from(payload.len()).unwrap()];
    pdu.extend_from_slice(payload);
    pdu
}

fn write<'id>(port: &mut Port<'id>, place: &mut TxPlace<'id>, byte: u8) {
    port.write_pdu(place, DataPduKind::Start, |payload| {
        payload[0] = byte;
        PduLen::new(1).unwrap()
    });
}

/// Publish both event places at `first` and `first` + 30 ms, and run the
/// clock past both.
fn two_events<'id>(port: &mut Port<'id>, [one, two]: [ConnEvent<'id>; 2], first: u64) {
    let _published = port.publish_connection(one, &connection_event(at(first), port.now()));
    let _published =
        port.publish_connection(two, &connection_event(at(first + 30_000), port.now()));
    port.advance(at(first + 32_000));
}

/// Open the connection, run `body` with it, then withdraw, take every
/// place home, close and check every node is free again.
fn connected(
    body: impl for<'id> FnOnce(
        &mut Port<'id>,
        &Connection<'id>,
        [ConnEvent<'id>; 2],
        [TxPlace<'id>; 3],
    ) -> (Vec<ConnEvent<'id>>, Vec<TxPlace<'id>>),
) {
    installed::<0, 0, 1, 2, 3, 2, 0>(|port, stopped, mut right, roles| {
        let RoleRights {
            advertising,
            scanners,
            connections: [place],
            tests,
            accept_list,
        } = roles;
        let (stopped, place) = running(port, stopped, &mut right, |port| {
            let (role, events, tx) = port.open_connection(place, &link());
            let (mut events, mut tx) = body(port, &role, events, tx);
            port.withdraw(RoleRef::Connection(&role));
            let gave = round(port);
            assert!(gave.held.is_empty());
            events.extend(gave.events);
            tx.extend(gave.transmitted.into_iter().map(|(_, place)| place));
            tx.sort_by_key(TxPlace::slot);
            events.sort_by_key(ConnEvent::slot);
            let events: [ConnEvent<'_>; 2] = events.try_into().expect("every event home");
            let tx: [TxPlace<'_>; 3] = tx.try_into().expect("every transmit place home");
            assert_eq!(port.chain(0), 0);
            port.close_connection(role, events, tx)
        });
        assert_eq!(port.free_nodes(), 6, "every node is free after the closing");
        let roles = RoleRights {
            advertising,
            scanners,
            connections: [place],
            tests,
            accept_list,
        };
        let retired = port.retire(stopped, roles);
        port.release(retired, right);
    });
}

#[test]
fn the_centrals_pdus_reach_the_rounds_in_the_airs_order() {
    connected(|port, _, events, tx| {
        port.script(Role::Connection, 0, central(&data(&[1]), Ack::Withhold));
        port.script(Role::Connection, 0, central(&data(&[2]), Ack::Withhold));
        two_events(port, events, 2_000);
        let gave = round(port);
        let payloads: Vec<_> = gave.ends.iter().flat_map(|(_, p)| p.clone()).collect();
        assert_eq!(payloads, [vec![1], vec![2]]);
        release(port, gave.held);
        (gave.events, tx.into())
    });
}

#[test]
fn a_held_pdus_node_is_not_free_and_comes_back_at_its_release() {
    connected(|port, _, events, tx| {
        port.script(Role::Connection, 0, central(&data(&[1]), Ack::Withhold));
        port.script(Role::Connection, 0, central(&data(&[2]), Ack::Withhold));
        two_events(port, events, 2_000);
        let gave = round(port);
        assert_eq!(gave.held.len(), 2);
        assert_eq!(port.free_nodes(), 4, "the connection's two nodes are held");
        release(port, gave.held);
        assert_eq!(port.free_nodes(), 6, "released nodes go back at the close");
        (gave.events, tx.into())
    });
}

#[test]
fn with_no_node_free_the_centrals_pdu_is_neither_taken_nor_acknowledged() {
    connected(|port, _, events, tx| {
        for byte in 1..=3 {
            port.script(Role::Connection, 0, central(&data(&[byte]), Ack::Withhold));
        }
        two_events(port, events, 2_000);
        let gave = round(port);
        assert_eq!(port.free_nodes(), 4);
        let [one, two] = <[ConnEvent<'_>; 2]>::try_from(gave.events).ok().unwrap();
        two_events(port, [one, two], 100_000);
        let more = round(port);
        // Both nodes are held: the third PDU is not taken, and the central
        // says it has more.
        assert!(more.held.is_empty());
        assert!(matches!(
            more.ends[0].0,
            EventEnd::Executed(executed) if executed.peer == Some(MoreData::More)
        ));
        let mut held = gave.held;
        release(port, std::mem::take(&mut held));
        let [one, two] = <[ConnEvent<'_>; 2]>::try_from(more.events).ok().unwrap();
        two_events(port, [one, two], 200_000);
        let last = round(port);
        let payloads: Vec<_> = last.ends.iter().flat_map(|(_, p)| p.clone()).collect();
        assert_eq!(payloads, [vec![3]], "sent again once a node is free");
        release(port, last.held);
        (last.events, tx.into())
    });
}

#[test]
fn the_chain_leaves_in_the_order_submitted_each_pdu_once_when_acknowledged() {
    connected(|port, _, events, tx| {
        let [mut a, mut b, mut c] = tx;
        write(port, &mut a, 1);
        write(port, &mut b, 2);
        write(port, &mut c, 3);
        port.submit(a);
        port.submit(b);
        port.submit(c);
        port.script(Role::Connection, 0, central(&[], Ack::Acknowledge));
        port.script(Role::Connection, 0, central(&[], Ack::Withhold));
        two_events(port, events, 2_000);
        let first = round(port);
        assert_eq!(
            first
                .transmitted
                .iter()
                .map(|(end, place)| (*end, place.slot()))
                .collect::<Vec<_>>(),
            [(TxEnd::Transmitted, 0)],
            "one acknowledged, one withheld"
        );
        assert_eq!(port.chain(0), 2);
        port.script(Role::Connection, 0, central(&[], Ack::Acknowledge));
        port.script(Role::Connection, 0, central(&[], Ack::Acknowledge));
        let [one, two] = <[ConnEvent<'_>; 2]>::try_from(first.events).ok().unwrap();
        two_events(port, [one, two], 100_000);
        let second = round(port);
        assert_eq!(
            second
                .transmitted
                .iter()
                .map(|(end, place)| (*end, place.slot()))
                .collect::<Vec<_>>(),
            [(TxEnd::Transmitted, 1), (TxEnd::Transmitted, 2)]
        );
        let mut places: Vec<_> = first.transmitted.into_iter().map(|(_, p)| p).collect();
        places.extend(second.transmitted.into_iter().map(|(_, p)| p));
        (second.events, places)
    });
}

#[test]
fn a_withdraw_ends_the_chain_withdrawn_and_every_event_not_begun() {
    connected(|port, role, [one, two], tx| {
        let [mut a, b, c] = tx;
        write(port, &mut a, 1);
        port.submit(a);
        let _published = port.publish_connection(one, &connection_event(at(2_000), port.now()));
        let _published = port.publish_connection(two, &connection_event(at(32_000), port.now()));
        port.withdraw(RoleRef::Connection(role));
        let gave = round(port);
        assert_eq!(
            gave.ends.iter().map(|(end, _)| *end).collect::<Vec<_>>(),
            [
                EventEnd::NotExecuted(NotExecuted::Withdrawn),
                EventEnd::NotExecuted(NotExecuted::Withdrawn)
            ]
        );
        assert_eq!(
            gave.transmitted
                .iter()
                .map(|(end, _)| *end)
                .collect::<Vec<_>>(),
            [TxEnd::Withdrawn]
        );
        assert_eq!(port.listed(), 0);
        let mut places: Vec<_> = gave.transmitted.into_iter().map(|(_, p)| p).collect();
        places.extend([b, c]);
        (gave.events, places)
    });
}

#[test]
fn a_withdraw_lets_an_event_begun_run_to_its_end() {
    connected(|port, role, [one, two], tx| {
        port.script(Role::Connection, 0, central(&[], Ack::Withhold));
        let _published = port.publish_connection(one, &connection_event(at(2_000), port.now()));
        let _published = port.publish_connection(two, &connection_event(at(32_000), port.now()));
        port.advance(at(3_000));
        port.withdraw(RoleRef::Connection(role));
        port.advance(at(4_000));
        let gave = round(port);
        assert_eq!(gave.ends.len(), 2);
        assert_eq!(
            gave.ends[0].0,
            EventEnd::NotExecuted(NotExecuted::Withdrawn)
        );
        assert!(
            matches!(gave.ends[1].0, EventEnd::Executed(_)),
            "the begun one ran"
        );
        (gave.events, tx.into())
    });
}

#[test]
fn a_lost_central_is_heard_again_in_its_next_event() {
    connected(|port, _, events, tx| {
        port.script(
            Role::Connection,
            0,
            Turn::Central {
                heard: Heard::Lost,
                ack: Ack::Acknowledge,
                sends: data(&[7]),
            },
        );
        two_events(port, events, 2_000);
        let gave = round(port);
        assert!(
            matches!(gave.ends[0], (EventEnd::Executed(executed), _) if executed.anchor.is_none() && executed.exchanged == 0)
        );
        let payloads: Vec<_> = gave.ends.iter().flat_map(|(_, p)| p.clone()).collect();
        assert_eq!(payloads, [vec![7]]);
        release(port, gave.held);
        (gave.events, tx.into())
    });
}

#[test]
fn a_stop_ends_the_chain_withdrawn_and_the_events_as_the_reset_found_them() {
    installed::<0, 0, 1, 2, 3, 2, 0>(|port, stopped, mut right, roles| {
        let RoleRights {
            advertising,
            scanners,
            connections: [place],
            tests,
            accept_list,
        } = roles;
        let (stopped, (role, [one, two], [mut a, b, c])) =
            running(port, stopped, &mut right, |port| {
                let (role, [one, two], tx) = port.open_connection(place, &link());
                (role, [one, two], tx)
            });
        let _ = (&one, &two, &mut a, &b, &c, &role);
        // The stop above ran with nothing out; a run with the chain and two
        // events out follows.
        let (stopped, gave) = running(port, stopped, &mut right, |port| {
            write(port, &mut a, 1);
            port.submit(a);
            let _published = port.publish_connection(one, &connection_event(at(2_000), port.now()));
            let _published =
                port.publish_connection(two, &connection_event(at(32_000), port.now()));
            port.advance(at(3_000));
        });
        let () = gave;
        let gave = round(port);
        assert_eq!(
            gave.ends.iter().map(|(end, _)| *end).collect::<Vec<_>>(),
            [
                EventEnd::Indeterminate,
                EventEnd::NotExecuted(NotExecuted::Withdrawn)
            ]
        );
        assert_eq!(
            gave.transmitted
                .iter()
                .map(|(end, _)| *end)
                .collect::<Vec<_>>(),
            [TxEnd::Withdrawn]
        );
        let mut events = gave.events;
        events.sort_by_key(ConnEvent::slot);
        let events: [ConnEvent<'_>; 2] = events.try_into().ok().unwrap();
        let [(_, a)] = <[(TxEnd, TxPlace<'_>); 1]>::try_from(gave.transmitted)
            .ok()
            .unwrap();
        let place = port.close_connection(role, events, [a, b, c]);
        let roles = RoleRights {
            advertising,
            scanners,
            connections: [place],
            tests,
            accept_list,
        };
        let retired = port.retire(stopped, roles);
        port.release(retired, right);
    });
}
