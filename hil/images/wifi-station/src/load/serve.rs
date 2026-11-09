//! The load server in the group: one run at a time on the image's UDP
//! ([`crate::udp`]), on hart 1 beside the network.
//!
//! A run receives and echoes from its start, sends within its window and
//! reads the CPU, the named parts, the counters, the link's rates, the
//! PHY tracking ticks and the network core's turns at the window's ends (the cache's counters cleared
//! just before its start), sampling the boot hart between them when it
//! asks, and hart 1 beside it; after its drain it ends its socket and
//! keeps what it read until the next run starts. The boot hart's counters
//! and sampler come through its relay ([`super::relay`]); hart 1's counters
//! and sampler are the server's own, its counters read at the window's very
//! ends. After the window's end readings the server asks the station's radio
//! device, through the relay, for a stall snapshot of the receive path; the
//! socket boundary it stamps itself. The answers go back to the worker,
//! which publishes them.

use core::cell::Cell;
use core::convert::Infallible;
use core::pin::pin;

use embassy_futures::select::{Either, select};
use embassy_futures::yield_now;
use oer::observe;
use oer::observe::parts::{PARTS, PartReading};
use oer::observe::tracking::TrackingSample;
use oer::time::{Instant, Timer};
use oer::{Channel, GroupEnd};
use oer_hil_services_load::header::Header;
use oer_hil_services_load::tally::Tally;
use oer_hil_services_load::{
    Addba, AddbaCollected, Collected, Counters, CountersCollected, Hart1Collected, Hart1Window,
    Link, LinkCollected, Load, LoadError, Mac, MacCollected, NetworkCollected, PartsCollected,
    Positions, PositionsCollected, Report, Request, SamplesCollected, Sent, Stall, StallCollected,
    StallMac, StallMacCollected, Start, Started, Tracking, TrackingCollected, Transmit,
};
use oer_protocols_hil_protocol::RequestIdentity;
use static_cell::ConstStaticCell;

use super::action::{Action, Answer as Decided, Kept, Phase, action};
use super::pace::{GRANULARITY_US, Next, Schedule, UNPACED_BATCH};
use super::receive::{Datagram, Receiving as Tallying, SocketStamps, classify, echo};
use super::relay::{self, Client};
use super::report::{self, Snapshot, Windows};
use crate::udp::{self, Attempt, Network, Peer, Receiving, Run, Sending};

const _: () = if oer_hil_services_load::TUNE_KNOBS != oer_portable_trace::tune::KNOBS
    || oer_hil_services_load::TUNE_UNSET != oer_portable_trace::tune::UNSET
{
    panic!("the load service names the image's knobs");
};

/// The run's tally remembers 8192 words, 524 288 sequence numbers in 64 KiB:
/// some 90 s of 1400-byte datagrams at 65 Mbit/s, past the longest window
/// with its delay and drain.
const TALLY_WORDS: usize = 8192;
/// The one tally, in place in a static: it is far larger than a value the
/// image may move, so each run clears it in place.
pub(crate) static TALLY: ConstStaticCell<Tally<TALLY_WORDS>> = ConstStaticCell::new(Tally::new());
/// The tally the group takes once.
pub(crate) type RunTally = &'static mut Tally<TALLY_WORDS>;

/// The load requests the worker hands the server, each with its identity,
/// and the server's answers.
pub(crate) static LOAD: Channel<(RequestIdentity, Request), Answer, 1> = Channel::new();
/// The server's end, in the group.
pub(crate) type Requests = GroupEnd<(RequestIdentity, Request), Answer, 1>;

/// An answer of the load server, which the worker publishes.
pub(crate) enum Answer {
    Started(Started),
    Collected(Collected),
    Parts(PartsCollected),
    Counters(CountersCollected),
    Link(LinkCollected),
    Tracking(TrackingCollected),
    Mac(MacCollected),
    Positions(PositionsCollected),
    Hart1(Hart1Collected),
    Stall(StallCollected),
    StallMac(StallMacCollected),
    Network(NetworkCollected),
    Addba(AddbaCollected),
    Samples(SamplesCollected),
    Cpu(Load),
    /// The boot hart's relay answers it: a page of samples.
    Relayed,
}

