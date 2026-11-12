//! The network's MSDUs go from the network's own memory: the access point
//! writes each MPDU in place in front of its payload and sends that memory,
//! which comes back to the network once, after its terminal. A QoS peer's
//! payload stays where the network wrote it; a shorter header moves it
//! down once, counted. A refusal draws no number and returns the memory
//! whole.

use super::*;
use core::future::poll_fn;
use oer_protocols_ieee80211_core_roles_sta::data::{EthernetFrame, MsduParts, WmmUserPriority};
use oer_protocols_network_port::NetTransmit;

const GROUP: [u8; 6] = [0xff; 6];

/// The payload every test frame carries.
pub(super) fn payload() -> [u8; 64] {
    core::array::from_fn(|at| (at as u8) ^ 0x5a)
}

/// The network's frame to `destination`.
pub(super) fn frame(destination: [u8; 6]) -> EthernetFrame {
    let mut frame = EthernetFrame::empty();
    frame
        .set_parts(MsduParts {
            destination,
            source: ACCESS_POINT,
            ether_type: 0x0800,
            payload: &payload(),
        })
        .unwrap();
    frame
}

/// The access point's data MPDUs on the air, as its memory held them before
/// the modeled encryption.
pub(super) fn data_mpdus(air: &oer_protocols_ieee80211_testing_model::ModelAir) -> Vec<Vec<u8>> {
    air.sent()
        .iter()
        .filter(|frame| frame.role == oer_protocols_ieee80211_contracts_port::VifRole::AccessPoint)
        .filter(|frame| {
            frame
                .frame
                .first()
                .is_some_and(|control| control & 0x0c == 0x08)
        })
        .map(|frame| frame.plaintext.clone())
        .collect()
}

/// Start the radio of `$runner` and turn it until it serves its BSS: the
/// group key is in.
macro_rules! serving {
    ($runner:ident, $counted:expr, $air:expr) => {{
        let now = || oer_portable_time::Clock::now(&*$counted.clock);
        let started = poll_fn(|cx| $runner.port.poll_start(cx, supported(), now())).await;
        assert!(started.is_ok(), "the radio starts");
        let mut turns = 0;
        while $runner.homes.phase != Phase::Running {
            turns += 1;
            assert!(turns < 64, "the access point serves");
            $counted.clock.advance(Duration::from_millis(1)).unwrap();
            $air.clock_advanced();
            let facts = Facts {
                tx: Some(TxFacts::DUE),
                tbtt: false,
                rx: false,
            };
            assert!($runner.turn(now(), facts).is_ok());
        }
    }};
}

/// Offer `$memory` to `$runner`'s preparation of a data attempt: refused,
/// the ticket goes home and the memory comes back, whole.
macro_rules! refused {
    ($runner:ident, $memory:expr, $length:expr) => {{
        let ticket = $runner.homes.tickets.pop().unwrap();
        let buffer = $runner.port.network_buffer($memory);
        match $runner.admit_network(ticket, buffer, $length, WmmUserPriority::UP0) {
            Ok(_) => panic!("refused"),
            Err((ticket, Source::Network(buffer))) => {
                assert!($runner.homes.tickets.push(ticket).is_ok());
                <Ty<'_, Port<'_>> as TxNetworkPool<'_>>::into_memory(buffer)
                    .ok()
                    .expect("the model's memory")
            }
            Err(_) => panic!("the network's memory comes back"),
        }
    }};
}

/// A group frame goes with the 24-octet header of a non-QoS frame: its
/// payload moves down two octets, once, and that is counted. The MPDU on
/// the air is the network's own memory, written in place, which comes back
/// once after its terminal.
#[test]
fn a_group_frame_moves_its_payload_two_octets_once_and_its_memory_comes_back() {
    embassy_futures::block_on(with_local_slot(async |claim| {
        let counted = Counted::new();
        fixture!(claim, counted.clock.clone() => runner, air, _device, transmit, supply, shared);
        let script = async {
            counted.idle(64).await;
            assert!(shared.status().running);
            let memory = supply.next();
            assert!(transmit.try_transmit(frame(GROUP)).is_ok());
            transmit.flush();
            for _ in 0..8 {
                counted.advance(&air, Duration::from_millis(1)).await;
            }
            let sent = data_mpdus(&air);
            assert_eq!(sent.len(), 1, "the frame went once");
            let mpdu = &sent[0];
            // Data, protected, From-DS, to the group: a 24-octet header,
            // the CCMP header and LLC/SNAP in front of the payload.
            assert_eq!(mpdu[..2], [0x08, 0x42]);
            assert_eq!(mpdu[4..10], GROUP);
            assert_eq!(mpdu.len(), 24 + 8 + 8 + payload().len());
            assert_eq!(mpdu[40..], payload());
            assert_eq!(shared.status().payload_moves, 1, "one move, counted");
            transmit.reclaim();
            assert_eq!(
                supply.free(),
                super::super::network::AP_TX_DEPTH,
                "its memory came back, once"
            );
            let back = supply.memory(memory).expect("the same memory came back");
            assert_eq!(back[..mpdu.len()], mpdu[..], "the MPDU was that memory's");
            stop_and_finish(&counted, &air, &shared).await;
        };
        stopped(drive(runner.run(&shared, &counted), script).await);
    }));
}

