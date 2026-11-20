//! The scripted peers of the advertiser, the scanner and the test session.

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

use harness::{
    advertising_event, at, installed, power, running, scan_window, scannable, scanner, test_window,
};
use oer_protocols_bluetooth_le_contracts_port::{
    EventEnd, EventPlace, Executed, LePort, RoleRights, Supported, TestPhy, dtm::DtmTest,
};
use oer_protocols_bluetooth_le_testing_model::{Role, Turn};
use oer_protocols_bluetooth_le_wire::{TestChannel, dtm::DtmPayloadPattern};

const SCAN_REQ: [u8; 14] = [0x03, 12, 9, 9, 9, 9, 9, 9, 1, 2, 3, 4, 5, 6];

#[test]
fn a_scanners_request_reaches_the_advertising_events_end() {
    installed::<1, 0, 0, 2, 0, 2, 0>(|port, stopped, mut right, roles| {
        let RoleRights {
            advertising: [place],
            scanners,
            connections,
            tests,
            accept_list,
        } = roles;
        let (stopped, place) = running(port, stopped, &mut right, |port| {
            let (role, home) = port.open_advertising(place, &scannable());
            port.script(Role::Advertising, 0, Turn::Request(SCAN_REQ.to_vec()));
            let _published =
                port.publish_advertising(home, &advertising_event(at(2_000), port.now()));
            port.advance(at(7_000));
            let mut round = port.round();
            let mut ended = round.ended().unwrap();
            assert!(matches!(
                ended.end(),
                EventEnd::Executed(Executed { exchanged: 4, .. })
            ));
            let pdu = ended.received().expect("the request");
            assert_eq!(pdu.view().header, [0x03, 12]);
            assert_eq!(pdu.view().payload, &SCAN_REQ[2..]);
            drop(pdu);
            assert!(ended.received().is_none());
            let EventPlace::Advertising(home) = ended.place() else {
                panic!("the set's place");
            };
            let _rest = round.close();
            assert_eq!(
                port.free_nodes(),
                4,
                "a reception not taken goes back at the close"
            );
            port.close_advertising(role, home)
        });
        let roles = RoleRights {
            advertising: [place],
            scanners,
            connections,
            tests,
            accept_list,
        };
        let retired = port.retire(stopped, roles);
        port.release(retired, right);
    });
}

#[test]
fn a_scan_window_hears_the_advertisers_in_the_airs_order_while_nodes_are_free() {
    installed::<0, 1, 0, 2, 0, 2, 0>(|port, stopped, mut right, roles| {
        let RoleRights {
            advertising,
            scanners: [place],
            connections,
            tests,
            accept_list,
        } = roles;
        let (stopped, place) = running(port, stopped, &mut right, |port| {
            let (role, home) = port.open_scanner(place, &scanner());
            port.script(
                Role::Scan,
                0,
                Turn::Advertisements(vec![vec![0x02, 1, 1], vec![0x02, 1, 2], vec![0x02, 1, 3]]),
            );
            let _published = port.publish_scan(home, &scan_window(at(2_000), port.now()));
            port.advance(at(12_000));
            let mut round = port.round();
            let mut ended = round.ended().unwrap();
            assert!(matches!(
                ended.end(),
                EventEnd::Executed(Executed { exchanged: 2, .. })
            ));
            let mut heard = Vec::new();
            while let Some(pdu) = ended.received() {
                heard.push(pdu.view().payload.to_vec());
            }
            assert_eq!(
                heard,
                [vec![1], vec![2]],
                "two nodes: the third is not taken"
            );
            let EventPlace::Scan(home) = ended.place() else {
                panic!("the scanner's place");
            };
            let _rest = round.close();
            port.close_scanner(role, home)
        });
        assert_eq!(port.free_nodes(), 4);
        let roles = RoleRights {
            advertising,
            scanners: [place],
            connections,
            tests,
            accept_list,
        };
        let retired = port.retire(stopped, roles);
        port.release(retired, right);
    });
}

#[test]
fn a_test_window_counts_the_packets_sent_or_received() {
    installed::<0, 0, 0, 2, 0, 0, 1>(|port, stopped, mut right, roles| {
        let RoleRights {
            advertising,
            scanners,
            connections,
            tests: [place],
            accept_list,
        } = roles;
        let receive = Supported::new(DtmTest::Receive {
            channel: TestChannel::new(0).unwrap(),
            phy: TestPhy::Le1M,
        })
        .unwrap();
        let transmit = Supported::new(DtmTest::Transmit {
            channel: TestChannel::new(0).unwrap(),
            phy: TestPhy::Le1M,
            pattern: DtmPayloadPattern::Prbs9,
            length: 37,
            power: power(),
        })
        .unwrap();
        let (stopped, place) = running(port, stopped, &mut right, |port| {
            let mut exchanged = Vec::new();
            let mut place = place;
            for (setup, script) in [(&receive, Some(5)), (&transmit, None)] {
                let (role, home) = port.open_test(place, setup);
                if let Some(packets) = script {
                    port.script(Role::Test, 0, Turn::Packets(packets));
                }
                let start = port.now().as_micros() + 2_000;
                let _published = port.publish_test(home, &test_window(at(start), port.now()));
                port.advance(at(start + 1_000));
                let mut round = port.round();
                let ended = round.ended().unwrap();
                if let EventEnd::Executed(executed) = ended.end() {
                    exchanged.push(executed.exchanged);
                }
                let EventPlace::Test(home) = ended.place() else {
                    panic!("the test's place");
                };
                let _rest = round.close();
                place = port.close_test(role, home);
            }
            assert_eq!(exchanged, [5, 1], "five received; one sent in its window");
            place
        });
        let roles = RoleRights {
            advertising,
            scanners,
            connections,
            tests: [place],
            accept_list,
        };
        let retired = port.retire(stopped, roles);
        port.release(retired, right);
    });
}
