//! The load server in the second boot: one run at a time on one UDP
//! socket bound to the station's DHCP address, on hart 1 beside the IP
//! stack.
//!
//! A run receives and echoes from its start, sends within its window and
//! reads the CPU, the named parts, the counters, the link's rates and the
//! PHY tracking ticks at the window's ends (the cache's counters cleared
//! just before its start), sampling the boot hart between them when it
//! asks; after its drain it closes the socket and keeps what it read until
//! the next run starts. The boot hart's counters and sampler come through
//! its relay ([`super::relay`]); the answers go back to the worker, which
//! publishes them.

use core::cell::Cell;
use core::convert::Infallible;
use core::net::Ipv4Addr;
use core::pin::pin;

use embassy_futures::select::{Either, select};
use embassy_futures::yield_now;
use oer_espressif::observe;
use oer_espressif::observe::parts::{PARTS, PartReading};
use oer_espressif::observe::tracking::TrackingSample;
use oer_espressif::time::{Instant, Timer};
use oer_espressif::wifi::embassy_net::TryError;
use oer_espressif::wifi::embassy_net::udp::UdpSocket;
use oer_espressif::wifi::embassy_net::wire::{ListenSocketAddr, SocketAddr};
use oer_espressif::wifi::{IpDevice, Ipv4Status, Net};
use oer_espressif::{Channel, SecondEnd};
use oer_hil_protocol::RequestIdentity;
use oer_services_load::header::Header;
use oer_services_load::tally::Tally;
use oer_services_load::{
    Collected, Counters, CountersCollected, Link, LinkCollected, Load, LoadError, PartsCollected,
    Report, Request, SamplesCollected, Sent, Start, Started, Tracking, TrackingCollected, Transmit,
};
use static_cell::ConstStaticCell;

use super::action::{Action, Answer as Decided, Kept, MAX_PAYLOAD, Phase, action};
use super::pace::{GRANULARITY_US, MAX_BATCH, Next, Schedule};
use super::receive::{Datagram, Receiving, classify, echo};
use super::relay::{self, Client};
use super::report::{self, Snapshot, Windows};
use crate::{RX, TX};

/// The run's tally remembers 8192 words, 524 288 sequence numbers in 64 KiB:
/// some 90 s of 1400-byte datagrams at 65 Mbit/s, past the longest window
/// with its delay and drain.
const TALLY_WORDS: usize = 8192;
/// The one tally, in place in a static: it is far larger than a value the
/// image may move, so each run clears it in place.
pub(crate) static TALLY: ConstStaticCell<Tally<TALLY_WORDS>> = ConstStaticCell::new(Tally::new());
/// The tally the second boot takes once.
pub(crate) type RunTally = &'static mut Tally<TALLY_WORDS>;

/// The load requests the worker hands the server, each with its identity,
/// and the server's answers.
pub(crate) static LOAD: Channel<(RequestIdentity, Request), Answer, 1> = Channel::new();
/// The server's end, in the second boot.
pub(crate) type Requests = SecondEnd<(RequestIdentity, Request), Answer, 1>;

/// An answer of the load server, which the worker publishes.
pub(crate) enum Answer {
    Started(Started),
    Collected(Collected),
    Parts(PartsCollected),
    Counters(CountersCollected),
    Link(LinkCollected),
    Tracking(TrackingCollected),
    Samples(SamplesCollected),
    Cpu(Load),
    /// The boot hart's relay answers it: a page of samples.
    Relayed,
}

impl Answer {
    /// Publish it to the request `identity`.
    pub(crate) fn publish(self, identity: RequestIdentity) {
        use oer_hil_agent_espressif::answer;
        match self {
            Self::Started(started) => answer(identity, &started),
            Self::Collected(collected) => answer(identity, &collected),
            Self::Parts(parts) => answer(identity, &parts),
            Self::Counters(counters) => answer(identity, &counters),
            Self::Link(link) => answer(identity, &link),
            Self::Tracking(tracking) => answer(identity, &tracking),
            Self::Samples(samples) => answer(identity, &samples),
            Self::Cpu(load) => answer(identity, &load),
            Self::Relayed => {}
        }
    }
}

type Socket = UdpSocket<'static, IpDevice<RX, TX>>;