/// Frames the access point holds when it stops, queued or on their way to
/// the air, all come back to the network, each once.
#[test]
fn a_stop_returns_every_queued_and_submitted_network_memory_once() {
    embassy_futures::block_on(with_local_slot(async |claim| {
        let counted = Counted::new();
        fixture!(claim, counted.clock.clone() => runner, air, _device, transmit, supply, shared);
        let script = async {
            counted.idle(64).await;
            assert!(shared.status().running);
            for _ in 0..12 {
                assert!(transmit.try_transmit(frame(GROUP)).is_ok());
            }
            transmit.flush();
            // The stop comes behind them: the runner's turn before it takes
            // them, some to the port, the rest into its queues.
            stop_and_finish(&counted, &air, &shared).await;
        };
        stopped(drive(runner.run(&shared, &counted), script).await);
        assert!(runner.homes.queues.is_empty());
        assert!(
            runner.homes.purposes.iter().all(Option::is_none),
            "nothing is out"
        );
        transmit.reclaim();
        assert_eq!(
            supply.free(),
            super::super::network::AP_TX_DEPTH,
            "every memory came back, once"
        );
    }));
}

/// A frame the access point refuses before its preparation draws neither a
/// sequence number nor a packet number, and its memory comes back
/// unchanged. A prepared frame the port refuses comes back through its
/// withdrawn terminal, its memory with it.
#[test]
fn a_refusal_draws_no_number_and_returns_the_memory_whole() {
    embassy_futures::block_on(with_local_slot(async |claim| {
        let counted = Counted::new();
        fixture!(claim, counted.clock.clone() => runner, _air, _device, transmit, supply, _shared);
        let now = || oer_portable_time::Clock::now(&*counted.clock);
        let written = |destination| {
            let Ok(memory) = crate::station::TxSupply::<EthernetFrame>::write(
                &mut supply.clone(),
                frame(destination),
            ) else {
                panic!("the network writes its frame");
            };
            memory
        };
        let length = 14 + payload().len();
        let started = poll_fn(|cx| runner.port.poll_start(cx, supported(), now())).await;
        assert!(started.is_ok());
        // No group key yet: a group frame is refused before anything is
        // numbered or written.
        let memory = written(GROUP);
        let before = memory.clone();
        let sequence = runner.homes.engine.service().current_data_sequence();
        let back = refused!(runner, memory, length);
        assert_eq!(back, before, "the memory is whole, unwritten");
        assert_eq!(
            runner.homes.engine.service().current_data_sequence(),
            sequence
        );
        crate::station::TxSupply::<EthernetFrame>::give(&mut supply.clone(), back);
        transmit.reclaim();
        assert_eq!(supply.free(), super::super::network::AP_TX_DEPTH);
    }));
}

/// Once the group key is in, a frame to no authorized peer is refused
/// before the key numbers anything: its packet numbers and the sequence
/// space stay as they were, and its memory comes back unwritten. A group
/// frame the port then refuses at its submission ends withdrawn: its
/// terminal brings the prepared memory home, to the network, whole.
#[test]
fn a_frame_refused_before_or_at_the_port_returns_its_memory_and_draws_nothing_before() {
    embassy_futures::block_on(with_local_slot(async |claim| {
        let counted = Counted::new();
        fixture!(claim, counted.clock.clone() => runner, air, _device, transmit, supply, _shared);
        let now = || oer_portable_time::Clock::now(&*counted.clock);
        serving!(runner, counted, air);
        let written = |destination| {
            let Ok(memory) = crate::station::TxSupply::<EthernetFrame>::write(
                &mut supply.clone(),
                frame(destination),
            ) else {
                panic!("the network writes its frame");
            };
            memory
        };
        let length = 14 + payload().len();
        let numbers = || {
            runner
                .homes
                .group
                .as_ref()
                .map(|group| group.packet_numbers().last())
        };
        let packet_number = numbers();
        assert!(packet_number.is_some(), "the group key is in");
        let memory = written(PEER);
        let before = memory.clone();
        let sequence = runner.homes.engine.service().current_data_sequence();
        let back = refused!(runner, memory, length);
        assert_eq!(back, before, "the memory is whole, unwritten");
        assert_eq!(
            runner.homes.engine.service().current_data_sequence(),
            sequence
        );
        assert_eq!(
            runner
                .homes
                .group
                .as_ref()
                .map(|group| group.packet_numbers().last()),
            packet_number,
            "no packet number drawn"
        );
        crate::station::TxSupply::<EthernetFrame>::give(&mut supply.clone(), back);
        // A group frame admitted, then refused by the port's submission.
        let at = supply.next();
        let memory = written(GROUP);
        let ticket = runner.homes.tickets.pop().unwrap();
        let index = ticket.index();
        let buffer = runner.port.network_buffer(memory);
        let Ok(admitted) = runner.admit_network(ticket, buffer, length, WmmUserPriority::UP0)
        else {
            panic!("the group frame is admitted");
        };
        air.fail_after_terminals();
        let queue = QueueId::new(GROUP_PEER, WmmUserPriority::UP0).unwrap();
        assert!(
            runner
                .hand_over(index, admitted, Purpose::Data(queue, None), now())
                .is_ok()
        );
        assert!(
            runner.homes.purposes.iter().all(Option::is_none),
            "it ended at once"
        );
        transmit.reclaim();
        assert_eq!(
            supply.free(),
            super::super::network::AP_TX_DEPTH,
            "the prepared memory came back"
        );
        let back = supply.memory(at).expect("the same memory");
        assert_eq!(back[40..][..payload().len()], payload(), "its MPDU, whole");
        assert!(data_mpdus(&air).is_empty(), "nothing went on the air");
    }));
}
