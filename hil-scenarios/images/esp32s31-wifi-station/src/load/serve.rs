//! The load server on the station's IP stack: one run at a time on one UDP
//! socket bound to the station's DHCP address.
//!
//! The server shares hart 1's network task with the IP stack. A run
//! receives and echoes from its start, sends within its window and reads
//! the CPU, the named parts and the counters at the window's ends (the
//! cache's cleared just before its start),
//! sampling the boot hart between them when it asks; after its drain it
//! closes the socket and keeps what it read until the next run starts.
//! Every answer goes to the boot hart's relay, which publishes it, as do
//! the sampler and the boot hart's counters ([`super::relay`]).

use core::cell::Cell;
use core::convert::Infallible;
use core::net::Ipv4Addr;
use core::pin::pin;
use core::sync::atomic::Ordering;
use core::task::{Poll, Waker};

use embassy_futures::{
    select::{Either, Either3, select, select3},
    yield_now,
};
use embassy_net::TryError;
use embassy_net::udp::UdpSocket;
use embassy_net::wire::{ListenSocketAddr, SocketAddr};
use oer_hil_protocol::RequestIdentity;
use oer_memory::{WakingSpscQueue, WakingSpscReceiver, WakingSpscSender};
use oer_services_load::header::Header;
use oer_services_load::tally::Tally;
use oer_services_load::{
    Collected, Counters, CountersCollected, Load, LoadError, PartsCollected, Report, Request,
    SamplesCollected, Sent, Start, Started, Transmit,
};
use oer_time::{CLOCK_EPOCH_OFFSET, Instant, Timer as _};
use static_cell::ConstStaticCell;

use super::action::{Action, Answer, Kept, MAX_PAYLOAD, Phase, action};
use super::pace::{GRANULARITY_US, MAX_BATCH, Next, Schedule};
use super::receive::{Datagram, Receiving, classify, echo};
use super::relay::{self, Client};
use super::{counters, cpu, parts};
use crate::fatal::Failure;
use crate::network::ip::{Device, Interface};
use crate::peers::{NetworkNotify, RelayNotify};
use crate::station::emit;

/// Each socket queues at most this many received datagrams (the stack's
/// `udp-rx-queue-count-16`): some 2.7 ms of 1400-byte datagrams at 65 Mbit/s.
const SOCKET_RX_QUEUE: usize = 16;
const _: () = {
    if xarxa::config::UDP_RX_QUEUE_COUNT != SOCKET_RX_QUEUE {
        panic!("the image selects the stack's socket receive queue");
    }
};

/// The run's tally remembers 8192 words, 524 288 sequence numbers in 64 KiB:
/// some 90 s of 1400-byte datagrams at 65 Mbit/s, past the longest window
/// with its delay and drain.
const TALLY_WORDS: usize = 8192;
/// The one tally, in place in a static: it is far larger than a value the
/// image may move, so each run clears it in place.
static TALLY: ConstStaticCell<Tally<TALLY_WORDS>> = ConstStaticCell::new(Tally::new());

type Queue = WakingSpscQueue<(RequestIdentity, Request), 1, NetworkNotify, RelayNotify>;
/// The console on the boot hart sends, the server on hart 1 receives. The
/// console never waits for room: a taken request raises the relay's signal
/// instead, which finds nothing to do.
static REQUESTS: ConstStaticCell<Queue> = ConstStaticCell::new(WakingSpscQueue::across_harts(
    &crate::peers::NETWORK,
    &crate::peers::RELAY,
));
/// The console's end of the load requests.
pub(crate) type Sender =
    WakingSpscSender<'static, (RequestIdentity, Request), 1, NetworkNotify, RelayNotify>;
/// The load server's end.
pub(crate) type Receiver =
    WakingSpscReceiver<'static, (RequestIdentity, Request), 1, NetworkNotify, RelayNotify>;

/// The load request queue's two ends, split once.
pub(crate) fn requests() -> (Sender, Receiver) {
    REQUESTS
        .try_take()
        .unwrap_or_else(|| crate::fail(Failure::Storage))
        .split()
}