/// Serve the load requests until the chip resets, sampling and reading
/// the boot hart through `hart0`.
pub(crate) async fn serve(
    net: &Net<RX, TX>,
    mut requests: Requests,
    mut hart0: Client,
    tally: RunTally,
) -> Infallible {
    let mut kept: Option<(u32, Ended)> = None;
    loop {
        let (identity, request) = requests.receive().await;
        let phase = Phase::Idle {
            kept: kept.as_ref().map(|(run, _)| *run),
        };
        let answer = match action(phase, request) {
            Action::Start(start) => {
                match sampled(&start, &mut hart0)
                    .await
                    .and_then(|()| open(net, &start))
                {
                    Ok(socket) => {
                        requests.send(Answer::Started(Started::Running)).await;
                        let ended = run(&mut requests, socket, start, tally, net, &mut hart0).await;
                        kept = Some((start.run, ended));
                        continue;
                    }
                    Err(error) => Answer::Started(Started::Refused(error)),
                }
            }
            Action::Answer(decided) => decided_answer(decided),
            Action::Cpu => Answer::Cpu(cpu_load().await),
            Action::Kept(kind) => match &kept {
                Some((_, ended)) => kept_answer(ended, kind, identity, &mut hart0).await,
                None => decided_answer(Decided::UnknownRun(kind)),
            },
        };
        requests.send(answer).await;
    }
}

/// What an ended run kept, of `kind`.
async fn kept_answer(
    ended: &Ended,
    kind: Kept,
    identity: RequestIdentity,
    hart0: &mut Client,
) -> Answer {
    match kind {
        Kept::Report => Answer::Collected(Collected::Report(ended.report.clone())),
        Kept::Parts { from } => {
            Answer::Parts(PartsCollected::Report(report::page(&ended.parts, from)))
        }
        Kept::Counters => Answer::Counters(CountersCollected::Report(ended.counters)),
        Kept::Link => Answer::Link(LinkCollected::Report(ended.link)),
        Kept::Tracking => Answer::Tracking(TrackingCollected::Report(ended.tracking)),
        Kept::Samples { from } => match ended.sampled {
            // The samples are in the boot hart's buffer: its relay
            // answers with the page.
            Some(Ok(())) => {
                relay::page_of(hart0, identity, from).await;
                Answer::Relayed
            }
            Some(Err(error)) => Answer::Samples(SamplesCollected::Refused(error)),
            None => Answer::Samples(SamplesCollected::Refused(LoadError::NotSampled)),
        },
    }
}

/// Whether the image can sample `start`: a run that asks for samples
/// needs the sampler disarmed.
async fn sampled(start: &Start, hart0: &mut Client) -> Result<(), LoadError> {
    match start.profile {
        Some(_) => relay::sampler_available(hart0).await,
        None => Ok(()),
    }
}

/// What the server answers by itself.
fn decided_answer(decided: Decided) -> Answer {
    let unknown = LoadError::UnknownRun;
    match decided {
        Decided::Started(started) => Answer::Started(started),
        Decided::Running(Kept::Report) => Answer::Collected(Collected::Running),
        Decided::Running(Kept::Parts { .. }) => Answer::Parts(PartsCollected::Running),
        Decided::Running(Kept::Counters) => Answer::Counters(CountersCollected::Running),
        Decided::Running(Kept::Link) => Answer::Link(LinkCollected::Running),
        Decided::Running(Kept::Tracking) => Answer::Tracking(TrackingCollected::Running),
        Decided::Running(Kept::Samples { .. }) => Answer::Samples(SamplesCollected::Running),
        Decided::UnknownRun(Kept::Report) => Answer::Collected(Collected::Refused(unknown)),
        Decided::UnknownRun(Kept::Parts { .. }) => Answer::Parts(PartsCollected::Refused(unknown)),
        Decided::UnknownRun(Kept::Counters) => {
            Answer::Counters(CountersCollected::Refused(unknown))
        }
        Decided::UnknownRun(Kept::Link) => Answer::Link(LinkCollected::Refused(unknown)),
        Decided::UnknownRun(Kept::Tracking) => {
            Answer::Tracking(TrackingCollected::Refused(unknown))
        }
        Decided::UnknownRun(Kept::Samples { .. }) => {
            Answer::Samples(SamplesCollected::Refused(unknown))
        }
    }
}

/// The run's socket, bound to `start.port` of the station's DHCP address.
fn open(net: &Net<RX, TX>, start: &Start) -> Result<Socket, LoadError> {
    let address = match net.status().ipv4 {
        Ipv4Status::Lease(configuration) | Ipv4Status::Static(configuration) => {
            configuration.address
        }
        Ipv4Status::Unconfigured => return Err(LoadError::NoAddress),
    };
    let mut socket = UdpSocket::new(net.stack()).map_err(|_| LoadError::Bind)?;
    let local = SocketAddr::from((Ipv4Addr::from(address), start.port));
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
    parts: Windows,
    counters: Counters,
    link: Link,
    tracking: Tracking,
    sampled: Option<Result<(), LoadError>>,
}

/// What an ended run keeps until the next starts; its samples stay in the
/// boot hart's sampler. `sampled` is `None` for a run that asked for none.
struct Ended {
    report: Report,
    parts: Windows,
    counters: Counters,
    link: Link,
    tracking: Tracking,
    sampled: Option<Result<(), LoadError>>,
}

