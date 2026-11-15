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
//!   ([`Case::RetryHistory`], [`RETRY_HISTORIES`]);
//! - a proof holds only while the port's state it proves lasts: a second
//!   copy of a stop's proof is refused once the port retired, a stop of a
//!   retired port stops it again under a new proof (a retirement held
//!   across it is refused, one dropped is asked again), a start's proof is
//!   refused once a stop began, a proof of the installation before a
//!   reinstall is refused at every call, a refusal returns its proof and
//!   leaves what the port has out as it was, and a release takes the
//!   stop's right ([`Case::Proofs`]);
//! - each reinstall is at the port's next TSF epoch, and a port at its last
//!   refuses with the proof back ([`Case::Epochs`]).

use core::num::NonZeroU8;
use core::task::{Context, Waker};

use oer_portable_time::{Duration, Instant};
use oer_protocols_ieee80211_contracts_port::{
    CleanupRefused, CommandKind, CommandProgress, CommandRefusal, DataPort, PortCommand, PortSink,
    ProofRefused, ReinstallRefused, Retired, RxPull, StartProgress, StartProof, StartRefused,
    Stopped, SubmitRefusal, SupportedChannel, TxFacts,
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
    /// The port and the stop's right the harness holds, borrowed together
    /// for a stop or a recovery: the port gave the right with its first
    /// installation. `None` once a release took it.
    fn stop_parts(
        &mut self,
    ) -> Option<(
        &mut Self::Port,
        &mut oer_protocols_ieee80211_contracts_port::StopRight<'id>,
    )>;
    /// Take the stop's right from the harness, for a release: `None` once
    /// one took it.
    fn take_right(&mut self) -> Option<oer_protocols_ieee80211_contracts_port::StopRight<'id>>;
    /// Keep the stop's right a refused release gave back.
    fn keep_right(&mut self, right: oer_protocols_ieee80211_contracts_port::StopRight<'id>);
    /// Take the stop's proof the harness keeps: the port's installation
    /// starts stopped, so the harness holds one until a start takes it.
    fn take_stopped(&mut self) -> Option<oer_protocols_ieee80211_contracts_port::Stopped<'id>>;
    /// Keep the stop's proof `stopped` for the next start, clean or
    /// retirement.
    fn keep_stopped(&mut self, stopped: oer_protocols_ieee80211_contracts_port::Stopped<'id>);
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
    /// Keep the next station MPDU's air from answering, or let it answer.
    fn hold_air(&mut self, hold: crate::Hold);
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
    fn submit_station(
        &mut self,
        submission: crate::Submission,
        transmissions: NonZeroU8,
    ) -> Result<(), Violation>;
    /// Take the station MPDU's terminal from the sink, keeping its `Sent`
    /// for a later send: how it ended and whether its last publication
    /// carried the Retry bit. `None` while it has not ended.
    fn take_station(&mut self) -> Option<StationSent>;
    /// Send the fixture's transmit tokens home (its tickets and buffers):
    /// what a retirement needs of the fixture.
    fn end_tokens(&mut self);
    /// The transmit buffers of the current installation the fixture holds
    /// itself (its own and its kept station MPDU's): what it was handed and
    /// has not handed the port.
    fn buffers_held(&self) -> usize;
    /// Make the sink refuse its next `count` deliveries, terminals and
    /// units alike.
    fn refuse(&mut self, count: usize);
    /// The refusals [`Self::refuse`] asked that the sink has not made yet.
    fn refusals_left(&self) -> usize;
    /// Fail the fixture's hardware at the port's next service: a fault
    /// that leaves every owner whole, or one that holds owners no stop
    /// returns, as `reach` says.
    fn fault(&mut self, reach: crate::FaultReach);
    /// Fail the port's next stop poll with a fault of the device, once:
    /// the stop after it goes on as before.
    fn fail_next_stop(&mut self);
    /// Hold every start of the port before it ends, while `hold` is set:
    /// its poll answers that it goes on.
    fn hold_start(&mut self, hold: crate::Hold);
    /// Hold every stop of the port unconfirmed, while `hold` is set.
    fn hold_stop(&mut self, hold: crate::Hold);
    /// The TSF epoch the port's installation runs at, as the fixture reads
    /// it: the contract does not show it.
    fn epoch(&self) -> u32;
    /// Put the port at its last TSF epoch, which no reinstall can raise.
    fn last_epoch(&mut self);
    /// Reinstall the port in place with `retired`, from the fixture's
    /// installations (the port raises its TSF epoch): the new
    /// installation's stop. Its
    /// tokens end with the fixture.
    ///
    /// # Errors
    ///
    /// The port's refusal, with the proof back.
    fn reinstall(
        &mut self,
        retired: Retired<'id>,
    ) -> Result<Stopped<'id>, ProofRefused<Retired<'id>, ReinstallRefused>>;
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

/// The Retry bit a send's last publication carries: clear or set.
#[derive(Clone, Copy)]
enum RetryBit {
    Clear,
    Set,
}

const fn send(ends: &'static [StationEnd], end: SentEnd, retry: RetryBit) -> Send {
    Send {
        ends,
        aged: false,
        expect: StationSent {
            end,
            retry: matches!(retry, RetryBit::Set),
        },
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
        &[send(&[Ack], Delivered, RetryBit::Clear)],
        &[send(&[AckTimeout, Ack], Delivered, RetryBit::Set)],
        &[send(&[Collision, Ack], Delivered, RetryBit::Clear)],
        &[send(&[CtsTimeout, Ack], Delivered, RetryBit::Clear)],
        &[send(
            &[Collision, CtsTimeout, Ack],
            Delivered,
            RetryBit::Clear,
        )],
        &[send(
            &[AckTimeout, Collision, Ack],
            Delivered,
            RetryBit::Set,
        )],
        &[send(
            &[AckTimeout, CtsTimeout, Ack],
            Delivered,
            RetryBit::Set,
        )],
        &[send(
            &[Collision, AckTimeout, Ack],
            Delivered,
            RetryBit::Set,
        )],
        &[send(
            &[CtsTimeout, AckTimeout, Ack],
            Delivered,
            RetryBit::Set,
        )],
        &[
            send(&[Collision, Collision], Undelivered, RetryBit::Clear),
            send(&[Ack], Delivered, RetryBit::Clear),
        ],
        &[
            send(&[CtsTimeout], Undelivered, RetryBit::Clear),
            send(&[Ack], Delivered, RetryBit::Clear),
        ],
        &[
            send(&[AckTimeout], Undelivered, RetryBit::Clear),
            send(&[Ack], Delivered, RetryBit::Set),
        ],
        &[
            send(&[AckTimeout], Undelivered, RetryBit::Clear),
            send(&[Collision], Undelivered, RetryBit::Set),
            send(&[Ack], Delivered, RetryBit::Set),
        ],
        &[
            send(&[KeyRefused], Indeterminate, RetryBit::Clear),
            send(&[Ack], Delivered, RetryBit::Set),
        ],
        &[
            send(&[AckTimeout, KeyRefused], Indeterminate, RetryBit::Set),
            send(&[Ack], Delivered, RetryBit::Set),
        ],
        &[
            send(&[HardwareTimeout], Indeterminate, RetryBit::Clear),
            send(&[Ack], Delivered, RetryBit::Set),
        ],
        &[
            send(&[Ack], Delivered, RetryBit::Clear),
            send(&[Ack], Delivered, RetryBit::Set),
        ],
        &[
            send(&[Collision], Undelivered, RetryBit::Clear),
            send(&[AckTimeout], Undelivered, RetryBit::Clear),
            send(&[Ack], Delivered, RetryBit::Set),
        ],
        &[
            send(&[Collision], Undelivered, RetryBit::Clear),
            send(&[HardwareTimeout], Indeterminate, RetryBit::Clear),
            send(&[Ack], Delivered, RetryBit::Set),
        ],
        &[
            send(&[Collision], Undelivered, RetryBit::Clear),
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
            send(&[AckTimeout], Undelivered, RetryBit::Clear),
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
    /// A proof holds only while the state it proves lasts.
    Proofs,
    /// Each reinstall is at the port's next TSF epoch; a port at its last
    /// refuses `Epochs` with the proof back.
    Epochs,
    /// What the installation issued is home or out, class by class, after
    /// every call, by the suite's own ledger.
    Conservation,
    /// A fault that leaves its owners whole is taken back, and its stop
    /// returns them all, by the ledger.
    FaultWhole,
    /// A fault that holds owners no stop returns is kept, with what it
    /// leaves out, and the port does not retire, by the ledger.
    FaultHolding,
}

impl Case {
    /// Every case, in suite order.
    pub const ALL: [Self; 11] = [
        Self::QuietCalls,
        Self::Refusals,
        Self::ReceiveRoom,
        Self::ConfigureBarrier,
        Self::Deadline,
        Self::RetryHistory,
        Self::Proofs,
        Self::Epochs,
        Self::Conservation,
        Self::FaultWhole,
        Self::FaultHolding,
    ];
}

/// What a case found wrong.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Violation {
    /// A call returned an item it could neither give its sink nor keep.
    Unkept,
    /// A call woke its caller's task.
    SelfWake,
    /// The port did not start or stop within the case's steps.
    Lifecycle,
    /// A refusal changed or lost an owner, or named another reason.
    Refusal,
    /// A receive pull ignored the room, lost or invented a unit, or the
    /// port refused the recycle of a unit it gave.
    Receive,
    /// A barrier command ran early or never ran.
    Barrier,
    /// The deadline did not follow the port's queues.
    Deadline,
    /// The port faulted.
    Fault,
    /// The fixture could not set up the case.
    Fixture,
    /// The port took a proof of a state it left, or a refusal did not
    /// return its proof or changed what the port has out: the step of
    /// [`Case::Proofs`] that found it.
    Proof(u8),
    /// A reinstall was not at the port's next TSF epoch, or a port at its
    /// last epoch did not refuse with the proof back: the step of
    /// [`Case::Epochs`] that found it.
    Epoch(u8),
    /// What the port has out differs from the suite's ledger after the
    /// `step` of [`Case::Conservation`], in the classes `classes`
    /// ([`outstanding::class`](oer_protocols_ieee80211_contracts_port::outstanding::class)).
    Conserved {
        /// The step.
        step: u8,
        /// The classes that differ.
        classes: u32,
    },
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
        Case::Proofs => proofs(harness),
        Case::Epochs => epochs(harness),
        Case::Conservation => conservation(harness),
        Case::FaultWhole => fault_conservation(harness, crate::FaultReach::Whole),
        Case::FaultHolding => fault_conservation(harness, crate::FaultReach::Holding),
    }
}

/// Start the port, settling the fixture between polls.
///
/// # Errors
///
/// [`Violation::Lifecycle`] when it does not start.
pub fn start<'id, H: DataPortHarness<'id>>(harness: &mut H) -> Result<(), Violation> {
    let channel = harness.channel();
    let Some(stopped) = harness.take_stopped() else {
        return Err(Violation::Lifecycle);
    };
    let mut proof = oer_protocols_ieee80211_contracts_port::StartProof::from(stopped);
    for _ in 0..PROGRESS_STEPS {
        let waker = harness.waker();
        let now = harness.now();
        let (port, _) = harness.parts();
        match port.poll_start(&mut Context::from_waker(&waker), proof, channel, now) {
            Ok(StartProgress::Started) => return Ok(()),
            Ok(StartProgress::Pending(starting)) => {
                proof = starting.into();
                harness.settle();
            }
            Err(StartRefused::Refused(proof, _)) => {
                harness.keep_stopped(proof);
                return Err(Violation::Lifecycle);
            }
            Err(StartRefused::Stale(stale)) => {
                if let oer_protocols_ieee80211_contracts_port::StartProof::Stopped(stopped) =
                    stale.proof
                {
                    harness.keep_stopped(stopped);
                }
                return Err(Violation::Lifecycle);
            }
            Err(StartRefused::Unstopped { .. }) => return Err(Violation::Lifecycle),
        }
    }
    // A start still going on after the case's steps: its proof ends here.
    drop(proof);
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
    crate::answered(
        port.service_tx(facts.tx.unwrap_or(TxFacts::DUE), facts.tbtt, now, sink),
        Violation::Unkept,
    )?
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
    crate::answered(
        port.service_tx(facts.tx.unwrap_or(TxFacts::DUE), facts.tbtt, now, sink),
        Violation::Unkept,
    )?
    .map_err(|_| Violation::Fault)?;
    match port.receive(8, now).map_err(|_| Violation::Fault)? {
        RxPull::Unit { frame, .. } => {
            let _recycled = port.recycle(frame);
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
    if crate::answered(port.submit(attempt, now, sink), Violation::Unkept)?
        != Err(SubmitRefusal::NotEnabled)
    {
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
    match crate::answered(port.command(command, now, sink), Violation::Unkept)?
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
        let _recycled = port.recycle(frame);
        return Err(Violation::Receive);
    }
    let mut taken = 0;
    for _ in 0..PROGRESS_STEPS {
        let now = harness.now();
        let (port, _) = harness.parts();
        match port.receive(3, now).map_err(|_| Violation::Fault)? {
            RxPull::Unit { frame, .. } => {
                port.recycle(frame).map_err(|_| Violation::Receive)?;
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
        command = match crate::answered(port.command(command, now, sink), Violation::Unkept)?
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
            match crate::answered(port.command(command, now, sink), Violation::Unkept)?
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
    harness.hold_air(crate::Hold::Held);
    let attempt = harness.station_mpdu().ok_or(Violation::Fixture)?;
    let now = harness.now();
    let (port, sink) = harness.parts();
    crate::answered(port.submit(attempt, now, sink), Violation::Unkept)?
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
    harness.hold_air(crate::Hold::Released);
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
            let submission = if at == 0 {
                crate::Submission::First
            } else {
                crate::Submission::Again
            };
            harness.submit_station(submission, transmissions)?;
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

/// Poll the port's stop to its confirmation, serving it between polls: its
/// proof.
fn stopped<'id, H: DataPortHarness<'id>>(harness: &mut H) -> Result<Stopped<'id>, Violation> {
    for _ in 0..PROGRESS_STEPS {
        let waker = harness.waker();
        let now = harness.now();
        let (port, right) = harness.stop_parts().ok_or(Violation::Fixture)?;
        match port.poll_stop(right, &mut Context::from_waker(&waker), now) {
            core::task::Poll::Ready(Ok(stopped)) => return Ok(stopped),
            core::task::Poll::Ready(Err(_)) => return Err(Violation::Lifecycle),
            core::task::Poll::Pending => {
                let _served = service(harness)?;
                harness.settle();
            }
        }
    }
    Err(Violation::Lifecycle)
}

/// Every call that takes a stop's proof refuses `stale`, returns it and
/// leaves what the port has out as it was; `copy` is a proof of the same
/// state, which the returned one equals. `step` names the first call of
/// the four in a violation.
fn refuses_stale_stop<'id, H: DataPortHarness<'id>>(
    harness: &mut H,
    stale: Stopped<'id>,
    copy: &Stopped<'id>,
    step: u8,
) -> Result<Stopped<'id>, Violation> {
    let channel = harness.channel();
    let waker = harness.waker();
    let now = harness.now();
    let (port, sink) = harness.parts();
    let before = port.outstanding();
    let stale = match port.poll_start(&mut Context::from_waker(&waker), stale.into(), channel, now)
    {
        Err(StartRefused::Stale(refused)) => match refused.proof {
            StartProof::Stopped(stopped) if stopped == *copy => stopped,
            _ => return Err(Violation::Proof(step)),
        },
        _ => return Err(Violation::Proof(step)),
    };
    if port.outstanding() != before {
        return Err(Violation::Proof(step));
    }
    if crate::answered(port.clean(&stale, sink), Violation::Unkept)? != Err(CleanupRefused::Stale)
        || port.outstanding() != before
    {
        return Err(Violation::Proof(step + 1));
    }
    if port.take_unconsumed(&stale, sink).is_ok() || port.outstanding() != before {
        return Err(Violation::Proof(step + 2));
    }
    let stale = match port.retire(stale) {
        Err(ProofRefused::Stale(refused)) if refused.proof == *copy => refused.proof,
        _ => return Err(Violation::Proof(step + 3)),
    };
    if port.outstanding() != before {
        return Err(Violation::Proof(step + 3));
    }
    Ok(stale)
}

fn proofs<'id, H: DataPortHarness<'id>>(harness: &mut H) -> Result<(), Violation> {
    // The installation's stop, and a second copy of it from the stopped
    // port's stop: the same proof.
    let first = harness.take_stopped().ok_or(Violation::Fixture)?;
    let copy = stopped(harness)?;
    if copy != first {
        return Err(Violation::Proof(0));
    }
    // A third copy, to compare what the refusals return with.
    let witness = stopped(harness)?;
    harness.end_tokens();
    let (port, _) = harness.parts();
    let Ok(retired) = port.retire(first) else {
        return Err(Violation::Proof(1));
    };
    // The port retired: the second copy holds no more, at any call.
    let copy = refuses_stale_stop(harness, copy, &witness, 2)?;
    // A stop of the retired port stops it again under a new proof, which
    // the port takes; the retirement held across it is refused.
    let again = stopped(harness)?;
    let right = harness.take_right().ok_or(Violation::Fixture)?;
    let (port, _) = harness.parts();
    let before = port.outstanding();
    let refused = match port.release(retired, right) {
        Err(refused) => refused,
        Ok(_) => return Err(Violation::Proof(6)),
    };
    if port.outstanding() != before {
        return Err(Violation::Proof(6));
    }
    harness.keep_right(refused.right);
    let retired = refused.proof;
    if !matches!(harness.reinstall(retired), Err(ProofRefused::Stale(_))) {
        return Err(Violation::Proof(7));
    }
    // A retirement dropped by its holder leaves a way to the end: the stop
    // gives a proof the port takes, and the retirement is asked again.
    let (port, _) = harness.parts();
    let Ok(dropped) = port.retire(again) else {
        return Err(Violation::Proof(8));
    };
    drop(dropped);
    let again = stopped(harness)?;
    let (port, _) = harness.parts();
    let Ok(retired) = port.retire(again) else {
        return Err(Violation::Proof(9));
    };
    // The next installation: the proofs of the one before stay refused.
    let next = harness
        .reinstall(retired)
        .map_err(|_| Violation::Proof(10))?;
    let copy = refuses_stale_stop(harness, copy, &witness, 11)?;
    drop(copy);
    // A retirement's proof has no second copy: one made at the first
    // installation's serial, after the port moved, is refused by a release
    // and a reinstall, and nothing of what the port has out changes.
    let right = harness.take_right().ok_or(Violation::Fixture)?;
    let (port, _) = harness.parts();
    let before = port.outstanding();
    let forged = Retired::issue(oer_protocols_ieee80211_contracts_port::StateSerial::FIRST);
    let Err(refused) = port.release(forged, right) else {
        return Err(Violation::Proof(15));
    };
    if port.outstanding() != before {
        return Err(Violation::Proof(15));
    }
    harness.keep_right(refused.right);
    if !matches!(
        harness.reinstall(refused.proof),
        Err(ProofRefused::Stale(_))
    ) {
        return Err(Violation::Proof(16));
    }
    // A start going on ends with the stop that begins meanwhile, before the
    // stop confirms: its proof is refused at the next poll.
    harness.hold_start(crate::Hold::Held);
    harness.hold_stop(crate::Hold::Held);
    let channel = harness.channel();
    let waker = harness.waker();
    let now = harness.now();
    let (port, right) = harness.stop_parts().ok_or(Violation::Fixture)?;
    let Ok(StartProgress::Pending(starting)) =
        port.poll_start(&mut Context::from_waker(&waker), next.into(), channel, now)
    else {
        return Err(Violation::Proof(17));
    };
    let _begun = port.poll_stop(right, &mut Context::from_waker(&waker), now);
    let before = port.outstanding();
    match port.poll_start(
        &mut Context::from_waker(&waker),
        starting.into(),
        channel,
        now,
    ) {
        Err(StartRefused::Stale(refused)) if matches!(refused.proof, StartProof::Starting(_)) => {}
        _ => return Err(Violation::Proof(18)),
    }
    if port.outstanding() != before {
        return Err(Violation::Proof(18));
    }
    harness.hold_start(crate::Hold::Released);
    harness.hold_stop(crate::Hold::Released);
    // The stop gives the next proof, and the installation still runs and
    // ends.
    let again = stopped(harness)?;
    harness.keep_stopped(again);
    start(harness)?;
    let stop = stopped(harness)?;
    harness.end_tokens();
    let (port, _) = harness.parts();
    let Ok(retired) = port.retire(stop) else {
        return Err(Violation::Proof(19));
    };
    let Some(right) = harness.take_right() else {
        return Err(Violation::Fixture);
    };
    let (port, _) = harness.parts();
    // The release takes the right: no stop of the released port can be
    // written (the contract's compile-fail case `stop_after_release`).
    if port.release(retired, right).is_err() {
        return Err(Violation::Proof(20));
    }
    Ok(())
}

fn epochs<'id, H: DataPortHarness<'id>>(harness: &mut H) -> Result<(), Violation> {
    let first = harness.take_stopped().ok_or(Violation::Fixture)?;
    harness.end_tokens();
    let (port, _) = harness.parts();
    let Ok(retired) = port.retire(first) else {
        return Err(Violation::Epoch(0));
    };
    // Each reinstall is at the port's next epoch.
    let before = harness.epoch();
    let next = harness
        .reinstall(retired)
        .map_err(|_| Violation::Epoch(1))?;
    if Some(harness.epoch()) != before.checked_add(1) {
        return Err(Violation::Epoch(1));
    }
    harness.end_tokens();
    let (port, _) = harness.parts();
    let Ok(retired) = port.retire(next) else {
        return Err(Violation::Epoch(2));
    };
    // A port at its last epoch refuses with the proof back, and stays at it.
    harness.last_epoch();
    let last = harness.epoch();
    let retired = match harness.reinstall(retired) {
        Err(ProofRefused::Refused(retired, ReinstallRefused::Epochs)) => retired,
        _ => return Err(Violation::Epoch(3)),
    };
    if harness.epoch() != last {
        return Err(Violation::Epoch(3));
    }
    // The proof came back whole: the port refuses it again for the epoch,
    // not as stale.
    match harness.reinstall(retired) {
        Err(ProofRefused::Refused(_, ReinstallRefused::Epochs)) => Ok(()),
        _ => Err(Violation::Epoch(4)),
    }
}

/// The suite's own ledger of what the port handed it and what it gave
/// back: the classes of [`Outstanding`] it can count from its side, each
/// compared after every call of [`Case::Conservation`].
///
/// [`Outstanding`]: oer_protocols_ieee80211_contracts_port::Outstanding
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct Ledger {
    /// Whether the radio runs or a start or stop is under way.
    radio: LedgerRadio,
    /// Received units the suite holds.
    received: usize,
    /// Received units it lent to the network, not yet dropped.
    lent: usize,
    /// Attempts it submitted whose terminal it has not taken.
    in_flight: usize,
    /// Terminals the sink refused, which the port keeps.
    refused: usize,
    /// Whether an aggregate arena it published has not settled.
    arena: LedgerArena,
    /// Whether the port reported a fault it has not taken back.
    fault: LedgerFault,
}

/// The ledger's radio: stopped, or running or between.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum LedgerRadio {
    #[default]
    Stopped,
    Active,
}

/// The ledger's aggregate arena: settled, or not.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum LedgerArena {
    #[default]
    Settled,
    Unsettled,
}

/// The ledger's fault: none, or one the port keeps.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum LedgerFault {
    #[default]
    Clear,
    Held,
}

impl Ledger {
    /// The classes in which `outstanding` differs from the ledger, the
    /// transmit buffers with the fixture's own `held`.
    fn differs<F>(
        &self,
        outstanding: &oer_protocols_ieee80211_contracts_port::Outstanding<F>,
        held: usize,
    ) -> u32 {
        use oer_protocols_ieee80211_contracts_port::{RunPhase, outstanding::class};
        let mut classes = 0;
        if (outstanding.phase != RunPhase::Stopped) != matches!(self.radio, LedgerRadio::Active) {
            classes |= class::PHASE;
        }
        if outstanding.received != self.received {
            classes |= class::RECEIVED;
        }
        if outstanding.lent != self.lent {
            // Lent units are no class: the bit names them here alone.
            classes |= 1 << 31;
        }
        if outstanding.count(class::QUEUED) + outstanding.running != self.in_flight {
            classes |= class::QUEUED | class::RUNNING;
        }
        if outstanding.fault.is_some() != matches!(self.fault, LedgerFault::Held) {
            classes |= class::FAULT;
        }
        if outstanding.unconsumed != self.refused {
            classes |= class::UNCONSUMED;
        }
        if (outstanding.arenas == oer_protocols_ieee80211_contracts_port::ArenaState::Held)
            != matches!(self.arena, LedgerArena::Unsettled)
        {
            classes |= class::ARENAS;
        }
        // Every buffer the installation issued is with the fixture, in an
        // attempt out, or in a refused terminal the port keeps.
        if outstanding.transmit_out != held + self.in_flight + self.refused {
            classes |= class::TRANSMIT_OUT;
        }
        classes
    }

    /// Compare with the port after `step`.
    fn check<'id, H: DataPortHarness<'id>>(
        &self,
        harness: &mut H,
        step: u8,
    ) -> Result<(), Violation> {
        let held = harness.buffers_held();
        let (port, _) = harness.parts();
        match self.differs(&port.outstanding(), held) {
            0 => Ok(()),
            classes => Err(Violation::Conserved { step, classes }),
        }
    }
}

fn conservation<'id, H: DataPortHarness<'id>>(harness: &mut H) -> Result<(), Violation> {
    let mut ledger = Ledger::default();
    ledger.check(harness, 0)?;
    start(harness)?;
    ledger.radio = LedgerRadio::Active;
    ledger.check(harness, 1)?;
    // Two units received: one recycled, one lent to the network.
    harness.deliver(2);
    let mut units = (None, None);
    for _ in 0..PROGRESS_STEPS {
        let now = harness.now();
        let (port, _) = harness.parts();
        match port.receive(2, now).map_err(|_| Violation::Fault)? {
            RxPull::Unit { frame, .. } => {
                if units.0.is_none() {
                    units.0 = Some(frame);
                } else {
                    units.1 = Some(frame);
                }
                ledger.received += 1;
                ledger.check(harness, 2)?;
                if units.1.is_some() {
                    break;
                }
            }
            _ => harness.settle(),
        }
    }
    let (Some(recycled), Some(lent)) = units else {
        return Err(Violation::Fixture);
    };
    let (port, _) = harness.parts();
    if port.recycle(recycled).is_err() {
        return Err(Violation::Receive);
    }
    ledger.received -= 1;
    ledger.check(harness, 3)?;
    let (port, _) = harness.parts();
    let Ok(lent) = port.lend(lent) else {
        return Err(Violation::Receive);
    };
    ledger.received -= 1;
    ledger.lent += 1;
    ledger.check(harness, 4)?;
    // An aggregate on the air when the stop begins, and a station MPDU
    // queued behind it.
    if !harness.publish_held_arena() {
        return Err(Violation::Fixture);
    }
    ledger.arena = LedgerArena::Unsettled;
    ledger.check(harness, 5)?;
    let attempt = harness.station_mpdu().ok_or(Violation::Fixture)?;
    let now = harness.now();
    let (port, sink) = harness.parts();
    crate::answered(port.submit(attempt, now, sink), Violation::Unkept)?
        .map_err(|_| Violation::Refusal)?;
    ledger.in_flight += 1;
    ledger.check(harness, 6)?;
    let _served = service(harness)?;
    ledger.check(harness, 14)?;
    // The station MPDU's terminal refused by the sink at the stop: the
    // port keeps it.
    harness.refuse(1);
    harness.hold_air(crate::Hold::Released);
    let mut stop = None;
    for _ in 0..PROGRESS_STEPS {
        let waker = harness.waker();
        let now = harness.now();
        let (port, right) = harness.stop_parts().ok_or(Violation::Fixture)?;
        match port.poll_stop(right, &mut Context::from_waker(&waker), now) {
            core::task::Poll::Ready(Ok(stopped)) => {
                stop = Some(stopped);
                break;
            }
            core::task::Poll::Ready(Err(_)) => return Err(Violation::Lifecycle),
            core::task::Poll::Pending => {
                // The stop waits for the aggregate: it ends, and settles.
                harness.release_arena();
                if let Some(done) = service(harness)? {
                    harness.settle_arena(done);
                    ledger.arena = LedgerArena::Settled;
                }
                harness.settle();
            }
        }
        if harness.take_completed() > 0 {
            ledger.in_flight -= 1;
        }
        if harness.refusals_left() == 0 && ledger.in_flight > 0 {
            // The sink refused the MPDU's terminal: the port keeps it.
            ledger.in_flight -= 1;
            ledger.refused += 1;
        }
        ledger.radio = LedgerRadio::Active;
        ledger.check(harness, 15)?;
    }
    let stopped = stop.ok_or(Violation::Lifecycle)?;
    ledger.radio = LedgerRadio::Stopped;
    if ledger.refused != 1 {
        // The stop withdrew the MPDU, and the sink refused its terminal.
        return Err(Violation::Conserved {
            step: 7,
            classes: oer_protocols_ieee80211_contracts_port::outstanding::class::UNCONSUMED,
        });
    }
    if ledger.arena == LedgerArena::Unsettled {
        // The stop confirmed with the aggregate settled.
        return Err(Violation::Conserved {
            step: 16,
            classes: oer_protocols_ieee80211_contracts_port::outstanding::class::ARENAS,
        });
    }
    ledger.check(harness, 7)?;
    // A retirement with the refused terminal kept names it, and changes
    // nothing.
    harness.end_tokens();
    ledger.check(harness, 8)?;
    let (port, _) = harness.parts();
    let stopped = match port.retire(stopped) {
        Err(ProofRefused::Refused(stopped, out))
            if out.classes()
                & oer_protocols_ieee80211_contracts_port::outstanding::class::UNCONSUMED
                != 0 =>
        {
            stopped
        }
        _ => {
            return Err(Violation::Conserved {
                step: 9,
                classes: 0,
            });
        }
    };
    ledger.check(harness, 9)?;
    // The kept terminal comes once after the stop; its owners go home.
    let (port, sink) = harness.parts();
    if port.take_unconsumed(&stopped, sink).is_err() {
        return Err(Violation::Lifecycle);
    }
    ledger.refused = 0;
    let _ended = harness.take_completed();
    harness.end_tokens();
    ledger.check(harness, 10)?;
    // The retirement, then the next installation with the network's unit
    // still lent across it.
    let (port, _) = harness.parts();
    let Ok(retired) = port.retire(stopped) else {
        return Err(Violation::Conserved {
            step: 11,
            classes: 0,
        });
    };
    let next = harness
        .reinstall(retired)
        .map_err(|_| Violation::Lifecycle)?;
    ledger.check(harness, 12)?;
    drop(lent);
    ledger.lent -= 1;
    ledger.check(harness, 13)?;
    harness.keep_stopped(next);
    Ok(())
}

fn fault_conservation<'id, H: DataPortHarness<'id>>(
    harness: &mut H,
    reach: crate::FaultReach,
) -> Result<(), Violation> {
    use oer_protocols_ieee80211_contracts_port::{FaultKept, Recovered, outstanding::class};
    let whole = reach == crate::FaultReach::Whole;
    let mut ledger = Ledger::default();
    start(harness)?;
    ledger.radio = LedgerRadio::Active;
    // A station MPDU on the air when the fault comes.
    harness.hold_air(crate::Hold::Held);
    let attempt = harness.station_mpdu().ok_or(Violation::Fixture)?;
    let now = harness.now();
    let (port, sink) = harness.parts();
    crate::answered(port.submit(attempt, now, sink), Violation::Unkept)?
        .map_err(|_| Violation::Refusal)?;
    ledger.in_flight += 1;
    let _served = service(harness)?;
    ledger.check(harness, 30)?;
    harness.fault(reach);
    // The service meets the fault: the port reports it, and keeps it.
    for _ in 0..PROGRESS_STEPS {
        let _served = service(harness);
        harness.settle();
        let (port, _) = harness.parts();
        if port.fault().is_some() {
            break;
        }
    }
    let (port, _) = harness.parts();
    if port.fault().is_none() {
        return Err(Violation::Fixture);
    }
    ledger.fault = LedgerFault::Held;
    ledger.in_flight -= harness.take_completed().min(ledger.in_flight);
    ledger.check(harness, 31)?;
    let (port, right) = harness.stop_parts().ok_or(Violation::Fixture)?;
    let before = port.outstanding();
    match port.recover_for_stop(right) {
        Ok(Recovered::TakenBack) if whole => ledger.fault = LedgerFault::Clear,
        // The kept fault comes with what it leaves out, as the port has it.
        Err(FaultKept { outstanding, .. }) if !whole && outstanding == before => {}
        _ => {
            return Err(Violation::Conserved {
                step: 32,
                classes: class::FAULT,
            });
        }
    }
    ledger.check(harness, 32)?;
    harness.hold_air(crate::Hold::Released);
    let mut stop = None;
    for _ in 0..PROGRESS_STEPS {
        let waker = harness.waker();
        let now = harness.now();
        let (port, right) = harness.stop_parts().ok_or(Violation::Fixture)?;
        match port.poll_stop(right, &mut Context::from_waker(&waker), now) {
            core::task::Poll::Ready(Ok(stopped)) => {
                stop = Some(stopped);
                break;
            }
            core::task::Poll::Ready(Err(_)) if !whole => break,
            core::task::Poll::Ready(Err(_)) => return Err(Violation::Lifecycle),
            core::task::Poll::Pending => {
                let _served = service(harness);
                harness.settle();
            }
        }
        ledger.in_flight -= harness.take_completed().min(ledger.in_flight);
        ledger.check(harness, 33)?;
    }
    ledger.in_flight -= harness.take_completed().min(ledger.in_flight);
    harness.end_tokens();
    if whole {
        // Every owner came home: the port retires.
        let stopped = stop.ok_or(Violation::Lifecycle)?;
        ledger.radio = LedgerRadio::Stopped;
        ledger.check(harness, 34)?;
        let (port, _) = harness.parts();
        if port.retire(stopped).is_err() {
            return Err(Violation::Conserved {
                step: 35,
                classes: 0,
            });
        }
        return Ok(());
    }
    // The kept fault holds the port: no retirement, and what is out is
    // the ledger's.
    ledger.radio = if stop.is_none() {
        LedgerRadio::Active
    } else {
        LedgerRadio::Stopped
    };
    ledger.check(harness, 34)?;
    if let Some(stopped) = stop {
        let (port, _) = harness.parts();
        match port.retire(stopped) {
            Err(ProofRefused::Refused(_, out)) if out.classes() & class::FAULT != 0 => {}
            _ => {
                return Err(Violation::Conserved {
                    step: 35,
                    classes: class::FAULT,
                });
            }
        }
    }
    Ok(())
}