/// Hand `request` to the load server, or refuse it while one waits.
pub(crate) fn queue(requests: &Sender, identity: RequestIdentity, request: Request) {
    if requests.try_send((identity, request)).is_err() {
        use oer_hil_protocol::base::{RejectReason, Rejected};
        emit(identity, &Rejected(RejectReason::Busy));
    }
}

/// The station's IP interface while its stack runs: hart 1's network task
/// lends it once the stack started and withdraws it when the stack stopped.
/// A stop of the station first closes the load's socket through it: the
/// socket's queued datagrams hold the station's lent receive buffers.
pub(crate) struct Network {
    interface: Cell<Option<Interface>>,
    /// The station stops: no run opens a socket, a running one closes it.
    stopping: Cell<bool>,
    /// A run's socket is open.
    open: Cell<bool>,
    /// The run waiting for the stop, and the stop waiting for the run; both
    /// in hart 1's network task.
    run_waker: Cell<Option<Waker>>,
    stop_waker: Cell<Option<Waker>>,
}

impl Network {
    pub(crate) const fn new() -> Self {
        Self {
            interface: Cell::new(None),
            stopping: Cell::new(false),
            open: Cell::new(false),
            run_waker: Cell::new(None),
            stop_waker: Cell::new(None),
        }
    }

    pub(crate) fn lend(&self, interface: Interface) {
        self.interface.set(Some(interface));
    }

    pub(crate) fn withdraw(&self) {
        self.interface.set(None);
    }

    /// Close the load's socket and open no other: ready once none is open.
    pub(crate) async fn stop_load(&self) {
        self.stopping.set(true);
        if let Some(waker) = self.run_waker.take() {
            waker.wake();
        }
        core::future::poll_fn(|cx| {
            if self.open.get() {
                self.stop_waker.set(Some(cx.waker().clone()));
                Poll::Pending
            } else {
                Poll::Ready(())
            }
        })
        .await
    }

    /// Ready once the station stops.
    async fn stopping(&self) {
        core::future::poll_fn(|cx| {
            if self.stopping.get() {
                Poll::Ready(())
            } else {
                self.run_waker.set(Some(cx.waker().clone()));
                Poll::Pending
            }
        })
        .await
    }

    /// The run's socket was closed.
    fn closed(&self) {
        self.open.set(false);
        if let Some(waker) = self.stop_waker.take() {
            waker.wake();
        }
    }
}

type Socket = UdpSocket<'static, Device>;