impl Answer {
    /// Publish it to the request `identity`.
    pub(crate) fn publish(self, identity: RequestIdentity) {
        use oer_hil_agent::answer;
        match self {
            Self::Started(started) => answer(identity, &started),
            Self::Collected(collected) => answer(identity, &collected),
            Self::Parts(parts) => answer(identity, &parts),
            Self::Counters(counters) => answer(identity, &counters),
            Self::Link(link) => answer(identity, &link),
            Self::Tracking(tracking) => answer(identity, &tracking),
            Self::Mac(mac) => answer(identity, &mac),
            Self::Positions(positions) => answer(identity, &positions),
            Self::Hart1(hart1) => answer(identity, &hart1),
            Self::Stall(stall) => answer(identity, &stall),
            Self::StallMac(mac) => answer(identity, &mac),
            Self::Network(network) => answer(identity, &network),
            Self::Addba(addba) => answer(identity, &addba),
            Self::Samples(samples) => answer(identity, &samples),
            Self::Cpu(load) => answer(identity, &load),
            Self::Relayed => {}
        }
    }
}

/// Serve the load requests until the chip resets, sampling and reading
/// the boot hart through `hart0` and reading and sampling hart 1 with
/// `hart1`.
pub(crate) async fn serve(
    net: &Network,
    mut requests: Requests,
    mut hart0: Client,
    mut hart1: observe::Hart1,
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
                    .and_then(|()| udp::open(net, &start))
                {
                    Ok(socket) => {
                        requests.send(Answer::Started(Started::Running)).await;
                        // Built where it stays: an awaited call would build
                        // the run's future on this poll's stack, then move it.
                        let mut running = pin!(run(
                            &mut requests,
                            socket,
                            start,
                            tally,
                            net,
                            &mut hart0,
                            &mut hart1,
                        ));
                        let ended = running.as_mut().await;
                        kept = Some((start.run, ended));
                        continue;
                    }
                    Err(error) => Answer::Started(Started::Refused(error)),
                }
            }
            Action::Answer(decided) => decided_answer(decided),
            Action::Cpu => Answer::Cpu(cpu_load().await),
            Action::Kept(kind) => match &kept {
                Some((_, ended)) => {
                    kept_answer(ended, kind, identity, net, &mut hart0, &mut hart1).await
                }
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
    net: &Network,
    hart0: &mut Client,
    hart1: &mut observe::Hart1,
) -> Answer {
    match kind {
        Kept::Report => Answer::Collected(Collected::Report(ended.report.clone())),
        Kept::Parts { from } => {
            Answer::Parts(PartsCollected::Report(report::page(&ended.parts, from)))
        }
        Kept::Counters => Answer::Counters(CountersCollected::Report(ended.counters)),
        Kept::Link => Answer::Link(LinkCollected::Report(ended.link)),
        Kept::Mac => Answer::Mac(MacCollected::Report(ended.mac)),
        Kept::Positions => Answer::Positions(PositionsCollected::Report(ended.positions)),
        Kept::Hart1 => Answer::Hart1(Hart1Collected::Report(ended.hart1)),
        Kept::Tracking => Answer::Tracking(TrackingCollected::Report(ended.tracking)),
        Kept::Stall => Answer::Stall(StallCollected::Report(ended.stall)),
        Kept::StallMac => Answer::StallMac(StallMacCollected::Report(ended.stall_mac)),
        // The kept run's window, which the network holds until the next one
        // closes.
        Kept::Network => Answer::Network(NetworkCollected::Report(udp::network_window(net))),
        Kept::Addba => Answer::Addba(AddbaCollected::Report(ended.addba)),
        Kept::Samples { from, hart } => match ended.sampled.get(usize::from(hart)) {
            // Hart 0's samples are in the boot hart's buffer: its relay
            // answers with the page.
            Some(Some(Ok(()))) if hart == 0 => {
                relay::page_of(hart0, identity, from).await;
                Answer::Relayed
            }
            Some(Some(Ok(()))) => Answer::Samples(hart1_page(hart1, from)),
            Some(Some(Err(error))) => Answer::Samples(SamplesCollected::Refused(*error)),
            Some(None) => Answer::Samples(SamplesCollected::Refused(LoadError::NotSampled)),
            None => Answer::Samples(SamplesCollected::Refused(LoadError::NoSampler)),
        },
    }
}

/// Hart 1's last session's page from the sample numbered `from`.
fn hart1_page(hart1: &mut observe::Hart1, from: u16) -> SamplesCollected {
    let sampler = hart1.sampler();
    let page = sampler.session().and_then(|session| {
        sampler.samples(|buffer| {
            report::samples(buffer, from, session.ticks, session.stray, session.cut)
        })
    });
    page.map_or(
        SamplesCollected::Refused(LoadError::NoSampler),
        SamplesCollected::Page,
    )
}

