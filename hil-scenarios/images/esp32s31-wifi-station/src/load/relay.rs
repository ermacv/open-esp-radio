//! The load server on hart 1 and what it needs from the boot hart: the
//! console, which publishes its answers, the PC sampler, which samples the
//! boot hart on its TIMG1, and the boot hart's cycle and instruction
//! counters.
//!
//! The server's [`Client`] sends each [`Command`] through one queue; the
//! boot hart's relay task serves them in order and answers the ones that
//! ask through two reply queues. Every queue runs across harts: an end
//! waits on its own task's executor signal, which the other hart raises
//! (`crate::peers`). A call numbers its command and takes only its own
//! reply, so a call the server abandoned leaves a reply the next call
//! discards.

use core::cell::Cell;
use core::future::poll_fn;
use core::task::Poll;

use oer_hil_protocol::RequestIdentity;
use oer_memory::{
    AffineSpscTrySendError, QueueNotify as _, WakingSpscQueue, WakingSpscReceiver, WakingSpscSender,
};
use oer_services_load::{
    Collected, CountersCollected, LinkCollected, Load, LoadError, PartsCollected, SamplesCollected,
    Started, TrackingCollected,
};
use static_cell::ConstStaticCell;

use super::counters::{Cache, Hart};
use super::parts::{Part, Polled};
use super::sampler::Profiler;
use crate::fatal::Failure;
use crate::peers::{self, NetworkNotify, RelayNotify};
use crate::station::emit;

/// An answer the load server publishes.
pub(crate) enum Answer {
    Started(Started),
    Collected(Collected),
    Parts(PartsCollected),
    Counters(CountersCollected),
    Link(LinkCollected),
    Tracking(TrackingCollected),
    Samples(SamplesCollected),
    Cpu(Load),
}

impl Answer {
    fn emit(self, identity: RequestIdentity) {
        match self {
            Self::Started(started) => emit(identity, &started),
            Self::Collected(collected) => emit(identity, &collected),
            Self::Parts(parts) => emit(identity, &parts),
            Self::Counters(counters) => emit(identity, &counters),
            Self::Link(link) => emit(identity, &link),
            Self::Tracking(tracking) => emit(identity, &tracking),
            Self::Samples(samples) => emit(identity, &samples),
            Self::Cpu(load) => emit(identity, &load),
        }
    }
}

/// What the load server asks the boot hart.
#[allow(
    clippy::large_enum_variant,
    reason = "commands live in place in their static queue; the image has no allocator"
)]
pub(crate) enum Command {
    /// Publish this answer.
    Publish(RequestIdentity, Answer),
    /// Publish the last session's samples from the one numbered `from`.
    Page(RequestIdentity, u16),
    /// Whether a sampled run can start.
    Available(u32),
    /// Start sampling every `period_us`.
    Start { call: u32, period_us: u32 },
    /// Ask the running session to stop at its next tick.
    RequestStop,
    /// Stop the session and take it back.
    Stop(u32),
    /// The boot hart's and the cache's counters now; with `start`, the
    /// cache's cleared and running first.
    Counters { call: u32, start: bool },
}

/// The answer to call `call` of the sampler.
struct Sampled {
    call: u32,
    result: Result<(), LoadError>,
}

/// The answer to call `call` of the counters.
struct Counted {
    call: u32,
    hart: Hart,
}

/// Answers the server sends while its relay serves an earlier command.
const COMMANDS: usize = 4;

type CommandQueue = WakingSpscQueue<Command, COMMANDS, RelayNotify, NetworkNotify>;
type SampledQueue = WakingSpscQueue<Sampled, 1, NetworkNotify, RelayNotify>;
type CountedQueue = WakingSpscQueue<Counted, 1, NetworkNotify, RelayNotify>;
static QUEUE: ConstStaticCell<CommandQueue> = ConstStaticCell::new(WakingSpscQueue::across_harts(
    &peers::RELAY,
    &peers::NETWORK,
));
static SAMPLED: ConstStaticCell<SampledQueue> = ConstStaticCell::new(
    WakingSpscQueue::across_harts(&peers::NETWORK, &peers::RELAY),
);
static COUNTED: ConstStaticCell<CountedQueue> = ConstStaticCell::new(
    WakingSpscQueue::across_harts(&peers::NETWORK, &peers::RELAY),
);

/// The relay's ends, split once: the boot hart's and hart 1's.
pub(crate) fn split(profiler: Profiler, cache: Cache) -> (Relay, Client) {
    let (Some(commands), Some(sampled), Some(counted)) =
        (QUEUE.try_take(), SAMPLED.try_take(), COUNTED.try_take())
    else {
        crate::fail(Failure::Storage);
    };
    let (send, receive) = commands.split();
    let (sampled_send, sampled_receive) = sampled.split();
    let (counted_send, counted_receive) = counted.split();
    (
        Relay {
            commands: receive,
            sampled: sampled_send,
            counted: counted_send,
            profiler,
            cache,
        },
        Client {
            commands: send,
            sampled: sampled_receive,
            counted: counted_receive,
            calls: Cell::new(0),
        },
    )
}

/// The boot hart's end: the relay task's.
pub(crate) struct Relay {
    commands: WakingSpscReceiver<'static, Command, COMMANDS, RelayNotify, NetworkNotify>,
    sampled: WakingSpscSender<'static, Sampled, 1, NetworkNotify, RelayNotify>,
    counted: WakingSpscSender<'static, Counted, 1, NetworkNotify, RelayNotify>,
    profiler: Profiler,
    cache: Cache,
}

