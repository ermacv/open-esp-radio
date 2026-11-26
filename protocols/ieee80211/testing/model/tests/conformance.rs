//! The conformance suite over the model port.
#![cfg_attr(
    test,
    expect(
        clippy::cast_possible_truncation,
        reason = "test code: a cast or a dropped result in a test loses no product's value"
    )
)]
#![allow(
    clippy::disallowed_methods,
    reason = "a test: its harness polls with a no-op waker and fails by panicking"
)]
#![allow(
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros,
    clippy::indexing_slicing,
    clippy::unwrap_used,
    reason = "test code: a test may panic"
)]

use oer_protocols_ieee802_wire::Eui48;
use oer_protocols_ieee80211_contracts_port::{
    Channel, ChannelWidth, EthernetAddresses, InterfaceRole, MpduLength, PortPools, RadioPort,
    ReceiveFilter, RegulatedChannel, Round, RxMeta, Station, StationReceive, SupportedChannel,
    SupportedFilter,
};
use oer_protocols_ieee80211_testing_conformance as suite;
use oer_protocols_ieee80211_testing_model::{
    AccessPointModel, Air, Anomaly, Fault, HardwareState, MODEL_PLACES, MODEL_SHORT_AMPDU_MCS,
    MODEL_TX_BODY, ModelCaps, ModelLink, ModelMemory, ModelPort, Programmed, Served, StationModel,
    StationModelOf,
};
use oer_protocols_radio_port::{Mint, branded};

/// The single places of the station: its eight purposes.
const SINGLES: usize = 8;

const ANOMALIES: [Anomaly; 4] = [
    Anomaly::MacFatal,
    Anomaly::CompletionStatus,
    Anomaly::WalkerLast,
    Anomaly::NoCompletion,
];

fn channel(number: u8) -> SupportedChannel<ModelCaps> {
    SupportedChannel::new(
        oer_protocols_radio_regulatory::TEST_DOMAIN
            .permit(RegulatedChannel::from(
                Channel::ghz2_4(number, ChannelWidth::Mhz20).unwrap(),
            ))
            .unwrap(),
    )
    .unwrap()
}

struct Model<'id, L: Served = StationModel<SINGLES>> {
    port: ModelPort<'id, L>,
    memory: u32,
}

impl<'id, L: Served> suite::Harness<'id, L> for Model<'id, L> {
    type Port = ModelPort<'id, L>;
    type Record = Vec<Programmed>;
    type State = HardwareState;

    fn port(&mut self) -> &mut Self::Port {
        &mut self.port
    }
    fn channel(&self) -> <Self::Port as RadioPort<'id>>::Channel {
        channel(6)
    }
    fn memory(&mut self, len: u16) -> ModelMemory {
        self.memory += 1;
        ModelMemory {
            id: self.memory,
            len,
            front: [0; oer_protocols_ieee80211_contracts_port::HEADROOM],
        }
    }
    fn link<I: InterfaceRole>(&self) -> ModelLink {
        ModelLink { protected: false }
    }
    fn land(&mut self, bytes: &[u8]) {
        self.port
            .land(bytes, RxMeta::unavailable(channel(6).channel()));
    }
    fn anomalies(&self) -> usize {
        ANOMALIES.len()
    }
    fn raise(&mut self, index: usize) {
        self.port.raise(ANOMALIES[index]);
    }
    fn hold_busy(&mut self) {
        self.port.hold_busy();
    }
    fn expire_next(&mut self) {
        self.port.expire_next();
    }
    fn chains_on(&self, category: oer_protocols_ieee80211_wire::qos::WmmAccessCategory) -> usize {
        self.port.chains_on(category)
    }
    fn hold_air(&mut self) {
        for category in oer_protocols_ieee80211_contracts_port::QUEUES {
            self.port.set_air(category, Air::Hold);
        }
    }
    fn release_air(&mut self) {
        for category in oer_protocols_ieee80211_contracts_port::QUEUES {
            self.port.set_air(category, Air::Ack);
        }
    }
    fn record(&self) -> Vec<Programmed> {
        self.port.record().to_vec()
    }
    fn state(&self) -> HardwareState {
        self.port.state()
    }
    fn port_octets(&self) -> suite::PortOctets {
        if self.port.counts().overwritten == 0 {
            suite::PortOctets::Whole
        } else {
            suite::PortOctets::Overwritten
        }
    }
}

