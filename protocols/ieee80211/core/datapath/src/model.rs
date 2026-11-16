//! The model port of one running installation, for the engine's tests.

extern crate std;

use core::task::{Context, Waker};

use oer_portable_time::Instant;
use oer_protocols_ieee80211_contracts_port::{
    Channel, ChannelWidth, DataPort, GateOpen, PortPools, RadioPort, RegulatedChannel, StartPoll,
    SupportedChannel,
};
use oer_protocols_ieee80211_testing_model::{ModelCaps, ModelLayout, ModelPort, ModelRights};
use oer_protocols_radio_port::{Mint, branded};

/// The station's single places.
pub(crate) const SINGLES: usize = 8;

/// A model installation, its radio running and its gate open.
pub(crate) struct Installed<'id> {
    pub port: ModelPort<'id, SINGLES>,
    pub data: ModelRights<'id, SINGLES>,
    pub gate: GateOpen<'id>,
}

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

/// Run `body` over a fresh model installation, running, its gate open.
pub(crate) fn installed<R>(body: impl for<'id> FnOnce(Installed<'id>) -> R) -> R {
    let future = core::pin::pin!(branded(
        async |mint: Mint<'_, PortPools<ModelLayout<SINGLES>>>| {
            let install = mint.into_pools();
            let mut port = ModelPort::new(install.key, install.issuer, channel(1));
            let mut cx = Context::from_waker(Waker::noop());
            let starting = port.start(install.stopped, channel(6), Instant::EPOCH);
            let StartPoll::Running(_running) = port.poll_start(starting, &mut cx, Instant::EPOCH)
            else {
                panic!("the model starts at once");
            };
            let gate = port.open_gate(install.gate);
            body(Installed {
                port,
                data: install.data,
                gate,
            })
        }
    ));
    let mut cx = Context::from_waker(Waker::noop());
    match core::future::Future::poll(future, &mut cx) {
        core::task::Poll::Ready(result) => result,
        core::task::Poll::Pending => panic!("an installation's mint performs no async operation"),
    }
}
