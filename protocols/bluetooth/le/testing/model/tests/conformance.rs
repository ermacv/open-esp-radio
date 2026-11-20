//! The conformance suite over the model port.

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

use oer_protocols_bluetooth_le_contracts_port::{
    AdvertisingSet, ChannelSpacing, ConnectionSetup, Install, LeInstant, PortPools, ScannerSetup,
    StartFailure, Supported, TestPhy, dtm::DtmTest,
};
use oer_protocols_bluetooth_le_testing_conformance as suite;
use oer_protocols_bluetooth_le_testing_model::{
    Ack, Anomaly, EventId, HardwareState, Heard, ModelCaps, ModelPort, Programmed, Role,
    StartScript, Turn,
};
use oer_protocols_bluetooth_le_wire::TestChannel;
use oer_protocols_radio_port::{Mint, branded};

/// Two advertising sets, a scanner, two connections of two events, three
/// transmit places and two receive nodes, and a test session.
const A: usize = 2;
const S: usize = 1;
const C: usize = 2;
const E: usize = 2;
const T: usize = 3;
const R: usize = 2;
const X: usize = 1;

const ANOMALIES: [Anomaly; 5] = [
    Anomaly::ExceptionLane,
    Anomaly::Sentinel,
    Anomaly::UnexecutedLeft,
    Anomaly::LockAnswer,
    Anomaly::Bound,
];

struct Model<'id> {
    port: ModelPort<'id, A, S, C, E, T, R, X>,
}

const fn role(role: suite::Role) -> Role {
    match role {
        suite::Role::Advertising => Role::Advertising,
        suite::Role::Scan => Role::Scan,
        suite::Role::Connection => Role::Connection,
        suite::Role::Test => Role::Test,
    }
}

impl<'id> suite::Harness<'id, A, S, C, E, T, R, X> for Model<'id> {
    type Caps = ModelCaps;
    type Port = ModelPort<'id, A, S, C, E, T, R, X>;
    type Record = Vec<Programmed>;
    type State = HardwareState;

    fn port(&mut self) -> &mut Self::Port {
        &mut self.port
    }
    fn now(&self) -> LeInstant {
        self.port.now()
    }
    fn advance(&mut self, to: LeInstant) {
        self.port.advance(to);
    }
    fn peer(&mut self, of: suite::Role, instance: u8, peer: suite::Peer) {
        let turn = match peer {
            suite::Peer::Request(pdu) => Turn::Request(pdu),
            suite::Peer::Advertisements(heard) => Turn::Advertisements(heard),
            suite::Peer::Central { heard, ack, sends } => Turn::Central {
                heard: match heard {
                    suite::Heard::Heard => Heard::Heard,
                    suite::Heard::Lost => Heard::Lost,
                },
                ack: match ack {
                    suite::Ack::Acknowledge => Ack::Acknowledge,
                    suite::Ack::Withhold => Ack::Withhold,
                },
                sends,
            },
            suite::Peer::Overrun { exchanged } => Turn::Overrun { exchanged },
            suite::Peer::Packets(packets) => Turn::Packets(packets),
        };
        self.port.script(role(of), instance, turn);
    }
    fn displace(&mut self, event: suite::EventId) {
        self.port.displace(EventId {
            role: role(event.role),
            instance: event.instance,
            slot: event.slot,
        });
    }
    fn anomalies(&self) -> usize {
        ANOMALIES.len()
    }
    fn raise(&mut self, index: usize) {
        self.port.raise(ANOMALIES[index]);
    }
    fn fail_next_start(&mut self, failure: StartFailure) {
        self.port.script_start(StartScript::Fail(failure));
    }
    fn record(&self) -> Vec<Programmed> {
        self.port.record()
    }
    fn state(&self) -> HardwareState {
        self.port.state()
    }
    fn advertising_set(&self) -> Supported<ModelCaps, AdvertisingSet<'static>> {
        harness::nonconnectable()
    }
    fn scanner(&self) -> Supported<ModelCaps, ScannerSetup> {
        harness::scanner()
    }
    fn link(&self) -> Supported<ModelCaps, ConnectionSetup> {
        harness::link()
    }
    fn test(&self) -> Supported<ModelCaps, DtmTest> {
        Supported::new(DtmTest::Receive {
            channel: TestChannel::new(0).unwrap(),
            phy: TestPhy::Le1M,
        })
        .unwrap()
    }
    fn spacing(&self) -> ChannelSpacing<ModelCaps> {
        harness::SPACING
    }
}

/// Run `property` with a fresh installation of the model.
fn installed(property: impl for<'id> FnOnce(&mut Model<'id>, suite::Rights<'id, A, S, C, X>)) {
    let future = core::pin::pin!(branded(
        async |mint: Mint<'_, PortPools<ModelCaps, A, S, C, E, T, R, X>>| {
            let Install {
                key,
                stopped,
                right,
                roles,
            } = mint.into_pools();
            let mut model = Model {
                port: ModelPort::new(key, harness::at(0)),
            };
            property(
                &mut model,
                suite::Rights {
                    stopped,
                    right,
                    roles,
                },
            );
            assert_eq!(
                model.port.unheld_reads(),
                0,
                "no read of a node a handle does not hold"
            );
        }
    ));
    assert!(core::future::Future::poll(future, &mut harness::cx()).is_ready());
}

#[test]
fn every_right_is_in_exactly_one_place() {
    installed(|model, rights| suite::every_right_is_in_exactly_one_place(model, rights));
}

#[test]
fn an_event_ends_once_as_what_happened() {
    installed(|model, rights| suite::an_event_ends_once_as_what_happened(model, rights));
}

#[test]
fn events_end_in_the_airs_order() {
    installed(|model, rights| suite::events_end_in_the_airs_order(model, rights));
}

#[test]
fn a_withdraw_is_of_one_instance() {
    installed(|model, rights| suite::a_withdraw_is_of_one_instance(model, rights));
}

#[test]
fn the_chain_leaves_in_the_order_submitted() {
    installed(|model, rights| suite::the_chain_leaves_in_the_order_submitted(model, rights));
}

#[test]
fn a_received_pdu_is_in_one_place() {
    installed(|model, rights| suite::a_received_pdu_is_in_one_place(model, rights));
}

#[test]
fn the_indication_holds_until_the_stop() {
    installed(|model, rights| suite::the_indication_holds_until_the_stop(model, rights));
}

#[test]
fn a_stop_ends_everything() {
    installed(|model, rights| suite::a_stop_ends_everything(model, rights));
}

#[test]
fn a_second_run_is_as_the_first() {
    installed(|model, rights| suite::a_second_run_is_as_the_first(model, rights));
}

#[test]
fn no_call_returns_an_error() {
    suite::no_call_returns_an_error::<
        'static,
        ModelPort<'static, A, S, C, E, T, R, X>,
        A,
        S,
        C,
        E,
        T,
        R,
        X,
    >();
}
