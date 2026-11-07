//! Cases of the data port ([`DataPort`]) that every backend passes.
//!
//! A backend's tests implement [`DataPortHarness`] over its port and run
//! each case of [`check`]. The cases call the port as its actor does: one
//! `now` per call, a counting waker on every poll, and a sink the harness
//! keeps the terminals in. They check what the contract promises of every
//! backend:
//!
//! - no port call wakes its caller's task ([`Case::QuietCalls`]);
//! - a refusal returns its owners unchanged ([`Case::Refusals`]);
//! - a receive pull gives one unit at a time within the caller's room and
//!   keeps the rest ([`Case::ReceiveRoom`]);
//! - a configuration waits at the published data arena as `Held`, then
//!   runs ([`Case::ConfigureBarrier`]);
//! - the deadline follows a submitted MPDU and its completion
//!   ([`Case::Deadline`]);
//! - a station MPDU carries the Retry bit once an earlier transmission of it
//!   could have reached the air, across every explicit send of its `Sent`
//!   ([`Case::RetryHistory`], [`RETRY_HISTORIES`]).

use core::num::NonZeroU8;
use core::task::{Context, Poll, Waker};

use oer_portable_time::{Duration, Instant};
use oer_protocols_ieee80211_contracts_port::{
    CommandKind, CommandProgress, CommandRefusal, DataPort, PortCommand, PortSink, RxPull,
    SubmitRefusal, SupportedChannel, TxFacts,
};

/// The calls a case may repeat before it gives up on the port's progress.
pub const PROGRESS_STEPS: usize = 64;

/// A command of the port `P`.
pub type PortCommandOf<'id, P> =
    PortCommand<'id, <P as DataPort<'id>>::Types, <P as DataPort<'id>>::Caps>;

/// A backend fixture: its port, the sink of its terminals and the air
/// and time around it.
pub trait DataPortHarness<'id> {
    /// The backend's port.
    type Port: DataPort<'id>;
    /// The sink the harness keeps the port's terminals and frames in.
    type Sink: PortSink<'id, <Self::Port as DataPort<'id>>::Types>;

    /// The port and the sink, borrowed together for one call.
    fn parts(&mut self) -> (&mut Self::Port, &mut Self::Sink);
    /// The fixture's virtual time.
    fn now(&self) -> Instant;
    /// Move virtual time on by `by`.
    fn advance(&mut self, by: Duration);
    /// A waker that counts its wakes.
    fn waker(&self) -> Waker;
    /// The wakes of [`Self::waker`] so far.
    fn wakes(&self) -> usize;
    /// The channel the port starts on.
    fn channel(&self) -> SupportedChannel<<Self::Port as DataPort<'id>>::Caps>;
    /// Let the fixture's hardware make the progress it owes (a start's or
    /// a stop's answer), without touching the port.
    fn settle(&mut self);
    /// Put `units` received MPDUs on the port's ring.
    fn deliver(&mut self, units: usize);
    /// A fresh station MPDU from the fixture's tokens, admitted by the port.
    fn station_mpdu(&mut self) -> Option<<Self::Port as DataPort<'id>>::Admitted>;
    /// The identity of an admitted MPDU's owners, which a refusal keeps.
    fn identity(attempt: &<Self::Port as DataPort<'id>>::Admitted) -> u8;
    /// Keep the next station MPDU's air from answering (`true`) or let it
    /// answer (`false`).
    fn hold_air(&mut self, hold: bool);
    /// The station's receive configuration command, from the fixture's
    /// unique permission.
    fn configure(&mut self) -> Option<PortCommandOf<'id, Self::Port>>;
    /// Take the station's configuration terminal from the sink: whether it
    /// was there.
    fn take_configured(&mut self) -> bool;
    /// Take every single MPDU terminal from the sink, returning its owners
    /// to the fixture: how many there were.
    fn take_completed(&mut self) -> usize;
    /// Fill a data arena with one frame and publish it, the air holding its
    /// answer: whether it went on the air.
    fn publish_held_arena(&mut self) -> bool;
    /// Let the air answer the held arena.
    fn release_arena(&mut self);
    /// Settle a completed arena, returning its frames to the fixture.
    fn settle_arena(
        &mut self,
        done: oer_protocols_ieee80211_contracts_port::PortCompleted<'id, Self::Port>,
    );
    /// Answer the next transmissions of station MPDUs with `ends`, in order.
    fn answer_station(&mut self, ends: &[StationEnd]);
    /// Submit a station data MPDU under one stage of `transmissions`: a fresh
    /// one, or with `again` the `Sent` MPDU the last station terminal
    /// returned, as a retransmission.
    ///
    /// # Errors
    ///
    /// [`Violation::Fixture`] without the owners, a refusal otherwise.
    fn submit_station(&mut self, again: bool, transmissions: NonZeroU8) -> Result<(), Violation>;
    /// Take the station MPDU's terminal from the sink, keeping its `Sent`
    /// for a later send: how it ended and whether its last publication
    /// carried the Retry bit. `None` while it has not ended.
    fn take_station(&mut self) -> Option<StationSent>;
}