/// Serve the load requests until the chip resets, publishing, sampling and
/// reading the boot hart through `hart0`.
pub(crate) async fn serve(mut requests: Receiver, network: &Network, hart0: &Client) -> Infallible {
    let tally = TALLY
        .try_take()
        .unwrap_or_else(|| crate::fail(Failure::Storage));
    let mut kept: Option<(u32, Ended)> = None;
    loop {
        let (identity, request) = next(&mut requests).await;
        let phase = Phase::Idle {
            kept: kept.as_ref().map(|(run, _)| *run),
        };
        match action(phase, request) {
            Action::Start(start) => {
                match sampled(&start, hart0)
                    .await
                    .and_then(|()| open(network, &start))
                {
                    Ok(socket) => {
                        let running = relay::Answer::Started(Started::Running);
                        hart0.publish(identity, running).await;
                        let ended = run(&mut requests, socket, start, tally, network, hart0).await;
                        network.closed();
                        // A run the station's stop ended keeps no report.
                        kept = ended.map(|ended| (start.run, ended));
                    }
                    Err(error) => {
                        let refused = relay::Answer::Started(Started::Refused(error));
                        hart0.publish(identity, refused).await;
                    }
                }
            }
            Action::Answer(answer) => publish(hart0, identity, answer).await,
            Action::Cpu => {
                let load = relay::Answer::Cpu(cpu_load().await);
                hart0.publish(identity, load).await;
            }
            Action::Kept(kind) => match (&kept, kind) {
                (Some((_, ended)), Kept::Report) => {
                    let report = Collected::Report(ended.report.clone());
                    hart0
                        .publish(identity, relay::Answer::Collected(report))
                        .await;
                }
                (Some((_, ended)), Kept::Parts { from }) => {
                    let parts = PartsCollected::Report(parts::page(&ended.parts, from));
                    hart0.publish(identity, relay::Answer::Parts(parts)).await;
                }
                (Some((_, ended)), Kept::Counters) => {
                    let counters = CountersCollected::Report(ended.counters);
                    hart0
                        .publish(identity, relay::Answer::Counters(counters))
                        .await;
                }
                (Some((_, ended)), Kept::Samples { from }) => match ended.sampled {
                    // The samples are in the boot hart's buffer: its relay
                    // publishes the page.
                    Some(Ok(())) => hart0.page(identity, from).await,
                    Some(Err(error)) => {
                        let refused = SamplesCollected::Refused(error);
                        hart0
                            .publish(identity, relay::Answer::Samples(refused))
                            .await;
                    }
                    None => {
                        let refused = SamplesCollected::Refused(LoadError::NotSampled);
                        hart0
                            .publish(identity, relay::Answer::Samples(refused))
                            .await;
                    }
                },
                (None, kind) => publish(hart0, identity, Answer::UnknownRun(kind)).await,
            },
        }
    }
}

/// Whether the image can sample `start`: a run that asks for samples
/// needs the sampler disarmed.
async fn sampled(start: &Start, hart0: &Client) -> Result<(), LoadError> {
    match start.profile {
        Some(_) => hart0.available().await,
        None => Ok(()),
    }
}

/// The next request, waiting for one.
async fn next(requests: &mut Receiver) -> (RequestIdentity, Request) {
    loop {
        requests.ready().await;
        if let Some(request) = requests.try_receive() {
            return request;
        }
    }
}

/// Publish what the server answers by itself.
async fn publish(hart0: &Client, identity: RequestIdentity, answer: Answer) {
    use relay::Answer as Published;
    let unknown = LoadError::UnknownRun;
    let published = match answer {
        Answer::Started(started) => Published::Started(started),
        Answer::Running(Kept::Report) => Published::Collected(Collected::Running),
        Answer::Running(Kept::Parts { .. }) => Published::Parts(PartsCollected::Running),
        Answer::Running(Kept::Counters) => Published::Counters(CountersCollected::Running),
        Answer::Running(Kept::Samples { .. }) => Published::Samples(SamplesCollected::Running),
        Answer::UnknownRun(Kept::Report) => Published::Collected(Collected::Refused(unknown)),
        Answer::UnknownRun(Kept::Parts { .. }) => {
            Published::Parts(PartsCollected::Refused(unknown))
        }
        Answer::UnknownRun(Kept::Counters) => {
            Published::Counters(CountersCollected::Refused(unknown))
        }
        Answer::UnknownRun(Kept::Samples { .. }) => {
            Published::Samples(SamplesCollected::Refused(unknown))
        }
    };
    hart0.publish(identity, published).await;
}

/// The run's socket, bound to `start.port` of the station's DHCP address.
fn open(network: &Network, start: &Start) -> Result<Socket, LoadError> {
    if network.stopping.get() {
        return Err(LoadError::NoAddress);
    }
    let interface = network.interface.get().ok_or(LoadError::NoAddress)?;
    let lease = interface.dhcpv4_lease().ok_or(LoadError::NoAddress)?;
    let mut socket = UdpSocket::new(interface.stack()).map_err(|_| LoadError::Bind)?;
    let local = SocketAddr::from((lease.address.address(), start.port));
    socket
        .bind(local, ListenSocketAddr::UNSPECIFIED)
        .map_err(|_| LoadError::Bind)?;
    network.open.set(true);
    Ok(socket)
}