/// Whether the image can sample `start`: a run that asks for samples
/// needs the boot hart's sampler disarmed; hart 1's refusal leaves only
/// hart 1 unsampled.
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
        Decided::Running(Kept::Mac) => Answer::Mac(MacCollected::Running),
        Decided::Running(Kept::Positions) => Answer::Positions(PositionsCollected::Running),
        Decided::Running(Kept::Hart1) => Answer::Hart1(Hart1Collected::Running),
        Decided::Running(Kept::Tracking) => Answer::Tracking(TrackingCollected::Running),
        Decided::Running(Kept::Stall) => Answer::Stall(StallCollected::Running),
        Decided::Running(Kept::StallMac) => Answer::StallMac(StallMacCollected::Running),
        Decided::Running(Kept::Network) => Answer::Network(NetworkCollected::Running),
        Decided::Running(Kept::Addba) => Answer::Addba(AddbaCollected::Running),
        Decided::Running(Kept::Samples { .. }) => Answer::Samples(SamplesCollected::Running),
        Decided::UnknownRun(Kept::Report) => Answer::Collected(Collected::Refused(unknown)),
        Decided::UnknownRun(Kept::Parts { .. }) => Answer::Parts(PartsCollected::Refused(unknown)),
        Decided::UnknownRun(Kept::Counters) => {
            Answer::Counters(CountersCollected::Refused(unknown))
        }
        Decided::UnknownRun(Kept::Link) => Answer::Link(LinkCollected::Refused(unknown)),
        Decided::UnknownRun(Kept::Mac) => Answer::Mac(MacCollected::Refused(unknown)),
        Decided::UnknownRun(Kept::Positions) => {
            Answer::Positions(PositionsCollected::Refused(unknown))
        }
        Decided::UnknownRun(Kept::Hart1) => Answer::Hart1(Hart1Collected::Refused(unknown)),
        Decided::UnknownRun(Kept::Tracking) => {
            Answer::Tracking(TrackingCollected::Refused(unknown))
        }
        Decided::UnknownRun(Kept::Stall) => Answer::Stall(StallCollected::Refused(unknown)),
        Decided::UnknownRun(Kept::StallMac) => {
            Answer::StallMac(StallMacCollected::Refused(unknown))
        }
        Decided::UnknownRun(Kept::Network) => Answer::Network(NetworkCollected::Refused(unknown)),
        Decided::UnknownRun(Kept::Addba) => Answer::Addba(AddbaCollected::Refused(unknown)),
        Decided::UnknownRun(Kept::Samples { .. }) => {
            Answer::Samples(SamplesCollected::Refused(unknown))
        }
    }
}

/// The measuring window's ends as the image kept them, and the run's socket
/// boundary.
struct Window {
    start_us: Cell<Option<u64>>,
    end_us: Cell<Option<u64>>,
    socket: SocketStamps,
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
    mac: Mac,
    positions: Positions,
    hart1: Hart1Window,
    tracking: Tracking,
    stall: Stall,
    stall_mac: StallMac,
    /// The ADDBA Requests the station took since boot, at the window's end.
    addba: Addba,
    sampled: Sampled,
}

/// How each hart's sampler served the window, by hart: `None` for a run
/// that asked for no samples.
type Sampled = [Option<Result<(), LoadError>>; 2];

/// What an ended run keeps until the next starts; its samples stay in
/// each hart's sampler.
struct Ended {
    report: Report,
    parts: Windows,
    counters: Counters,
    link: Link,
    mac: Mac,
    positions: Positions,
    hart1: Hart1Window,
    tracking: Tracking,
    stall: Stall,
    stall_mac: StallMac,
    addba: Addba,
    sampled: Sampled,
}