/// Run `body` with a fresh installation of the model's station.
fn installed(body: impl for<'id> FnOnce(Model<'id>, suite::Rights<'id, StationModel<SINGLES>>)) {
    installed_as::<StationModel<SINGLES>>(body);
}

/// Run `body` with a fresh installation of the model of the layout `L`.
fn installed_as<L: Served>(body: impl for<'id> FnOnce(Model<'id, L>, suite::Rights<'id, L>)) {
    let future = core::pin::pin!(branded(async |mint: Mint<'_, PortPools<L>>| {
        let install = mint.into_pools();
        let model = Model {
            port: ModelPort::new(install.key, install.issuer, channel(1)),
            memory: 0,
        };
        body(
            model,
            suite::Rights {
                stopped: Some(install.stopped),
                right: install.right,
                data: Some(install.data),
                gate: Some(install.gate),
            },
        );
    }));
    let mut cx = core::task::Context::from_waker(core::task::Waker::noop());
    assert!(core::future::Future::poll(future, &mut cx).is_ready());
}

#[test]
fn every_right_is_in_exactly_one_place() {
    installed(|mut model, rights| suite::every_right_is_in_exactly_one_place(&mut model, rights));
}

#[test]
fn a_second_run_is_as_the_first() {
    installed(|mut model, rights| suite::a_second_run_is_as_the_first(&mut model, rights));
}

#[test]
fn the_indication_holds_until_the_stop() {
    installed(|mut model, rights| suite::the_indication_holds_until_the_stop(&mut model, rights));
}

#[test]
fn a_unit_is_in_one_place() {
    installed(|mut model, rights| suite::a_unit_is_in_one_place(&mut model, rights));
}

#[test]
fn a_round_services_the_ring_once() {
    installed(|mut model, rights| {
        suite::a_round_services_the_ring_once(&mut model, rights, MODEL_PLACES, |model| {
            model.port.counts().services
        });
    });
}

#[test]
fn a_publication_sends_the_prefix_its_rate_admits() {
    // Six frames of 1 500 octets at a rate whose aggregate holds 6 500.
    installed(|mut model, rights| {
        suite::a_publication_sends_the_prefix_its_rate_admits(
            &mut model,
            rights,
            MpduLength::new(1_526).unwrap(),
            1_500,
            6,
        );
    });
}

#[test]
fn a_single_publication_sends_the_first_frame() {
    installed(|mut model, rights| {
        suite::a_single_publication_sends_the_first_frame(
            &mut model,
            rights,
            MpduLength::new(1_526).unwrap(),
            1_500,
        );
    });
}

#[test]
fn the_ports_octets_stay_the_ports() {
    // Three frames of the longest body the model's slots hold.
    installed(|mut model, rights| {
        suite::the_ports_octets_stay_the_ports(
            &mut model,
            rights,
            MpduLength::new(MODEL_TX_BODY + 26).unwrap(),
            MODEL_TX_BODY as u16,
        );
    });
}

#[test]
fn an_aggregate_of_one_frame_ends_single() {
    // Two frames of 2 000 octets where the aggregate holds 3 000.
    installed(|mut model, rights| {
        suite::an_aggregate_of_one_frame_ends_single(
            &mut model,
            rights,
            MpduLength::new(2_026).unwrap(),
            2_000,
            MODEL_SHORT_AMPDU_MCS,
        );
    });
}

#[test]
fn an_unclassifiable_frame_goes_back_at_its_staging() {
    installed(|mut model, rights| {
        suite::an_unclassifiable_frame_goes_back_at_its_staging(&mut model, rights, |model| {
            model.port.counts().dropped
        });
    });
}

#[test]
fn a_stop_ends_everything_on_the_air() {
    installed(|mut model, rights| suite::a_stop_ends_everything_on_the_air(&mut model, rights));
}

#[test]
fn no_call_returns_an_error() {
    suite::no_call_returns_an_error::<ModelPort<'static, StationModel<SINGLES>>>();
}