/// Run `start` on `socket` beside the requests until its drain ended, with
/// `tally` cleared for it; what it keeps. A stop of the station closes the
/// socket: the run then receives and sends nothing more and ends at its
/// time.
async fn run(
    requests: &mut Requests,
    socket: Socket,
    start: Start,
    tally: &mut Tally<TALLY_WORDS>,
    net: &Net<RX, TX>,
    hart0: &mut Client,
) -> Ended {
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
            measure(&socket, &start, &window, &mut sent, net, hart0),
        ));
        loop {
            let running = Phase::Running { run: start.run };
            match select(requests.receive(), activity.as_mut()).await {
                Either::First((_identity, request)) => {
                    let answer = match action(running, request) {
                        Action::Answer(decided) => decided_answer(decided),
                        Action::Cpu => Answer::Cpu(cpu_load().await),
                        // A running server starts nothing and keeps no report.
                        Action::Start(_) => Answer::Started(Started::Refused(LoadError::Busy)),
                        Action::Kept(kind) => decided_answer(Decided::Running(kind)),
                    };
                    requests.send(answer).await;
                }
                Either::Second(Either::First(never)) => match never {},
                Either::Second(Either::Second(ends)) => break ends,
            }
        }
    };
    drop(socket);
    let report = Report {
        window_start_us: ends.start_us,
        window_end_us: ends.end_us,
        cpu_start: ends.cpu_start,
        cpu_end: ends.cpu_end,
        received: receiving.received(tally),
        sent,
    };
    Ended {
        report,
        parts: ends.parts,
        counters: ends.counters,
        link: ends.link,
        tracking: ends.tracking,
        sampled: ends.sampled,
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
/// the CPU and the parts, clear the cache's counters and read the counters
/// (both on the boot hart), sample when asked, send within it, close it
/// after `duration_ms`, read them again, stop sampling, then wait out the
/// drain.
async fn measure(
    socket: &Socket,
    start: &Start,
    window: &Window,
    sent: &mut Sent,
    net: &Net<RX, TX>,
    hart0: &mut Client,
) -> Ends {
    let planned_start = now_us().saturating_add(millis(start.delay_ms));
    let planned_end = planned_start.saturating_add(millis(start.duration_ms));
    wait_until(planned_start).await;
    let cpu_start = cpu_load().await;
    let (parts_start, tracking_start) = readings(true).await;
    let counters_start = snapshot(hart0, true, net).await;
    let start_us = now_us();
    window.start_us.set(Some(start_us));
    let sampling = match start.profile {
        Some(profile) => Some(relay::start_sampler(hart0, profile.period_us).await),
        None => None,
    };
    if let Some(transmit) = start.transmit {
        send(socket, start.run, transmit, start_us, planned_end, sent).await;
    }
    wait_until(planned_end).await;
    let end_us = now_us();
    window.end_us.set(Some(end_us));
    relay::request_stop(hart0).await;
    let cpu_end = cpu_load().await;
    let (parts_end, tracking_end) = readings(false).await;
    let counters_end = snapshot(hart0, false, net).await;
    let sampled = match sampling {
        Some(Ok(())) => Some(relay::stop_sampler(hart0).await),
        other => other,
    };
    wait_until(end_us.saturating_add(millis(start.drain_ms))).await;
    Ends {
        start_us,
        end_us,
        cpu_start,
        cpu_end,
        parts: report::parts(&parts_start, &parts_end),
        counters: report::counters(&counters_start, &counters_end),
        link: report::link_window(&counters_start, &counters_end),
        tracking: report::tracking(&tracking_start, &tracking_end),
        sampled,
    }
}

/// One end's counters: the boot hart's and the library's, read there, and
/// the frames the IP stack took; with `start`, the cache's counters
/// cleared first.
async fn snapshot(hart0: &mut Client, start: bool, net: &Net<RX, TX>) -> Snapshot {
    let stack_received = net.stack().statistics().rx_frames;
    Snapshot {
        stack_received,
        ..relay::counters(hart0, start).await.unwrap_or_default()
    }
}

/// Every part's polls and the tracking ticks now, in the current epoch of
/// the longest polls and ticks; with `begin`, a new epoch starts after them.
async fn readings(begin: bool) -> ([PartReading; PARTS], TrackingSample) {
    let parts = observe::parts().await;
    let tracking = observe::tracking().await;
    if begin {
        observe::start_epoch();
    }
    (parts, tracking)
}

/// The CPU report now.
async fn cpu_load() -> Load {
    let samples = observe::executors().await;
    report::load(now_us(), &samples)
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

/// The image's monotonic microseconds: the library's clock.
fn now_us() -> u64 {
    Instant::now().as_micros()
}

async fn wait_until(at_us: u64) {
    Timer::at(Instant::from_micros(at_us)).await;
}