/// How one transmission of a station MPDU ends, as the fixture's air
/// answers it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StationEnd {
    /// The ACK came.
    Ack,
    /// No ACK came.
    AckTimeout,
    /// The protection exchange drew no CTS.
    CtsTimeout,
    /// The attempt collided.
    Collision,
    /// The hardware refused the MPDU's key.
    KeyRefused,
    /// No completion: the hardware timed out and the publication is aborted.
    HardwareTimeout,
}

/// How a station send ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SentEnd {
    /// Its solicited response came.
    Delivered,
    /// The plan ran out without delivery.
    Undelivered,
    /// Whether the MPDU went out is unknown.
    Indeterminate,
    /// Its MSDU lifetime ran out before it went out.
    Expired,
    /// Any other end.
    Other,
}

/// A station send's end and the Retry bit of its MPDU as its last
/// publication left it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StationSent {
    /// How the send ended.
    pub end: SentEnd,
    /// Whether its MPDU carries the Retry bit.
    pub retry: bool,
}

/// One explicit send of a station MPDU in a [`RETRY_HISTORIES`] entry.
#[derive(Clone, Copy, Debug)]
pub struct Send {
    /// The transmissions its plan makes and how each ends; an empty list
    /// makes a plan of one that never reaches the air.
    pub ends: &'static [StationEnd],
    /// The MSDU's lifetime runs out before this send.
    pub aged: bool,
    /// What the send returns; the Retry bit is compared only for a send that
    /// reached the air.
    pub expect: StationSent,
}

const fn send(ends: &'static [StationEnd], end: SentEnd, retry: bool) -> Send {
    Send {
        ends,
        aged: false,
        expect: StationSent { end, retry },
    }
}

/// The idle time that runs any MSDU lifetime out.
const AGE: Duration = Duration::from_secs(3_600);

/// Histories of one MPDU, each send after the first a retransmission of the
/// `Sent` the previous one returned. The Retry bit of each send's last
/// publication follows one rule: it is set once an earlier transmission
/// could have reached the air (an ACK timeout, a delivery, or an indeterminate
/// end: a hardware timeout or a refused key), stays set, and is never set by
/// a CTS timeout or a collision alone.
pub const RETRY_HISTORIES: [&[Send]; 21] = {
    use SentEnd::{Delivered, Expired, Indeterminate, Undelivered};
    use StationEnd::{Ack, AckTimeout, Collision, CtsTimeout, HardwareTimeout, KeyRefused};
    [
        &[send(&[Ack], Delivered, false)],
        &[send(&[AckTimeout, Ack], Delivered, true)],
        &[send(&[Collision, Ack], Delivered, false)],
        &[send(&[CtsTimeout, Ack], Delivered, false)],
        &[send(&[Collision, CtsTimeout, Ack], Delivered, false)],
        &[send(&[AckTimeout, Collision, Ack], Delivered, true)],
        &[send(&[AckTimeout, CtsTimeout, Ack], Delivered, true)],
        &[send(&[Collision, AckTimeout, Ack], Delivered, true)],
        &[send(&[CtsTimeout, AckTimeout, Ack], Delivered, true)],
        &[
            send(&[Collision, Collision], Undelivered, false),
            send(&[Ack], Delivered, false),
        ],
        &[
            send(&[CtsTimeout], Undelivered, false),
            send(&[Ack], Delivered, false),
        ],
        &[
            send(&[AckTimeout], Undelivered, false),
            send(&[Ack], Delivered, true),
        ],
        &[
            send(&[AckTimeout], Undelivered, false),
            send(&[Collision], Undelivered, true),
            send(&[Ack], Delivered, true),
        ],
        &[
            send(&[KeyRefused], Indeterminate, false),
            send(&[Ack], Delivered, true),
        ],
        &[
            send(&[AckTimeout, KeyRefused], Indeterminate, true),
            send(&[Ack], Delivered, true),
        ],
        &[
            send(&[HardwareTimeout], Indeterminate, false),
            send(&[Ack], Delivered, true),
        ],
        &[
            send(&[Ack], Delivered, false),
            send(&[Ack], Delivered, true),
        ],
        &[
            send(&[Collision], Undelivered, false),
            send(&[AckTimeout], Undelivered, false),
            send(&[Ack], Delivered, true),
        ],
        &[
            send(&[Collision], Undelivered, false),
            send(&[HardwareTimeout], Indeterminate, false),
            send(&[Ack], Delivered, true),
        ],
        &[
            send(&[Collision], Undelivered, false),
            Send {
                ends: &[],
                aged: true,
                expect: StationSent {
                    end: Expired,
                    retry: false,
                },
            },
        ],
        &[
            send(&[AckTimeout], Undelivered, false),
            Send {
                ends: &[],
                aged: true,
                expect: StationSent {
                    end: Expired,
                    retry: false,
                },
            },
        ],
    ]
};

