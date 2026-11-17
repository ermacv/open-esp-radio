//! The model port of one running installation, for the engine's tests.

extern crate std;

use core::task::{Context, Waker};

use oer_portable_time::Instant;
use oer_protocols_ieee80211_contracts_port::{
    Channel, ChannelWidth, DataPort, GateClosed, GateOpen, IdleArena, PortPools, RadioPort,
    RegulatedChannel, StartPoll, SupportedChannel,
};
use oer_protocols_ieee80211_testing_model::{ModelCaps, ModelLayout, ModelPort, ModelRights};
use oer_protocols_radio_port::{Mint, branded};

/// The station's single places.
pub(crate) const SINGLES: usize = 8;

/// A model installation, its radio running and its gate open.
pub(crate) struct Installed<'id> {
    pub port: ModelPort<'id, SINGLES, 0>,
    /// The two arenas of the data's rights.
    pub arenas: [IdleArena<'id>; 2],
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
        async |mint: Mint<'_, PortPools<ModelLayout<SINGLES, 0>>>| {
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
                arenas: install.data.arenas,
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

/// A model installation of `S` single's places, its radio running and its
/// gate closed, as the controller holds it.
pub(crate) struct Running<'id, const S: usize, const D: usize> {
    pub port: ModelPort<'id, S, D>,
    pub data: ModelRights<'id, S, D>,
    pub gate: GateClosed<'id>,
}

/// Run `body` over a fresh model installation of `S` single's places,
/// running, its gate closed.
pub(crate) fn running<const S: usize, const D: usize, R>(
    body: impl for<'id> FnOnce(Running<'id, S, D>) -> R,
) -> R {
    let future = core::pin::pin!(branded(
        async |mint: Mint<'_, PortPools<ModelLayout<S, D>>>| {
            let install = mint.into_pools();
            let mut port = ModelPort::new(install.key, install.issuer, channel(1));
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
        }
    ));
    let mut cx = Context::from_waker(Waker::noop());
    match core::future::Future::poll(future, &mut cx) {
        core::task::Poll::Ready(result) => result,
        core::task::Poll::Pending => panic!("an installation's mint performs no async operation"),
    }
}