/// The measuring window's ends as the image kept them.
struct Window {
    start_us: Cell<Option<u64>>,
    end_us: Cell<Option<u64>>,
}

impl Window {
    /// Whether `now_us` lies within the window: after its start, before its
    /// end.
    fn contains(&self, now_us: u64) -> bool {
        self.start_us.get().is_some_and(|start| now_us >= start)
            && self.end_us.get().is_none_or(|end| now_us < end)
    }
}

/// What the window's two ends read.
struct Ends {
    start_us: u64,
    end_us: u64,
    cpu_start: Load,
    cpu_end: Load,
    parts: parts::Windows,
    counters: Counters,
    sampled: Option<Result<(), LoadError>>,
}

/// What an ended run keeps until the next starts; its samples stay in the
/// sampler. `sampled` is `None` for a run that asked for none.
struct Ended {
    report: Report,
    parts: parts::Windows,
    counters: Counters,
    sampled: Option<Result<(), LoadError>>,
}

/// Run `start` on `socket` beside the requests until its drain ended, with
/// `tally` cleared for it; what it keeps.
async fn run(
    requests: &mut Receiver,
    mut socket: Socket,
    start: Start,
    tally: &mut Tally<TALLY_WORDS>,
    network: &Network,
    hart0: &Client,
) -> Option<Ended> {
    tally.clear();
    let window = Window {
        start_us: Cell::new(None),
        end_us: Cell::new(None),
    };
    let mut receiving = Receiving::default();
    let mut sent = Sent::default();
    let ends = {
        let mut activity = pin!(select(
            receive(&socket, start.run, tally, &mut receiving, &window),
            measure(&socket, &start, &window, &mut sent, network, hart0),
        ));
        loop {
            let running = Phase::Running { run: start.run };
            match select3(next(requests), activity.as_mut(), network.stopping()).await {
                Either3::First((identity, request)) => match action(running, request) {
                    Action::Answer(answer) => publish(hart0, identity, answer).await,
                    Action::Cpu => {
                        let load = relay::Answer::Cpu(cpu_load().await);
                        hart0.publish(identity, load).await;
                    }
                    // A running server starts nothing and keeps no report.
                    Action::Start(_) => {
                        let busy = relay::Answer::Started(Started::Refused(LoadError::Busy));
                        hart0.publish(identity, busy).await;
                    }
                    Action::Kept(kind) => {
                        publish(hart0, identity, Answer::Running(kind)).await;
                    }
                },
                Either3::Second(Either::First(never)) => match never {},
                Either3::Second(Either::Second(ends)) => break Some(ends),
                Either3::Third(()) => break None,
            }
        }
    };
    socket.close();
    let Some(ends) = ends else {
        // The station stops: its received datagrams go with the socket. A
        // session the run started stops too; one the handler kept makes
        // the next sampled run refuse. A call the window had begun leaves
        // its reply, which these calls discard.
        hart0.request_stop().await;
        let _ = hart0.stop().await;
        return None;
    };
    let report = Report {
        window_start_us: ends.start_us,
        window_end_us: ends.end_us,
        cpu_start: ends.cpu_start,
        cpu_end: ends.cpu_end,
        received: receiving.received(tally),
        sent,
    };
    Some(Ended {
        report,
        parts: ends.parts,
        counters: ends.counters,
        sampled: ends.sampled,
    })
}

/// Count every datagram on the port and echo those that ask for it, at once.
async fn receive(
    socket: &Socket,
    run: u32,
    tally: &mut Tally<TALLY_WORDS>,
    counts: &mut Receiving,
    window: &Window,
) -> Infallible {
    loop {
        socket.wait_recv_ready().await;
        loop {
            let mut packet = match socket.try_recv() {
                Ok(packet) => packet,
                Err(TryError::WouldBlock) => break,
                // Only a closed socket refuses: it receives nothing more.
                Err(TryError::Other(_)) => return core::future::pending().await,
            };
            let now = now_us();
            match classify(packet.payload(), run) {
                Datagram::Echo(header) => {
                    let taken = echo(header).write(packet.payload_mut()).is_some()
                        && socket
                            .try_send_to(packet.payload(), packet.meta().remote_addr)
                            .is_ok();
                    counts.echo(taken);
                }
                Datagram::Load(header) => counts.load(
                    tally,
                    header.sequence,
                    packet.payload().len(),
                    now,
                    window.contains(now),
                ),
                Datagram::Foreign => counts.foreign(),
            }
        }
    }
}