/// One case of the suite.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Case {
    /// No call wakes its caller's task.
    QuietCalls,
    /// Refusals return their owners unchanged.
    Refusals,
    /// A receive pull gives one unit within the caller's room.
    ReceiveRoom,
    /// A configuration is held at a published arena, then runs.
    ConfigureBarrier,
    /// The deadline follows a published MPDU and its completion.
    Deadline,
    /// Each send of [`RETRY_HISTORIES`] ends and carries Retry as listed.
    RetryHistory,
}

impl Case {
    /// Every case, in suite order.
    pub const ALL: [Self; 6] = [
        Self::QuietCalls,
        Self::Refusals,
        Self::ReceiveRoom,
        Self::ConfigureBarrier,
        Self::Deadline,
        Self::RetryHistory,
    ];
}

/// What a case found wrong.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Violation {
    /// A call woke its caller's task.
    SelfWake,
    /// The port did not start or stop within the case's steps.
    Lifecycle,
    /// A refusal changed or lost an owner, or named another reason.
    Refusal,
    /// A receive pull ignored the room, or lost or invented a unit.
    Receive,
    /// A barrier command ran early or never ran.
    Barrier,
    /// The deadline did not follow the port's queues.
    Deadline,
    /// The port faulted.
    Fault,
    /// The fixture could not set up the case.
    Fixture,
    /// A send of a [`RETRY_HISTORIES`] entry, by its positions, ended
    /// otherwise than listed or never ended.
    Retry {
        /// The entry.
        history: u8,
        /// The send in it.
        send: u8,
    },
}

/// Run `case` on a fresh fixture.
///
/// # Errors
///
/// What the case found wrong.
pub fn check<'id, H: DataPortHarness<'id>>(harness: &mut H, case: Case) -> Result<(), Violation> {
    match case {
        Case::QuietCalls => quiet_calls(harness),
        Case::Refusals => refusals(harness),
        Case::ReceiveRoom => receive_room(harness),
        Case::ConfigureBarrier => configure_barrier(harness),
        Case::Deadline => deadline(harness),
        Case::RetryHistory => retry_history(harness),
    }
}

/// Start the port, settling the fixture between polls.
///
/// # Errors
///
/// [`Violation::Lifecycle`] when it does not start.
pub fn start<'id, H: DataPortHarness<'id>>(harness: &mut H) -> Result<(), Violation> {
    let channel = harness.channel();
    for _ in 0..PROGRESS_STEPS {
        let waker = harness.waker();
        let now = harness.now();
        let (port, _) = harness.parts();
        match port.poll_start(&mut Context::from_waker(&waker), channel, now) {
            Poll::Ready(Ok(())) => return Ok(()),
            Poll::Ready(Err(_)) => return Err(Violation::Lifecycle),
            Poll::Pending => harness.settle(),
        }
    }
    Err(Violation::Lifecycle)
}