/// Run `start` on `socket` beside the requests until its drain ended, with
/// `tally` cleared for it; what it keeps. A stop of the station closes the
/// socket: the run then receives and sends nothing more and ends at its
/// time.
async fn run(
    requests: &mut Requests,
    mut socket: Run<'_>,
    start: Start,
    tally: &mut Tally<TALLY_WORDS>,
    net: &Network,
    hart0: &mut Client,
    hart1: &mut observe::Hart1,
) -> Ended {
    tally.clear();
    let window = Window {
        start_us: Cell::new(None),
        end_us: Cell::new(None),
        socket: SocketStamps::default(),
    };
    let mut receiving = Tallying::default();
    let mut sent = Sent::default();
    let ends = {
        // Each future built where it stays, as in `serve`.
        let (mut receiver, mut sender) = socket.split();
        let mut receiver = pin!(receive(
            &mut receiver,
            start.run,
            tally,
            &mut receiving,
            &window
        ));
        let mut measuring = pin!(measure(
            &mut sender,
            &start,
            &window,
            &mut sent,
            net,
            hart0,
            hart1
        ));
        let mut activity = select(receiver.as_mut(), measuring.as_mut());
        loop {
            let running = Phase::Running { run: start.run };
            match select(requests.receive(), &mut activity).await {
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
        mac: ends.mac,
        positions: ends.positions,
        hart1: ends.hart1,
        tracking: ends.tracking,
        stall: ends.stall,
        stall_mac: ends.stall_mac,
        addba: ends.addba,
        sampled: ends.sampled,
    }
}

/// Count every datagram on the port and echo those that ask for it, at once.
async fn receive(
    socket: &mut Receiving<'_>,
    run: u32,
    tally: &mut Tally<TALLY_WORDS>,
    counts: &mut Tallying,
    window: &Window,
) -> Infallible {
    loop {
        socket.ready().await;
        while let Some(mut packet) = socket.take() {
            let now = now_us();
            // An unreadable lent buffer is counted as such, and goes back to
            // the station as its packet drops.
            let (kind, length) = match packet.payload() {
                Ok(payload) => (classify(payload, run), payload.len()),
                Err(_broken) => {
                    counts.unreadable();
                    continue;
                }
            };
            match kind {
                Datagram::Echo(header) => counts.echo(packet.echo(echo(header)).await),
                Datagram::Load(header) => {
                    window.socket.datagram(now);
                    counts.load(tally, header.sequence, length, now, window.contains(now));
                }
                Datagram::Foreign => counts.foreign(),
            }
        }
    }
}

/// The run's window and drain: open the window `delay_ms` after now, read
/// the CPU and the parts, clear the cache's counters and read the counters
/// (both on the boot hart), sample both harts when asked, send within it,
/// close it after `duration_ms`, read them again, stop sampling, ask the
/// radio device for its stall snapshot, then wait out the drain.
async fn measure(
    socket: &mut Sending<'_>,
    start: &Start,
    window: &Window,
    sent: &mut Sent,
    net: &Network,
    hart0: &mut Client,
    hart1: &mut observe::Hart1,
) -> Ends {
    oer_portable_trace::tune::set(start.tune);
    let planned_start = now_us().saturating_add(millis(start.delay_ms));
    let planned_end = planned_start.saturating_add(millis(start.duration_ms));
    wait_until(planned_start).await;
    let cpu_start = cpu_load().await;
    let (parts_start, tracking_start) = readings(true).await;
    let counters_start = snapshot(hart0, true, net).await;
    udp::open_turns(net);
    let hart1_start = report::hart_count(hart1.counters());
    let start_us = now_us();
    window.start_us.set(Some(start_us));
    let sampling = match start.profile {
        Some(profile) => Some(relay::start_sampler(hart0, profile.period_us).await),
        None => None,
    };
    let sampling1 = start.profile.map(|profile| {
        hart1
            .sampler()
            .start(profile.period_us)
            .map_err(|_| LoadError::NoSampler)
    });
    if let Some(transmit) = start.transmit {
        send(socket, start.run, transmit, start_us, planned_end, sent).await;
    }
    wait_until(planned_end).await;
    let end_us = now_us();
    let hart1_end = report::hart_count(hart1.counters());
    window.end_us.set(Some(end_us));
    // Before the first await: the service keeps turning while it runs.
    udp::close_turns(net);
    relay::request_stop(hart0).await;
    hart1.sampler().request_stop();
    let cpu_end = cpu_load().await;
    let (parts_end, tracking_end) = readings(false).await;
    let counters_end = snapshot(hart0, false, net).await;
    let sampled = match sampling {
        Some(Ok(())) => Some(relay::stop_sampler(hart0).await),
        other => other,
    };
    let sampled1 = match sampling1 {
        Some(Ok(())) => Some(
            hart1
                .sampler()
                .stop()
                .await
                .map_err(|_| LoadError::NoSampler),
        ),
        other => other,
    };
    // After every reading of the window: the snapshot's own turn must not
    // move them.
    let socket_last_us = window.socket.last();
    let requested_us = now_us();
    window.socket.watch_from(requested_us);
    // The relay answers each command in order: another reply does not
    // arise, and would leave the run without a snapshot.
    let (snapshot, stall_mac) = relay::stall_snapshot(hart0)
        .await
        .unwrap_or((None, StallMac::default()));
    wait_until(end_us.saturating_add(millis(start.drain_ms))).await;
    let stall = Stall {
        requested_us,
        socket_last_us,
        socket_after_us: window.socket.first_after(),
        snapshot,
    };
    Ends {
        start_us,
        end_us,
        cpu_start,
        cpu_end,
        parts: report::parts(&parts_start, &parts_end),
        counters: report::counters(&counters_start, &counters_end),
        link: report::link_window(&counters_start, &counters_end),
        mac: report::mac_window(&counters_start, &counters_end),
        positions: report::positions_window(&counters_start, &counters_end),
        hart1: Hart1Window {
            start: hart1_start,
            end: hart1_end,
        },
        tracking: report::tracking(&tracking_start, &tracking_end),
        stall,
        stall_mac,
        addba: counters_end.addba,
        sampled: [sampled, sampled1],
    }
}

/// One end's counters: the boot hart's and the library's, read there, and
/// the frames the IP stack took; with `start`, the cache's counters
/// cleared first.
async fn snapshot(hart0: &mut Client, start: bool, net: &Network) -> Snapshot {
    let stack_received = udp::received_frames(net);
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
/// or as fast as the socket takes them. Each datagram is built where it
/// leaves, every octet written in place from its header ([`datagram`]), and
/// the clock read once per batch.
async fn send(
    socket: &mut Sending<'_>,
    run: u32,
    transmit: Transmit,
    begin: u64,
    end: u64,
    sent: &mut Sent,
) {
    let peer = Peer {
        address: transmit.peer,
        port: transmit.peer_port,
    };
    let size = usize::from(transmit.payload);
    let schedule = Schedule::new(
        begin,
        end,
        transmit.payload,
        transmit.rate_bps,
        transmit.burst,
    );
    match schedule {
        Some(mut schedule) => loop {
            let now = now_us();
            let next = schedule.batch(now, || {
                let header = datagram(run, sent.datagrams, now);
                match socket.try_send(size, peer, header) {
                    Attempt::Taken => {
                        taken(sent, size, now);
                        true
                    }
                    Attempt::NoRoom => {
                        sent.refused = sent.refused.saturating_add(1);
                        false
                    }
                    // The datagram is lost; its time passes.
                    Attempt::Lost => {
                        sent.failed = sent.failed.saturating_add(1);
                        true
                    }
                }
            });
            match next {
                Next::Yield => yield_now().await,
                Next::Wait(at) => wait_until(at).await,
                // The socket had no room: the due datagram waits for it,
                // as an application's send does.
                Next::Blocked => {
                    let sequence = sent.datagrams;
                    let datagram = blocking(socket, size, peer, run, sequence, now);
                    match select(datagram, wait_until(end)).await {
                        Either::First(took) => {
                            let now = now_us();
                            if took {
                                taken(sent, size, now);
                            } else {
                                sent.failed = sent.failed.saturating_add(1);
                            }
                            schedule.taken(now);
                        }
                        Either::Second(()) => {
                            sent.missed = schedule.missed();
                            return;
                        }
                    }
                }
                Next::Done => {
                    sent.missed = schedule.missed();
                    return;
                }
            }
        },
        None => unpaced(socket, run, size, peer, end, sent).await,
    }
}

/// Datagram `sequence`, sent at `now`, once the socket has room for it.
async fn blocking(
    socket: &mut Sending<'_>,
    size: usize,
    peer: Peer,
    run: u32,
    sequence: u64,
    now: u64,
) -> bool {
    socket.send(size, peer, datagram(run, sequence, now)).await
}

/// Send as fast as the socket takes the datagrams until `end`: one deadline
/// around the whole loop, the clock read and a yield to the executor once
/// per [`UNPACED_BATCH`]; after a failed send, wait the pacing granularity.
async fn unpaced(
    socket: &mut Sending<'_>,
    run: u32,
    size: usize,
    peer: Peer,
    end: u64,
    sent: &mut Sent,
) {
    let sending = async {
        let mut now = now_us();
        let mut batch = 0;
        loop {
            if blocking(socket, size, peer, run, sent.datagrams, now).await {
                taken(sent, size, now);
            } else {
                sent.failed = sent.failed.saturating_add(1);
                wait_until(now_us().saturating_add(GRANULARITY_US)).await;
            }
            batch += 1;
            if batch == UNPACED_BATCH {
                batch = 0;
                yield_now().await;
                now = now_us();
            }
        }
    };
    select(sending, wait_until(end)).await;
}

/// The header of datagram `sequence`, sent at `now`. The image's UDP writes
/// the whole payload from it where it leaves, the header at its front and the
/// filler behind it ([`Header::write_datagram`]): every octet, on both stacks
/// alike.
const fn datagram(run: u32, sequence: u64, now: u64) -> Header {
    Header {
        run,
        sequence,
        sent_us: now,
        echo_request: false,
        echo_reply: false,
    }
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
