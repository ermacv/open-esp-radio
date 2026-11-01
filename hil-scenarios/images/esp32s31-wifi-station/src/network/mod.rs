//! Hart 1's composition: the station's network end, the IP stack over it
//! and the load server with its sockets, in one task of hart 1's
//! interrupt-mode executor (`FROM_CPU1`, P2 of hart 1).
//!
//! The task waits for the station's network start: once the boot's join
//! constructed the station on the boot hart, the station work hands this
//! task the station's network end ([`Start`]). The task starts the IP
//! stack ([`ip::start`]), answers whether it started, lends the load server
//! its interface and runs the IP service and the station's network stop
//! until the station's exit asks it to stop the service ([`EXIT`]). Every
//! end of the station's network channels, of the start and of the load's
//! queues on this hart is polled by this one task: they all notify through
//! its one signal (`crate::peers::NETWORK`).

pub(crate) mod ip;

use core::cell::Cell;
use core::convert::Infallible;
use core::pin::{Pin, pin};

use embassy_futures::{join::join as both, select::select};
use oer_espressif_interrupt_table::signal::PeerNotify;
use oer_esp32s31_compose::Esp32s31;
use oer_esp32s31_wifi as wifi;
use oer_memory::{PlacedFuture, StaticPlace, WakingSpscQueue, WakingSpscReceiver, WakingSpscSender};
use static_cell::ConstStaticCell;

use crate::fatal::Failure;
use crate::handoff::{Exit, Lease};
use crate::load::parts::{Part, Polled};
use crate::load::{relay, serve};
use crate::peers::{self, NetworkNotify, StationNotify};
use crate::station::future::{Boundary, Tracked};

oer_espressif_executor::tasks! {
    /// Hart 1's tasks (P2 of hart 1): the station's network end, its IP
    /// stack and the load server.
    pub(crate) NetworkTasks: InterruptExecutor<Esp32s31, 1> {
        @observer: crate::load::cpu::Observer::<{ crate::load::cpu::NETWORK }>;
        network(inputs: Inputs) = run;
    }
}

/// What the station hands the network once its join constructed it.
pub(crate) struct Start {
    pub net: wifi::Network<'static, crate::station::radio::Notifies>,
    pub stopper: wifi::NetworkStopper<'static, crate::station::radio::Notifies>,
    pub address: [u8; 6],
    pub seed: u64,
}

/// Whether the IP stack started over the station's network end.
type Started = Result<(), oer_embassy_net::ip::Error>;

type StartQueue = WakingSpscQueue<Start, 1, NetworkNotify, StationNotify>;
type StartedQueue = WakingSpscQueue<Started, 1, StationNotify, NetworkNotify>;
static START: ConstStaticCell<StartQueue> =
    ConstStaticCell::new(WakingSpscQueue::across_harts(&peers::NETWORK, &peers::STATION));
static STARTED: ConstStaticCell<StartedQueue> =
    ConstStaticCell::new(WakingSpscQueue::across_harts(&peers::STATION, &peers::NETWORK));

/// The station's exit and the IP service's end.
pub(crate) static EXIT: Exit<'static, PeerNotify<peers::NetworkExecutor>, PeerNotify<peers::AppExecutor>> =
    Exit::new(&peers::NETWORK, &peers::STATION);
/// The lease the IP service holds, which the station's status reads.
pub(crate) static LEASE: Lease<'static, PeerNotify<peers::AppExecutor>> =
    Lease::new(&peers::STATION);

/// The station work's ends, on the boot hart.
pub(crate) struct Station {
    start: WakingSpscSender<'static, Start, 1, NetworkNotify, StationNotify>,
    started: WakingSpscReceiver<'static, Started, 1, StationNotify, NetworkNotify>,
}

/// Hart 1's network task's ends.
pub(crate) struct Inputs {
    start: WakingSpscReceiver<'static, Start, 1, NetworkNotify, StationNotify>,
    started: WakingSpscSender<'static, Started, 1, StationNotify, NetworkNotify>,
    load: serve::Receiver,
    hart0: relay::Client,
}

/// Both harts' ends, split once: the station's, and hart 1's with the load
/// server's requests and its client of the boot hart's relay.
pub(crate) fn split(load: serve::Receiver, hart0: relay::Client) -> (Station, Inputs) {
    let (Some(start), Some(started)) = (START.try_take(), STARTED.try_take()) else {
        crate::fail(Failure::Storage);
    };
    let (start_send, start_receive) = start.split();
    let (started_send, started_receive) = started.split();
    (
        Station {
            start: start_send,
            started: started_receive,
        },
        Inputs {
            start: start_receive,
            started: started_send,
            load,
            hart0,
        },
    )
}