/// The relay task: serve the load server's commands until the chip resets.
pub(crate) async fn run(relay: Relay) {
    let serve = core::pin::pin!(serve(relay));
    match (Polled {
        part: Part::Relay,
        future: serve,
    })
    .await {}
}

async fn serve(relay: Relay) -> core::convert::Infallible {
    let Relay {
        mut commands,
        mut sampled,
        mut counted,
        mut profiler,
        mut cache,
    } = relay;
    loop {
        commands.ready().await;
        let Some(command) = commands.try_receive() else {
            continue;
        };
        match command {
            Command::Publish(identity, answer) => answer.emit(identity),
            Command::Page(identity, from) => emit(
                identity,
                &profiler.page(from).map_or(
                    SamplesCollected::Refused(LoadError::NoSampler),
                    SamplesCollected::Page,
                ),
            ),
            Command::Available(call) => {
                let result = if profiler.available() {
                    Ok(())
                } else {
                    Err(LoadError::NoSampler)
                };
                sampled.ready().await.send(Sampled { call, result });
            }
            Command::Start { call, period_us } => {
                let result = profiler.start(period_us);
                sampled.ready().await.send(Sampled { call, result });
            }
            Command::RequestStop => profiler.request_stop(),
            Command::Stop(call) => {
                let result = profiler.stop().await;
                sampled.ready().await.send(Sampled { call, result });
            }
            Command::Counters { call, start } => {
                let hart = Hart::now(&mut cache, start);
                counted.ready().await.send(Counted { call, hart });
            }
        }
    }
}

/// Hart 1's end: the load server's, in hart 1's network task.
pub(crate) struct Client {
    commands: WakingSpscSender<'static, Command, COMMANDS, RelayNotify, NetworkNotify>,
    sampled: WakingSpscReceiver<'static, Sampled, 1, NetworkNotify, RelayNotify>,
    counted: WakingSpscReceiver<'static, Counted, 1, NetworkNotify, RelayNotify>,
    /// The number of the last call.
    calls: Cell<u32>,
}

impl Client {
    /// Publish `answer` to the request `identity`.
    pub(crate) async fn publish(&self, identity: RequestIdentity, answer: Answer) {
        self.send(Command::Publish(identity, answer)).await;
    }

    /// Publish the last session's samples from the one numbered `from`.
    pub(crate) async fn page(&self, identity: RequestIdentity, from: u16) {
        self.send(Command::Page(identity, from)).await;
    }

    /// Whether a sampled run can start.
    ///
    /// # Errors
    /// [`LoadError::NoSampler`] when it cannot.
    pub(crate) async fn available(&self) -> Result<(), LoadError> {
        let call = self.call();
        self.send(Command::Available(call)).await;
        self.sampled(call).await
    }

    /// Forget the last session's samples and start sampling every
    /// `period_us`.
    ///
    /// # Errors
    /// [`LoadError::NoSampler`] when the sampler cannot start.
    pub(crate) async fn start(&self, period_us: u32) -> Result<(), LoadError> {
        let call = self.call();
        self.send(Command::Start { call, period_us }).await;
        self.sampled(call).await
    }

    /// Ask the running session to stop at its next tick.
    pub(crate) async fn request_stop(&self) {
        self.send(Command::RequestStop).await;
    }

    /// Stop the running session and take it back.
    ///
    /// # Errors
    /// [`LoadError::NoSampler`] when the sampler's handler kept it.
    pub(crate) async fn stop(&self) -> Result<(), LoadError> {
        let call = self.call();
        self.send(Command::Stop(call)).await;
        self.sampled(call).await
    }

    /// The boot hart's and the cache's counters, read there now; with
    /// `start`, the cache's cleared and running first.
    pub(crate) async fn counters(&self, start: bool) -> Hart {
        let call = self.call();
        self.send(Command::Counters { call, start }).await;
        poll_fn(|cx| {
            peers::NETWORK.register(cx);
            while let Some(counted) = self.counted.try_receive() {
                if counted.call == call {
                    return Poll::Ready(counted.hart);
                }
            }
            Poll::Pending
        })
        .await
    }

    fn call(&self) -> u32 {
        let call = self.calls.get().wrapping_add(1);
        self.calls.set(call);
        call
    }

    /// The reply to sampler call `call`; earlier calls' replies are
    /// discarded.
    async fn sampled(&self, call: u32) -> Result<(), LoadError> {
        poll_fn(|cx| {
            peers::NETWORK.register(cx);
            while let Some(sampled) = self.sampled.try_receive() {
                if sampled.call == call {
                    return Poll::Ready(sampled.result);
                }
            }
            Poll::Pending
        })
        .await
    }

    /// Queue `command`, waiting while the relay serves earlier ones.
    async fn send(&self, mut command: Command) {
        loop {
            match self.commands.try_send(command) {
                Ok(()) => return,
                Err(AffineSpscTrySendError(back)) => command = back,
            }
            poll_fn(|cx| {
                peers::NETWORK.register(cx);
                if self.commands.free_capacity() == 0 {
                    Poll::Pending
                } else {
                    Poll::Ready(())
                }
            })
            .await;
        }
    }
}