/// Serve the transmit path once at the fixture's time.
fn service<'id, H: DataPortHarness<'id>>(
    harness: &mut H,
) -> Result<Option<oer_protocols_ieee80211_contracts_port::PortCompleted<'id, H::Port>>, Violation>
{
    let waker = harness.waker();
    let now = harness.now();
    let (port, sink) = harness.parts();
    let facts = port.poll_facts(&mut Context::from_waker(&waker));
    port.service_tx(facts.tx.unwrap_or(TxFacts::DUE), facts.tbtt, now, sink)
        .map_err(|_| Violation::Fault)
}

fn quiet_calls<'id, H: DataPortHarness<'id>>(harness: &mut H) -> Result<(), Violation> {
    start(harness)?;
    let before = harness.wakes();
    let waker = harness.waker();
    let now = harness.now();
    let (port, sink) = harness.parts();
    let mut cx = Context::from_waker(&waker);
    let facts = port.poll_facts(&mut cx);
    let _ = port.deadline();
    port.service_tx(facts.tx.unwrap_or(TxFacts::DUE), facts.tbtt, now, sink)
        .map_err(|_| Violation::Fault)?;
    match port.receive(8, now).map_err(|_| Violation::Fault)? {
        RxPull::Unit { frame, .. } => {
            port.recycle(frame);
            return Err(Violation::Receive);
        }
        RxPull::Empty | RxPull::Later(_) => {}
    }
    let _ = port.poll_lend_room(&mut cx);
    let _ = port.poll_lent_returned(&mut cx);
    let _ = port.poll_facts(&mut cx);
    if harness.wakes() != before {
        return Err(Violation::SelfWake);
    }
    Ok(())
}

fn refusals<'id, H: DataPortHarness<'id>>(harness: &mut H) -> Result<(), Violation> {
    let attempt = harness.station_mpdu().ok_or(Violation::Fixture)?;
    let now = harness.now();
    let (port, sink) = harness.parts();
    if port.submit(attempt, now, sink) != Err(SubmitRefusal::NotEnabled) {
        return Err(Violation::Refusal);
    }
    // The refused attempt ended withdrawn at once: its terminal returns its
    // owners, and nothing else ended.
    if harness.take_completed() != 1 {
        return Err(Violation::Refusal);
    }
    let command = harness.configure().ok_or(Violation::Fixture)?;
    let now = harness.now();
    let (port, sink) = harness.parts();
    match port
        .command(command, now, sink)
        .map_err(|_| Violation::Fault)?
    {
        CommandProgress::Refused(command, CommandRefusal::NotEnabled)
            if command.kind() == CommandKind::Configure => {}
        _ => return Err(Violation::Refusal),
    }
    if harness.take_configured() || harness.take_completed() != 0 {
        return Err(Violation::Refusal);
    }
    Ok(())
}

fn receive_room<'id, H: DataPortHarness<'id>>(harness: &mut H) -> Result<(), Violation> {
    start(harness)?;
    harness.deliver(3);
    let now = harness.now();
    let (port, _) = harness.parts();
    if let RxPull::Unit { frame, .. } = port.receive(0, now).map_err(|_| Violation::Fault)? {
        port.recycle(frame);
        return Err(Violation::Receive);
    }
    let mut taken = 0;
    for _ in 0..PROGRESS_STEPS {
        let now = harness.now();
        let (port, _) = harness.parts();
        match port.receive(3, now).map_err(|_| Violation::Fault)? {
            RxPull::Unit { frame, .. } => {
                port.recycle(frame);
                taken += 1;
            }
            RxPull::Later(at) if at <= now => {}
            RxPull::Later(at) => harness.advance(at.saturating_duration_since(now)),
            RxPull::Empty => break,
        }
    }
    if taken != 3 {
        return Err(Violation::Receive);
    }
    Ok(())
}