impl Station {
    /// Hand `start` to hart 1 and wait until its IP stack started over it.
    ///
    /// # Errors
    /// The IP stack's refusal; the network then holds the station's network
    /// end for good.
    pub(crate) async fn start(self, start: Start) -> Started {
        // The one start of the boot: its queue is empty.
        if self.start.try_send(start).is_err() {
            crate::fail(Failure::Storage);
        }
        let mut started = self.started;
        loop {
            started.ready().await;
            if let Some(started) = started.try_receive() {
                return started;
            }
        }
    }
}

/// The station's exit: ask hart 1 to stop the IP service and wait until it
/// ended.
pub(crate) async fn stop() {
    EXIT.request();
    EXIT.ended().await;
}

pub(crate) type Work = impl core::future::Future<Output = ()>;
static WORK: ConstStaticCell<StaticPlace<Work>> = ConstStaticCell::new(StaticPlace::new());

/// Hart 1's network task: its work lives in its own static place, written
/// at the task's first poll.
async fn run(inputs: Inputs) {
    let work = place(inputs);
    Polled {
        part: Part::Network,
        future: work,
    }
    .await;
}

/// Write the work in its static place. The future passes through this
/// function's frame on its way there, once: kept out of line, that frame is
/// not on the path of the task's later polls, which run on hart 1's
/// interrupt stack.
#[inline(never)]
fn place(inputs: Inputs) -> PlacedFuture<'static, Work> {
    WORK.try_take()
        .unwrap_or_else(|| crate::fail(Failure::Storage))
        .future(|| work_future(inputs))
}

#[define_opaque(Work)]
fn work_future(inputs: Inputs) -> Work {
    async move {
        let Inputs {
            start,
            started,
            load,
            hart0,
        } = inputs;
        let network = serve::Network::new();
        let ip = pin!(network_end(start, started, &network));
        let load = pin!(serve::serve(load, &network, &hart0));
        let load = Polled {
            part: Part::Load,
            future: Boundary(load),
        };
        match select(Boundary(ip), load).await {
            embassy_futures::select::Either::First(never)
            | embassy_futures::select::Either::Second(never) => match never {},
        }
    }
}

/// Wait for the station's network end, start the IP stack over it and run
/// it until the station's exit; then stop it and report its end.
async fn network_end(
    mut start: WakingSpscReceiver<'static, Start, 1, NetworkNotify, StationNotify>,
    started: WakingSpscSender<'static, Started, 1, StationNotify, NetworkNotify>,
    network: &serve::Network,
) -> Infallible {
    let Start {
        net,
        stopper,
        address,
        seed,
    } = loop {
        start.ready().await;
        if let Some(start) = start.try_receive() {
            break start;
        }
    };
    let (ip, interface, mut running) = match ip::start(address, seed, net) {
        Ok(ip) => ip,
        Err(error) => {
            answer(&started, Err(error));
            // The station never runs: its stop never comes.
            let _stopper = stopper;
            return core::future::pending().await;
        }
    };
    network.lend(interface);
    answer(&started, Ok(()));
    let done = Cell::new(false);
    {
        let serving = both(
            Polled {
                part: Part::Ip,
                future: Tracked {
                    future: Pin::new(&mut running),
                    done: &done,
                },
            },
            // The station's stop asks the network first: the IP service
            // stops and drops every frame it holds, then the station
            // retires the radio. Polled after the IP part, it sees that
            // part's end in the same round.
            stop_network(stopper, network, &ip, &done),
        );
        // Neither part ends before the exit.
        let _exit = select(serving, EXIT.requested()).await;
    }
    ip.stop();
    Tracked {
        future: Pin::new(&mut running),
        done: &done,
    }
    .await;
    network.withdraw();
    EXIT.end();
    core::future::pending().await
}

/// Answer the station's start; its queue holds the one answer.
fn answer(started: &WakingSpscSender<'static, Started, 1, StationNotify, NetworkNotify>, outcome: Started) {
    if started.try_send(outcome).is_err() {
        crate::fail(Failure::Storage);
    }
}

/// Once the station asks, close the load's socket and stop the IP service,
/// then report the network stopped. Without the request it never ends.
async fn stop_network(
    stopper: wifi::NetworkStopper<'static, crate::station::radio::Notifies>,
    network: &serve::Network,
    ip: &oer_embassy_net::ip::Control<'static>,
    done: &Cell<bool>,
) {
    stopper.requested().await;
    // The socket's queued datagrams hold lent receive buffers.
    network.stop_load().await;
    ip.stop();
    core::future::poll_fn(|_| {
        if done.get() {
            core::task::Poll::Ready(())
        } else {
            // The IP part's end comes in a later round of the same join,
            // which its own wake starts.
            core::task::Poll::Pending
        }
    })
    .await;
    stopper.stopped();
    core::future::pending::<()>().await;
}