/// The run's window and drain: open the window `delay_ms` after now, read
/// the CPU and the parts, clear the cache's counters and read the counters
/// (both on the boot hart),
/// sample when asked, send within it, close it after `duration_ms`, read
/// them again, stop sampling, then wait out the drain.
async fn measure(
    socket: &Socket,
    start: &Start,
    window: &Window,
    sent: &mut Sent,
    network: &Network,
    hart0: &Client,
) -> Ends {
    let planned_start = now_us().saturating_add(millis(start.delay_ms));
    let planned_end = planned_start.saturating_add(millis(start.duration_ms));
    wait_until(planned_start).await;
    let cpu_start = cpu_load().await;
    let parts_start = parts_sample(true).await;
    let counters_start = counters::snapshot(hart0.counters(true).await, stack_received(network));
    let start_us = now_us();
    window.start_us.set(Some(start_us));
    let sampling = match start.profile {
        Some(profile) => Some(hart0.start(profile.period_us).await),
        None => None,
    };
    if let Some(transmit) = start.transmit {
        send(socket, start.run, transmit, start_us, planned_end, sent).await;
    }
    wait_until(planned_end).await;
    let end_us = now_us();
    window.end_us.set(Some(end_us));
    hart0.request_stop().await;
    let cpu_end = cpu_load().await;
    let parts_end = parts_sample(false).await;
    let counters_end = counters::snapshot(hart0.counters(false).await, stack_received(network));
    let sampled = match sampling {
        Some(Ok(())) => Some(hart0.stop().await),
        other => other,
    };
    wait_until(end_us.saturating_add(millis(start.drain_ms))).await;
    Ends {
        start_us,
        end_us,
        cpu_start,
        cpu_end,
        parts: parts::window(&parts_start, &parts_end),
        counters: counters::window(&counters_start, &counters_end),
        sampled,
    }
}

/// The frames the IP stack took from its device so far; none before it
/// has an interface.
fn stack_received(network: &Network) -> u32 {
    network
        .interface
        .get()
        .map_or(0, |interface| interface.stack().statistics().rx_frames)
}

fn millis(ms: u32) -> u64 {
    u64::from(ms).saturating_mul(1000)
}

/// Send `transmit`'s datagrams from `begin` until `end`: paced to its rate,
/// or as fast as the socket takes them.
async fn send(
    socket: &Socket,
    run: u32,
    transmit: Transmit,
    begin: u64,
    end: u64,
    sent: &mut Sent,
) {
    let peer = SocketAddr::from((Ipv4Addr::from(transmit.peer), transmit.peer_port));
    let mut buffer = [0; MAX_PAYLOAD as usize];
    // The start's payload was validated against the buffer.
    let Some(datagram) = buffer.get_mut(..usize::from(transmit.payload)) else {
        return;
    };
    match Schedule::new(begin, end, transmit.payload, transmit.rate_bps) {
        Some(mut schedule) => loop {
            let next = schedule.batch(now_us(), || {
                let now = now_us();
                match write(datagram, run, sent.datagrams, now) {
                    Some(datagram) => match socket.try_send_to(datagram, peer) {
                        Ok(()) => taken(sent, datagram.len(), now),
                        Err(TryError::WouldBlock) => {
                            sent.refused = sent.refused.saturating_add(1);
                        }
                        Err(TryError::Other(_)) => sent.failed = sent.failed.saturating_add(1),
                    },
                    None => sent.failed = sent.failed.saturating_add(1),
                }
            });
            match next {
                Next::Yield => yield_now().await,
                Next::Wait(at) => wait_until(at).await,
                Next::Done => {
                    sent.missed = schedule.missed();
                    return;
                }
            }
        },
        None => unpaced(socket, run, datagram, peer, end, sent).await,
    }
}