fn configure_barrier<'id, H: DataPortHarness<'id>>(harness: &mut H) -> Result<(), Violation> {
    start(harness)?;
    if !harness.publish_held_arena() {
        return Err(Violation::Fixture);
    }
    let mut command = harness.configure().ok_or(Violation::Fixture)?;
    for _ in 0..2 {
        let now = harness.now();
        let (port, sink) = harness.parts();
        command = match port
            .command(command, now, sink)
            .map_err(|_| Violation::Fault)?
        {
            CommandProgress::Held(command) => command,
            _ => return Err(Violation::Barrier),
        };
        if harness.take_configured() {
            return Err(Violation::Barrier);
        }
    }
    harness.release_arena();
    let mut held = Some(command);
    for _ in 0..PROGRESS_STEPS {
        if let Some(done) = service(harness)? {
            harness.settle_arena(done);
        }
        if let Some(command) = held.take() {
            let now = harness.now();
            let (port, sink) = harness.parts();
            match port
                .command(command, now, sink)
                .map_err(|_| Violation::Fault)?
            {
                CommandProgress::Held(command) => held = Some(command),
                CommandProgress::Done => {}
                CommandProgress::Refused(..) => return Err(Violation::Barrier),
            }
        }
        if held.is_none() && harness.take_configured() {
            return Ok(());
        }
        harness.settle();
    }
    Err(Violation::Barrier)
}

fn deadline<'id, H: DataPortHarness<'id>>(harness: &mut H) -> Result<(), Violation> {
    start(harness)?;
    let idle = {
        let (port, _) = harness.parts();
        port.deadline()
    };
    harness.hold_air(true);
    let attempt = harness.station_mpdu().ok_or(Violation::Fixture)?;
    let now = harness.now();
    let (port, sink) = harness.parts();
    port.submit(attempt, now, sink)
        .map_err(|_| Violation::Refusal)?;
    let _ = service(harness)?;
    let published = {
        let (port, _) = harness.parts();
        port.deadline()
    };
    // A published MPDU bounds the next turn, earlier than an idle port.
    match (published, idle) {
        (Some(published), Some(idle)) if published < idle => {}
        (Some(_), None) => {}
        _ => return Err(Violation::Deadline),
    }
    harness.hold_air(false);
    for _ in 0..PROGRESS_STEPS {
        harness.settle();
        let _ = service(harness)?;
        if harness.take_completed() != 0 {
            let (port, _) = harness.parts();
            let after = port.deadline();
            // The completion took its bound off the cached deadline.
            return if after == published && after.is_some() {
                Err(Violation::Deadline)
            } else {
                Ok(())
            };
        }
    }
    Err(Violation::Deadline)
}

fn retry_history<'id, H: DataPortHarness<'id>>(harness: &mut H) -> Result<(), Violation> {
    start(harness)?;
    for (history, sends) in RETRY_HISTORIES.iter().enumerate() {
        for (at, send) in sends.iter().enumerate() {
            let wrong = Violation::Retry {
                history: u8::try_from(history).unwrap_or(u8::MAX),
                send: u8::try_from(at).unwrap_or(u8::MAX),
            };
            if send.aged {
                harness.advance(AGE);
            }
            harness.answer_station(send.ends);
            let transmissions = u8::try_from(send.ends.len())
                .ok()
                .and_then(NonZeroU8::new)
                .unwrap_or(NonZeroU8::MIN);
            harness.submit_station(at != 0, transmissions)?;
            let sent = serve_station(harness)?.ok_or(wrong)?;
            // A send that never reached the air put no Retry bit on it; what
            // its memory holds then is the backend's own.
            let aired = send.expect.end != SentEnd::Expired;
            if sent.end != send.expect.end || (aired && sent.retry != send.expect.retry) {
                return Err(wrong);
            }
        }
    }
    Ok(())
}

/// Serve the port until the station MPDU's terminal, moving time to the
/// port's deadline whenever two services in a row change nothing.
fn serve_station<'id, H: DataPortHarness<'id>>(
    harness: &mut H,
) -> Result<Option<StationSent>, Violation> {
    let mut idle = 0_u8;
    for _ in 0..PROGRESS_STEPS {
        harness.settle();
        if let Some(done) = service(harness)? {
            harness.settle_arena(done);
        }
        if let Some(sent) = harness.take_station() {
            return Ok(Some(sent));
        }
        idle = idle.saturating_add(1);
        if idle >= 2 {
            idle = 0;
            let now = harness.now();
            let (port, _) = harness.parts();
            if let Some(at) = port.deadline()
                && at > now
            {
                harness.advance(at.saturating_duration_since(now));
            }
        }
    }
    Ok(None)
}
