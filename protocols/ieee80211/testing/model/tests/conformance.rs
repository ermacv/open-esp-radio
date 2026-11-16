//! The conformance suite over the model port.

#![allow(
    clippy::disallowed_methods,
    clippy::disallowed_macros,
    clippy::unwrap_used,
    clippy::indexing_slicing,
    reason = "a test: its harness polls with a no-op waker and fails by panicking"
)]

use oer_protocols_ieee80211_contracts_port::{
    Channel, ChannelWidth, MpduLength, PortPools, RadioPort, RegulatedChannel, RxMeta,
    SupportedChannel,
};
use oer_protocols_ieee80211_testing_conformance as suite;
use oer_protocols_ieee80211_testing_model::{
    Air, Anomaly, HardwareState, MODEL_AGREEMENTS, MODEL_GROUPS, MODEL_PLACES,
    MODEL_SHORT_AMPDU_MCS, ModelCaps, ModelLayout, ModelLink, ModelMemory, ModelPort, Programmed,
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

struct Model<'id> {
    port: ModelPort<'id, SINGLES>,
    memory: u32,
}

impl<'id> suite::Harness<'id, SINGLES, MODEL_GROUPS, MODEL_AGREEMENTS> for Model<'id> {
    type Port = ModelPort<'id, SINGLES>;
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
        }
    }
    fn link(&self) -> ModelLink {
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
}

/// Run `body` with a fresh installation of the model.
fn installed(
    body: impl for<'id> FnOnce(Model<'id>, suite::Rights<'id, SINGLES, MODEL_GROUPS, MODEL_AGREEMENTS>),
) {
    let future = core::pin::pin!(branded(
        async |mint: Mint<'_, PortPools<ModelLayout<SINGLES>>>| {
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
        }
    ));
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
    suite::no_call_returns_an_error::<ModelPort<'static, SINGLES>>();
}
