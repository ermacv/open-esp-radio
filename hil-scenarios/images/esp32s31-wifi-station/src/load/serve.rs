//! The load server on the station's IP stack: one run at a time on one UDP
//! socket bound to the station's DHCP address.
//!
//! The server shares the station work's task (P1). A run receives and
//! echoes from its start, sends within its window and reads the CPU at the
//! window's ends; after its drain it closes the socket and keeps its report
//! until the next run starts. Every answer is published without waiting.

use core::cell::Cell;
use core::convert::Infallible;
use core::net::Ipv4Addr;
use core::pin::pin;

use embassy_futures::{
    select::{Either, select},
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
    Collected, Load, LoadError, Report, Request, Sent, Start, Started, Transmit,
};
use oer_time::{CLOCK_EPOCH_OFFSET, Instant, Timer as _};
use static_cell::ConstStaticCell;

use super::action::{Action, Answer, MAX_PAYLOAD, Phase, action};
use super::cpu;
use super::pace::{GRANULARITY_US, MAX_BATCH, Next, Schedule};
use super::receive::{Datagram, Receiving, classify, echo};
use crate::fatal::Failure;
use crate::station::emit;
use crate::station::ip::{Device, Interface};

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

type Queue = WakingSpscQueue<(RequestIdentity, Request), 1>;
static REQUESTS: ConstStaticCell<Queue> = ConstStaticCell::new(WakingSpscQueue::new());
/// The console's end of the load requests.
pub(crate) type Sender = WakingSpscSender<'static, (RequestIdentity, Request), 1>;
/// The load server's end.
pub(crate) type Receiver = WakingSpscReceiver<'static, (RequestIdentity, Request), 1>;

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

/// The station's IP interface while its stack runs: the station work lends
/// it once the stack started and withdraws it when the stack stopped.
pub(crate) struct Network(Cell<Option<Interface>>);

impl Network {
    pub(crate) const fn new() -> Self {
        Self(Cell::new(None))
    }

    pub(crate) fn lend(&self, interface: Interface) {
        self.0.set(Some(interface));
    }

    pub(crate) fn withdraw(&self) {
        self.0.set(None);
    }
}

type Socket = UdpSocket<'static, Device>;

/// Serve the load requests until the chip resets.
pub(crate) async fn serve(mut requests: Receiver, network: &Network) -> Infallible {
    let tally = TALLY
        .try_take()
        .unwrap_or_else(|| crate::fail(Failure::Storage));
    let mut kept: Option<(u32, Report)> = None;
    loop {
        let (identity, request) = next(&mut requests).await;
        let phase = Phase::Idle {
            kept: kept.as_ref().map(|(run, _)| *run),
        };
        match action(phase, request) {
            Action::Start(start) => match open(network, &start) {
                Ok(socket) => {
                    emit(identity, &Started::Running);
                    let report = run(&mut requests, socket, start, tally).await;
                    kept = Some((start.run, report));
                }
                Err(error) => emit(identity, &Started::Refused(error)),
            },
            Action::Answer(answer) => publish(identity, answer),
            Action::Cpu => emit(identity, &cpu_load().await),
            Action::Report => match &kept {
                Some((_, report)) => emit(identity, &Collected::Report(report.clone())),
                None => emit(identity, &Collected::Refused(LoadError::UnknownRun)),
            },
        }
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

fn publish(identity: RequestIdentity, answer: Answer) {
    match answer {
        Answer::Started(started) => emit(identity, &started),
        Answer::Running => emit(identity, &Collected::Running),
        Answer::UnknownRun => emit(identity, &Collected::Refused(LoadError::UnknownRun)),
    }
}

/// The run's socket, bound to `start.port` of the station's DHCP address.
fn open(network: &Network, start: &Start) -> Result<Socket, LoadError> {
    let interface = network.0.get().ok_or(LoadError::NoAddress)?;
    let lease = interface.dhcpv4_lease().ok_or(LoadError::NoAddress)?;
    let mut socket = UdpSocket::new(interface.stack()).map_err(|_| LoadError::Bind)?;
    let local = SocketAddr::from((lease.address.address(), start.port));
    socket
        .bind(local, ListenSocketAddr::UNSPECIFIED)
        .map_err(|_| LoadError::Bind)?;
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
}

/// Run `start` on `socket` beside the requests until its drain ended, with
/// `tally` cleared for it; its report.
async fn run(
    requests: &mut Receiver,
    mut socket: Socket,
    start: Start,
    tally: &mut Tally<TALLY_WORDS>,
) -> Report {
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
            measure(&socket, &start, &window, &mut sent),
        ));
        loop {
            let running = Phase::Running { run: start.run };
            match select(next(requests), activity.as_mut()).await {
                Either::First((identity, request)) => match action(running, request) {
                    Action::Answer(answer) => publish(identity, answer),
                    Action::Cpu => emit(identity, &cpu_load().await),
                    // A running server starts nothing and keeps no report.
                    Action::Start(_) => emit(identity, &Started::Refused(LoadError::Busy)),
                    Action::Report => emit(identity, &Collected::Running),
                },
                Either::Second(Either::First(never)) => match never {},
                Either::Second(Either::Second(ends)) => break ends,
            }
        }
    };
    socket.close();
    Report {
        window_start_us: ends.start_us,
        window_end_us: ends.end_us,
        cpu_start: ends.cpu_start,
        cpu_end: ends.cpu_end,
        received: receiving.received(tally),
        sent,
    }
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
/// the CPU, send within it, close it after `duration_ms`, read the CPU
/// again, then wait out the drain.
async fn measure(socket: &Socket, start: &Start, window: &Window, sent: &mut Sent) -> Ends {
    let planned_start = now_us().saturating_add(millis(start.delay_ms));
    let planned_end = planned_start.saturating_add(millis(start.duration_ms));
    wait_until(planned_start).await;
    let cpu_start = cpu_load().await;
    let start_us = now_us();
    window.start_us.set(Some(start_us));
    if let Some(transmit) = start.transmit {
        send(socket, start.run, transmit, start_us, planned_end, sent).await;
    }
    wait_until(planned_end).await;
    let end_us = now_us();
    window.end_us.set(Some(end_us));
    let cpu_end = cpu_load().await;
    wait_until(end_us.saturating_add(millis(start.drain_ms))).await;
    Ends {
        start_us,
        end_us,
        cpu_start,
        cpu_end,
    }
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

/// The image's monotonic microseconds.
fn now_us() -> u64 {
    oer_embassy_time::now_micros()
}

/// The executors' activity clock: the time driver's microseconds, which
/// the image's clock leads by [`CLOCK_EPOCH_OFFSET`], wrapping at 2^32.
fn activity_now() -> u32 {
    now_us().wrapping_sub(CLOCK_EPOCH_OFFSET.as_micros()) as u32
}

async fn wait_until(at_us: u64) {
    oer_embassy_time::EmbassyClock
        .wait_until(Instant::from_micros(at_us))
        .await;
}