/// Send as fast as the socket takes the datagrams until `end`, yielding to
/// the executor after each batch; after a failed send, wait the pacing
/// granularity.
async fn unpaced(
    socket: &Socket,
    run: u32,
    datagram: &mut [u8],
    peer: SocketAddr,
    end: u64,
    sent: &mut Sent,
) {
    let mut batch = 0;
    loop {
        let now = now_us();
        if now >= end {
            return;
        }
        let Some(datagram) = write(datagram, run, sent.datagrams, now) else {
            sent.failed = sent.failed.saturating_add(1);
            return;
        };
        match select(socket.send_to(datagram, peer), wait_until(end)).await {
            Either::First(Ok(())) => taken(sent, datagram.len(), now),
            Either::First(Err(_)) => {
                sent.failed = sent.failed.saturating_add(1);
                wait_until(now.saturating_add(GRANULARITY_US).min(end)).await;
            }
            Either::Second(()) => return,
        }
        batch += 1;
        if batch == MAX_BATCH {
            batch = 0;
            yield_now().await;
        }
    }
}

/// `datagram` with the header of sequence `sequence` sent at `now`.
fn write(datagram: &mut [u8], run: u32, sequence: u64, now: u64) -> Option<&[u8]> {
    Header {
        run,
        sequence,
        sent_us: now,
        echo_request: false,
        echo_reply: false,
    }
    .write(datagram)?;
    Some(datagram)
}

/// The socket took a datagram of `bytes` at `now`.
fn taken(sent: &mut Sent, bytes: usize, now: u64) {
    sent.datagrams = sent.datagrams.saturating_add(1);
    sent.bytes = sent
        .bytes
        .saturating_add(u64::try_from(bytes).unwrap_or(u64::MAX));
    sent.first_us.get_or_insert(now);
    sent.last_us = Some(now);
}

/// The CPU report now. A slot its executor is writing is sampled again
/// after yielding.
async fn cpu_load() -> Load {
    let mut samples = [cpu::Sample::default(); cpu::SLOTS];
    for (sample, counter) in samples.iter_mut().zip(&cpu::COUNTERS) {
        *sample = loop {
            match counter.sample(activity_now()) {
                Ok(sample) => break sample,
                Err(cpu::Concurrent) => yield_now().await,
            }
        };
    }
    cpu::load(now_us(), &samples)
}

/// Every part's sample now, in the current epoch of the longest polls;
/// with `begin`, a new epoch starts after it. A part its executor is
/// writing is sampled again after yielding.
async fn parts_sample(begin: bool) -> [parts::Sample; parts::COUNT] {
    let epoch = parts::EPOCH.load(Ordering::Relaxed);
    let mut samples = [parts::Sample::default(); parts::COUNT];
    for (sample, counter) in samples.iter_mut().zip(&parts::COUNTERS) {
        *sample = loop {
            match counter.sample(activity_now(), epoch) {
                Ok(sample) => break sample,
                Err(parts::Concurrent) => yield_now().await,
            }
        };
    }
    if begin {
        parts::EPOCH.store(epoch.wrapping_add(1), Ordering::Relaxed);
    }
    samples
}

/// The image's monotonic microseconds.
fn now_us() -> u64 {
    oer_embassy_time::now_micros()
}

/// The executors' activity clock: the time driver's microseconds, which
/// the image's clock leads by [`CLOCK_EPOCH_OFFSET`], wrapping at 2^32.
pub(crate) fn activity_now() -> u32 {
    now_us().wrapping_sub(CLOCK_EPOCH_OFFSET.as_micros()) as u32
}

async fn wait_until(at_us: u64) {
    oer_embassy_time::EmbassyClock
        .wait_until(Instant::from_micros(at_us))
        .await;
}
