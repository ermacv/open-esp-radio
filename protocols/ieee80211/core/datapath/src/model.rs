//! The model port of one running installation, for the engine's tests.

extern crate std;

use core::task::{Context, Waker};

use oer_portable_time::Instant;
use oer_protocols_ieee80211_contracts_port::{
    Channel, ChannelWidth, DataPort, First, GateClosed, GateOpen, LongChain, PortPools, Queue,
    RadioPort, RegulatedChannel, Second, StartPoll, SupportedChannel,
};
use oer_protocols_ieee80211_testing_model::{
    MODEL_CHAIN_SLOTS, MODEL_PLACES, ModelCaps, ModelPort, ModelRights, Served, StationModel,
};
use oer_protocols_radio_port::{Mint, branded};

/// A model installation, its radio running and its gate open.
pub(crate) struct Installed<'id> {
    pub port: ModelPort<'id, StationModel>,
    /// The two long chains of the radio's rights.
    pub arenas: (
        LongChain<'id, First, MODEL_CHAIN_SLOTS>,
        LongChain<'id, Second, MODEL_CHAIN_SLOTS>,
    ),
    /// The four queues.
    pub queues: [Queue<'id>; 4],
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
    let future = core::pin::pin!(branded(async |mint: Mint<'_, PortPools<StationModel>>| {
        let install = mint.into_pools();
        let mut port = ModelPort::new(install.key, install.issuer, channel(1));
        let mut cx = Context::from_waker(Waker::noop());
        let starting = port.start(install.stopped, channel(6), Instant::EPOCH);
        let StartPoll::Running(_running) = port.poll_start(starting, &mut cx, Instant::EPOCH)
        else {
            panic!("the model starts at once");
        };
        let gate = port.open_gate(install.gate);
        let (radio, _station) = install.data;
        body(Installed {
            port,
            arenas: radio.chains,
            queues: radio.queues,
            gate,
        })
    }));
    let mut cx = Context::from_waker(Waker::noop());
    match core::future::Future::poll(future, &mut cx) {
        core::task::Poll::Ready(result) => result,
        core::task::Poll::Pending => panic!("an installation's mint performs no async operation"),
    }
}

/// A model installation of the layout `L`, its radio running and its gate
/// closed, as the controller holds it.
pub(crate) struct Running<'id, L: Served, const P: usize = MODEL_PLACES> {
    pub port: ModelPort<'id, L, P>,
    pub data: ModelRights<'id, L>,
    pub gate: GateClosed<'id>,
}

/// Run `body` over a fresh model installation of the layout `L`, running,
/// its gate closed.
pub(crate) fn running<L: Served, R>(body: impl for<'id> FnOnce(Running<'id, L>) -> R) -> R {
    running_on::<L, MODEL_PLACES, R>(body)
}

/// The same over a model port of `P` staging places.
pub(crate) fn running_on<L: Served, const P: usize, R>(
    body: impl for<'id> FnOnce(Running<'id, L, P>) -> R,
) -> R {
    let future = core::pin::pin!(branded(async |mint: Mint<'_, PortPools<L>>| {
        let install = mint.into_pools();
        let mut port = ModelPort::<'_, L, P>::new(install.key, install.issuer, channel(1));
        let mut cx = Context::from_waker(Waker::noop());
        let starting = port.start(install.stopped, channel(6), Instant::EPOCH);
        let StartPoll::Running(_running) = port.poll_start(starting, &mut cx, Instant::EPOCH)
        else {
            panic!("the model starts at once");
        };
        body(Running {
            port,
            data: install.data,
            gate: install.gate,
        })
    }));
    let mut cx = Context::from_waker(Waker::noop());
    match core::future::Future::poll(future, &mut cx) {
        core::task::Poll::Ready(result) => result,
        core::task::Poll::Pending => panic!("an installation's mint performs no async operation"),
    }
}