/// The addresses lie in the unit's twelve octets before the EtherType, and
/// nowhere else; without those octets nothing is written.
#[test]
fn a_units_addresses_are_written_before_its_ether_type() {
    installed(|mut model, _rights| {
        let mut frame = [0; 40];
        // Data, From DS.
        frame[..2].copy_from_slice(&[0x08, 0x02]);
        model
            .port
            .land(&frame, RxMeta::unavailable(channel(1).channel()));
        let mut round = Round::open(&mut model.port, oer_portable_time::Instant::EPOCH);
        let handle = round.staged().expect("the ring held a unit").take();
        let addresses = |ether_type_at| EthernetAddresses {
            destination: Eui48::new([1, 2, 3, 4, 5, 6]),
            source: Eui48::new([7, 8, 9, 10, 11, 12]),
            ether_type_at,
        };
        for outside in [11, 41] {
            assert!(round.write_addresses(&handle, addresses(outside)).is_err());
        }
        assert!(round.write_addresses(&handle, addresses(30)).is_ok());
        let mut expected = frame;
        expected[18..30].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]);
        assert_eq!(round.unit(&handle).unwrap().bytes, expected);
        round.release(handle);
        let _ = round.close();
    });
}

/// An access point of two clients: each client's key goes alone.
#[test]
fn a_clients_key_goes_alone() {
    installed_as::<AccessPointModel<SINGLES, 2>>(|mut model, rights| {
        suite::a_clients_key_goes_alone(&mut model, rights);
    });
}

/// The station's receive rule, set, changed and ended, and a setting that
/// expires.
#[test]
fn the_receive_rule_is_a_right() {
    installed(|mut model, rights| {
        let bss =
            oer_protocols_ieee802_wire::IndividualAddress::new(Eui48::new([2, 0, 0, 0, 0, 9]))
                .unwrap();
        let filter = SupportedFilter::<ModelCaps, Station>::new(ReceiveFilter::BSS_MEMBER).unwrap();
        suite::the_receive_rule_is_a_right(
            &mut model,
            rights,
            oer_protocols_ieee802_wire::IndividualAddress::new(Eui48::new([2, 0, 0, 0, 0, 1]))
                .unwrap(),
            StationReceive::OtherBssManagement,
            StationReceive::Bss { bssid: bss, filter },
        );
    });
}

/// A chain on best effort and a chain of one on voice, each on its own
/// queue, each returning its own queue's right.
#[test]
fn chains_on_two_queues_end_each_with_its_queue() {
    installed(|mut model, rights| {
        suite::chains_on_two_queues_end_each_with_its_queue(&mut model, rights);
    });
}

/// The case fails on a model that puts every chain on best effort: a
/// second chain on a busy queue.
#[test]
#[should_panic(expected = "each chain is on the queue it was published on")]
fn chains_on_two_queues_fails_when_the_model_puts_both_on_one() {
    installed(|mut model, rights| {
        model.port.fault(Fault::ChainsOnBestEffort);
        suite::chains_on_two_queues_end_each_with_its_queue(&mut model, rights);
    });
}

/// A stop with chains on two queues returns every chain, every queue's
/// right and every frame's memory.
#[test]
fn a_stop_returns_every_chain_and_queue() {
    installed(|mut model, rights| {
        suite::a_stop_returns_every_chain_and_queue(&mut model, rights);
    });
}

/// The case fails on a model whose stop leaves the chains on the air.
#[test]
#[should_panic(expected = "the long chain ended")]
fn a_stop_returns_every_chain_and_queue_fails_when_the_stop_leaves_them() {
    installed(|mut model, rights| {
        model.port.fault(Fault::StopLeavesChains);
        suite::a_stop_returns_every_chain_and_queue(&mut model, rights);
    });
}

/// A layout whose long chains have one descriptor builds, and its chains of
/// both kinds are on the air together, each in its own buffer and on its
/// own queue: the radio numbers its chains once.
#[test]
fn chains_of_one_descriptor_of_both_kinds_are_apart() {
    installed_as::<StationModelOf<SINGLES, 1>>(|mut model, rights| {
        suite::chains_on_two_queues_end_each_with_its_queue(&mut model, rights);
    });
}
