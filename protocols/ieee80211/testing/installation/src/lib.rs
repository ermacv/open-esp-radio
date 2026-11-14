#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![allow(
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros,
    clippy::disallowed_methods,
    clippy::panic,
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::unreachable
)]
//! The installation machine over a real port: a small pump that carries
//! the machine's asks out on a [`DataPort`] through its harness and feeds
//! the port's answers back, and the cases every port passes under it.
//!
//! The pump knows no role. Each ask goes to the port at once (a drain, a
//! retirement, a reinstall, a release), or starts a poll the pump goes on
//! with at every turn (a start, a stop); the data plane's stop and the
//! network's are answered at once, as no data plane or network runs here.
//! At every turn the pump also reports what only it sees: a fault of the
//! port with the port's answer, what came home after a refused
//! retirement, room in the sink after a refused drain, and its next round
//! after an answer that said "later" (a failed stop, a cleanup that could
//! not run).
//!
//! Each answer of the port maps to the machine's event by a `match` with
//! no wildcard arm, one function a call (the design's section 7): a new
//! variant of an answer does not compile until it has its event.
//!
//! - From a running port with traffic, a restart ends reinstalled and the
//!   port runs again ([`Case::Restart`]).
//! - A release ends released ([`Case::Release`]).
//! - A retirement refused for a unit out reports `held` with its class and
//!   ends once the unit comes home ([`Case::RetireHeld`]).
//! - A stop that fails once is asked again and the goal is reached
//!   ([`Case::StopFailsOnce`]).

use core::mem::MaybeUninit;
use core::task::{Context, Poll};

use oer_protocols_ieee80211_contracts_port::{
    CleanupRefused, DataPort, Outstanding, PortAnswer, ProofRefused, Recovery, ReinstallRefused,
    Retired, RxPull, StaleProof, StartFailure, StartPoll, StartProof, Starting, StopCause,
    StopUnconfirmed, Stopped, SupportedChannel, TxFacts, Unconsumed, outstanding::class,
};
use oer_protocols_ieee80211_core_installation::{
    End, Event, FaultAnswer, Machine, Network, Outbox, Phase, Request, Status, Unreached, Waiting,
};

use oer_protocols_ieee80211_testing_conformance::data_port::{DataPortHarness, PROGRESS_STEPS};

/// The channel the machine starts the port on.
type Channel<'id, P> = SupportedChannel<<P as DataPort<'id>>::Caps>;
/// The port's fault.
type FaultOf<'id, P> = <P as DataPort<'id>>::Fault;

/// One answer the pump owes the machine, owned until it is fed.
enum Answer<'id, F> {
    Started,
    StartRefused(Stopped<'id>, StartFailure<F>),
    StartUnstopped(StartFailure<F>, StopUnconfirmed<F>),
    Stopped(Stopped<'id>),
    StopUnconfirmed(StopUnconfirmed<F>),
    StopReleased,
    FaultTakenBack,
    FaultKept(F, Outstanding<F>),
    DataStopped,
    Ready,
    Drained,
    DrainRefused,
    DrainLater,
    DrainFaulted(F),
    Room,
    NetworkStopped,
    Retired(Retired<'id>),
    RetireRefused(Stopped<'id>, Outstanding<F>),
    Returned,
    Reinstalled(Stopped<'id>),
    ReinstallRefused(Retired<'id>, ReinstallRefused),
    Released,
    Stale,
    Again,
}

impl<F> Answer<'_, F> {
    /// The model's name of the event (the design's section 7).
    pub(crate) fn model_event(&self) -> &'static str {
        match self {
            Self::Started => "start_ok",
            Self::StartRefused(..) => "start_refused",
            Self::StartUnstopped(..) => "start_unstopped",
            Self::Stopped(_) => "stop_ok",
            Self::StopUnconfirmed(stop) => match stop.cause {
                StopCause::Deadline | StopCause::CoreEnabled => "stop_late",
                StopCause::Device(_) => "stop_failed",
                StopCause::Released => "stop_released",
            },
            Self::StopReleased => "stop_released",
            Self::FaultTakenBack => "fault_recovered",
            Self::FaultKept(..) => "fault_kept",
            Self::DataStopped => "data_stopped",
            Self::Ready => "ready",
            Self::Drained => "drained",
            Self::DrainRefused => "drain_refused",
            Self::DrainLater => "drain_later",
            Self::DrainFaulted(_) => "drain_faulted",
            Self::Room => "room",
            Self::NetworkStopped => "net_stopped",
            Self::Retired(_) => "retired",
            Self::RetireRefused(_, outstanding) if outstanding.fault.is_some() => {
                "retire_refused_fault"
            }
            Self::RetireRefused(..) => "retire_refused",
            Self::Returned => "returned",
            Self::Reinstalled(_) => "reinstalled",
            Self::ReinstallRefused(..) => "reinstall_refused",
            Self::Released => "released",
            Self::Stale => "stale",
            Self::Again => "again",
        }
    }
}

/// What a start's poll answered: the start goes on with its proof, or an
/// answer.
pub(crate) enum StartAnswer<'id, F> {
    Pending(Starting<'id>),
    Answer(Answer<'id, F>),
}

/// `poll_start`'s answer, as the machine's event.
pub(crate) fn start_answer<'id, F>(poll: StartPoll<'id, F>) -> StartAnswer<'id, F> {
    StartAnswer::Answer(match poll {
        StartPoll::Started => Answer::Started,
        StartPoll::Pending(starting) => return StartAnswer::Pending(starting),
        StartPoll::Refused(stopped, why) => Answer::StartRefused(stopped, why),
        StartPoll::Unstopped { cause, stop } => Answer::StartUnstopped(cause, stop),
        StartPoll::Stale(_) => Answer::Stale,
    })
}

/// `poll_stop`'s answer, as the machine's event: none while it is pending.
pub(crate) fn stop_answer<'id, F>(
    poll: Poll<Result<Stopped<'id>, StopUnconfirmed<F>>>,
) -> Option<Answer<'id, F>> {
    match poll {
        Poll::Pending => None,
        Poll::Ready(Ok(stopped)) => Some(Answer::Stopped(stopped)),
        Poll::Ready(Err(stop)) => Some(match stop.cause {
            // The stop goes on: the ask stays in flight.
            StopCause::Deadline | StopCause::CoreEnabled | StopCause::Device(_) => {
                Answer::StopUnconfirmed(stop)
            }
            StopCause::Released => Answer::StopReleased,
        }),
    }
}

/// `take_unconsumed`'s answer: none when the drain goes on to the cleanup.
pub(crate) fn unconsumed_answer<'id, F>(
    taken: Result<Unconsumed, StaleProof<()>>,
) -> Option<Answer<'id, F>> {
    match taken {
        Ok(Unconsumed::None) => None,
        Ok(Unconsumed::Kept) => Some(Answer::DrainRefused),
        Err(StaleProof { proof: () }) => Some(Answer::Stale),
    }
}

/// `clean`'s answer, as the machine's event.
pub(crate) fn clean_answer<'id, T, F>(
    cleaned: PortAnswer<'id, T, (), Result<(), CleanupRefused<F>>>,
) -> Answer<'id, F>
where
    T: oer_protocols_ieee80211_contracts_port::PortTypes<'id>,
{
    match cleaned {
        Ok(cleaned) => cleanup_answer(cleaned),
        // An item the sink refused.
        Err(_) => Answer::DrainRefused,
    }
}

/// The cleanup's own answer, as the machine's event.
pub(crate) fn cleanup_answer<'id, F>(cleaned: Result<(), CleanupRefused<F>>) -> Answer<'id, F> {
    match cleaned {
        Ok(()) => Answer::Drained,
        Err(CleanupRefused::Stale) => Answer::Stale,
        Err(CleanupRefused::Faulted(fault)) => Answer::DrainFaulted(fault),
        Err(CleanupRefused::Unavailable(_) | CleanupRefused::Held(_)) => Answer::DrainLater,
    }
}

/// `retire`'s answer, as the machine's event.
pub(crate) fn retire_answer<'id, F>(
    retired: Result<Retired<'id>, ProofRefused<Stopped<'id>, Outstanding<F>>>,
) -> Answer<'id, F> {
    match retired {
        Ok(retired) => Answer::Retired(retired),
        Err(ProofRefused::Refused(stopped, outstanding)) => {
            Answer::RetireRefused(stopped, outstanding)
        }
        Err(ProofRefused::Stale(_)) => Answer::Stale,
    }
}

/// `reinstall`'s answer, as the machine's event.
pub(crate) fn reinstall_answer<'id, F>(
    reinstalled: Result<Stopped<'id>, ProofRefused<Retired<'id>, ReinstallRefused>>,
) -> Answer<'id, F> {
    match reinstalled {
        Ok(stopped) => Answer::Reinstalled(stopped),
        Err(ProofRefused::Refused(
            retired,
            why @ (ReinstallRefused::SameEpoch
            | ReinstallRefused::ChannelUnset
            | ReinstallRefused::Receipt
            | ReinstallRefused::Resources),
        )) => Answer::ReinstallRefused(retired, why),
        Err(ProofRefused::Stale(_)) => Answer::Stale,
    }
}

/// `release`'s answer, as the machine's event.
pub(crate) fn release_answer<'id, R, F>(
    released: Result<R, StaleProof<Retired<'id>>>,
) -> Answer<'id, F> {
    match released {
        Ok(_) => Answer::Released,
        Err(StaleProof { proof: _ }) => Answer::Stale,
    }
}

/// `recover_for_stop`'s answer, as the machine's event: none without a
/// fault.
pub(crate) fn recovery_answer<'id, F>(recovery: Recovery<'id, F>) -> Option<Answer<'id, F>> {
    match recovery {
        Recovery::Clear => None,
        // The pump drops the recovery's proof: no call takes it.
        Recovery::Recovered(_) => Some(Answer::FaultTakenBack),
        Recovery::Kept(fault, outstanding) => Some(Answer::FaultKept(fault, outstanding)),
    }
}

/// The answers a step's asks can owe at once, and more.
const ANSWERS: usize = 8;

/// A report of the machine, by kind, as the cases read the log.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Reported {
    /// A request refused.
    Refused,
    /// A request superseded.
    Superseded,
    /// The radio runs.
    Started,
    /// A start ended without the radio running.
    StartRefused,
    /// Halted.
    Halted,
    /// Reinstalled.
    Reinstalled,
    /// A restart refused.
    RestartRefused,
    /// Released.
    Released,
    /// A request ended by the port's release.
    EndedByRelease,
    /// A run interrupted by a fault.
    Interrupted,
    /// Held: the radio's stop.
    HeldRadio,
    /// Held: a fault.
    HeldFault,
    /// Held: the unconsumed items.
    HeldUnconsumed,
    /// Held: the cleanup, asked again at the next round.
    HeldCleanup,
    /// Held: the cleanup, refused by the port's fault.
    HeldCleanupFault,
    /// Held: what is out, with its classes.
    HeldOutstanding(u32),
    /// Held: a stale proof.
    HeldStale,
    /// Held: a deadline.
    HeldDeadline,
    /// An event the machine had no transition for.
    NotExpected,
}

/// The reports a case keeps.
const LOG: usize = 32;

/// The pump's own state: what it owes the machine and what it polls.
struct State<'id, P: DataPort<'id>> {
    answers: [Option<Answer<'id, FaultOf<'id, P>>>; ANSWERS],
    /// A start in progress, with its proof and channel.
    starting: Option<(StartProof<'id>, Channel<'id, P>)>,
    /// A stop in progress.
    stopping: bool,
    /// A late stop was reported: not again until it ends.
    late: bool,
    /// An answer said "later": the next turn is the pump's next round.
    again: bool,
    /// A drain was refused: room is reported once the sink has it.
    drain_refused: bool,
    /// The classes a refused retirement named.
    refused: Option<u32>,
    /// The fault the pump reported last, while the port keeps one.
    fault_reported: bool,
    log: [Option<Reported>; LOG],
    logged: usize,
}

impl<'id, P: DataPort<'id>> State<'id, P> {
    fn owe(&mut self, answer: Answer<'id, FaultOf<'id, P>>) {
        if let Some(place) = self.answers.iter_mut().find(|place| place.is_none()) {
            *place = Some(answer);
        }
    }

    fn next(&mut self) -> Option<Answer<'id, FaultOf<'id, P>>> {
        let answer = self.answers.first_mut()?.take()?;
        self.answers.rotate_left(1);
        Some(answer)
    }

    fn log(&mut self, report: Reported) {
        if let Some(place) = self.log.get_mut(self.logged) {
            *place = Some(report);
        }
        self.logged = self.logged.saturating_add(1);
    }
}

/// The outbox a step writes into: the harness for the asks that go to
/// the port at once, the pump's state for what it polls and owes.
struct Carrier<'a, 'id, H: DataPortHarness<'id>> {
    harness: &'a mut H,
    state: &'a mut State<'id, H::Port>,
}

impl<'id, H: DataPortHarness<'id>> Outbox<'id, Channel<'id, H::Port>, FaultOf<'id, H::Port>>
    for Carrier<'_, 'id, H>
{
    fn start(&mut self, stopped: Stopped<'id>, channel: Channel<'id, H::Port>) {
        self.state.starting = Some((stopped.into(), channel));
    }
    fn stop(&mut self) {
        // A stop ends a start in progress: its proof goes.
        self.state.starting = None;
        self.state.stopping = true;
        self.state.late = false;
    }
    fn stop_data(&mut self) {
        self.state.owe(Answer::DataStopped);
    }
    fn end_tokens(&mut self) {
        self.harness.end_tokens();
        self.state.owe(Answer::Ready);
    }
    fn drain(&mut self, stopped: &Stopped<'id>) {
        let (port, sink) = self.harness.parts();
        let answer = match unconsumed_answer(port.take_unconsumed(stopped, sink)) {
            Some(answer) => answer,
            None => clean_answer(port.clean(stopped, sink)),
        };
        let _ended = self.harness.take_completed();
        match answer {
            Answer::DrainRefused => self.state.drain_refused = true,
            Answer::DrainLater => self.state.again = true,
            _ => {}
        }
        self.state.owe(answer);
    }
    fn stop_network(&mut self) {
        self.state.owe(Answer::NetworkStopped);
    }
    fn retire(&mut self, stopped: Stopped<'id>) {
        let (port, _) = self.harness.parts();
        let answer = retire_answer(port.retire(stopped));
        if let Answer::RetireRefused(_, outstanding) = &answer {
            self.state.refused = Some(outstanding.classes());
        }
        self.state.owe(answer);
    }
    fn reinstall(&mut self, retired: Retired<'id>, _epoch: u32) {
        // The harness keeps its own epochs, each new.
        let answer = reinstall_answer(self.harness.reinstall(retired));
        self.state.owe(answer);
    }
    fn release(&mut self, retired: Retired<'id>) {
        let (port, _) = self.harness.parts();
        let answer = release_answer(port.release(retired));
        self.state.owe(answer);
    }
    fn refused(&mut self, _: Request<Channel<'id, H::Port>>, _: Phase) {
        self.state.log(Reported::Refused);
    }
    fn ended(
        &mut self,
        end: Result<End, Unreached<'_, Channel<'id, H::Port>, FaultOf<'id, H::Port>>>,
    ) {
        self.state.log(match end {
            Ok(End::Started) => Reported::Started,
            Ok(End::Halted) => Reported::Halted,
            Ok(End::Reinstalled) => Reported::Reinstalled,
            Ok(End::Released) => Reported::Released,
            Err(Unreached::StartRefused(_)) => Reported::StartRefused,
            Err(Unreached::RestartRefused(_)) => Reported::RestartRefused,
            Err(Unreached::Superseded(_)) => Reported::Superseded,
            Err(Unreached::Released) => Reported::EndedByRelease,
        });
    }
    fn status(&mut self, status: Status<'_, FaultOf<'id, H::Port>>) {
        self.state.log(match status {
            Status::Interrupted => Reported::Interrupted,
            Status::Held(Waiting::Radio(_)) => Reported::HeldRadio,
            Status::Held(Waiting::Fault { .. }) => Reported::HeldFault,
            Status::Held(Waiting::Unconsumed) => Reported::HeldUnconsumed,
            Status::Held(Waiting::Cleanup) => Reported::HeldCleanup,
            Status::Held(Waiting::CleanupFault(_)) => Reported::HeldCleanupFault,
            Status::Held(Waiting::Outstanding(outstanding)) => {
                Reported::HeldOutstanding(outstanding.classes())
            }
            Status::Held(Waiting::Stale) => Reported::HeldStale,
            Status::Held(Waiting::Deadline(_)) => Reported::HeldDeadline,
            Status::NotExpected { .. } => Reported::NotExpected,
        });
    }
    fn deadline(&mut self, _at: Option<oer_portable_time::Instant>) {}
}

/// The machine and its pump over one harness's port.
pub struct Pump<'id, P: DataPort<'id>> {
    state: State<'id, P>,
}

impl<'id, P: DataPort<'id>> Default for Pump<'id, P> {
    fn default() -> Self {
        Self {
            state: State {
                answers: [const { None }; ANSWERS],
                starting: None,
                stopping: false,
                late: false,
                again: false,
                drain_refused: false,
                refused: None,
                fault_reported: false,
                log: [None; LOG],
                logged: 0,
            },
        }
    }
}

impl<'id, P: DataPort<'id>> Pump<'id, P> {
    /// The reports so far, oldest first.
    pub fn reports(&self) -> impl Iterator<Item = Reported> + '_ {
        self.state.log.iter().flatten().copied()
    }

    /// Feed `request` to the machine.
    pub fn request<H: DataPortHarness<'id, Port = P>>(
        &mut self,
        machine: &mut Machine<'id, Channel<'id, P>, FaultOf<'id, P>>,
        harness: &mut H,
        request: Request<Channel<'id, P>>,
    ) {
        let mut carrier = Carrier {
            harness,
            state: &mut self.state,
        };
        machine.step(
            Event::Request {
                request,
                deadline: None,
            },
            &mut carrier,
        );
    }

    /// Feed every answer the pump owes, in order.
    fn feed<H: DataPortHarness<'id, Port = P>>(
        &mut self,
        machine: &mut Machine<'id, Channel<'id, P>, FaultOf<'id, P>>,
        harness: &mut H,
    ) {
        while let Some(answer) = self.state.next() {
            let mut carrier = Carrier {
                harness: &mut *harness,
                state: &mut self.state,
            };
            match answer {
                Answer::Started => machine.step(Event::Started, &mut carrier),
                Answer::StartRefused(stopped, why) => {
                    machine.step(Event::StartRefused(stopped, why), &mut carrier);
                }
                Answer::StartUnstopped(cause, stop) => {
                    machine.step(Event::StartUnstopped { cause, stop: &stop }, &mut carrier);
                }
                Answer::Stopped(stopped) => machine.step(Event::Stopped(stopped), &mut carrier),
                Answer::StopUnconfirmed(stop) => {
                    machine.step(Event::StopUnconfirmed(&stop), &mut carrier);
                }
                Answer::StopReleased => machine.step(Event::StopReleased, &mut carrier),
                Answer::FaultTakenBack => {
                    machine.step(Event::Fault(FaultAnswer::TakenBack), &mut carrier);
                }
                Answer::FaultKept(fault, outstanding) => machine.step(
                    Event::Fault(FaultAnswer::Kept {
                        fault: &fault,
                        outstanding: &outstanding,
                    }),
                    &mut carrier,
                ),
                Answer::DataStopped => machine.step(Event::DataStopped, &mut carrier),
                Answer::Ready => machine.step(Event::Ready, &mut carrier),
                Answer::Drained => machine.step(Event::Drained, &mut carrier),
                Answer::DrainRefused => machine.step(Event::DrainRefused, &mut carrier),
                Answer::DrainLater => machine.step(Event::DrainLater, &mut carrier),
                Answer::DrainFaulted(fault) => {
                    machine.step(Event::DrainFaulted(&fault), &mut carrier);
                }
                Answer::Room => machine.step(Event::Room, &mut carrier),
                Answer::NetworkStopped => machine.step(Event::NetworkStopped, &mut carrier),
                Answer::Retired(retired) => machine.step(Event::Retired(retired), &mut carrier),
                Answer::RetireRefused(stopped, outstanding) => {
                    machine.step(Event::RetireRefused(stopped, &outstanding), &mut carrier);
                }
                Answer::Returned => machine.step(Event::Returned, &mut carrier),
                Answer::Reinstalled(stopped) => {
                    machine.step(Event::Reinstalled(stopped), &mut carrier);
                }
                Answer::ReinstallRefused(retired, why) => {
                    machine.step(Event::ReinstallRefused(retired, why), &mut carrier);
                }
                Answer::Released => machine.step(Event::Released, &mut carrier),
                Answer::Stale => machine.step(Event::Stale, &mut carrier),
                Answer::Again => machine.step(Event::Again, &mut carrier),
            }
        }
    }

    /// One turn: the answers owed, the polls under way, and what only the
    /// pump sees.
    fn turn<H: DataPortHarness<'id, Port = P>>(
        &mut self,
        machine: &mut Machine<'id, Channel<'id, P>, FaultOf<'id, P>>,
        harness: &mut H,
    ) {
        self.feed(machine, harness);
        let waker = harness.waker();
        let mut cx = Context::from_waker(&waker);
        let now = harness.now();
        let state = &mut self.state;
        if let Some((proof, channel)) = state.starting.take() {
            let (port, _) = harness.parts();
            match start_answer(port.poll_start(&mut cx, proof, channel, now)) {
                StartAnswer::Pending(starting) => state.starting = Some((starting.into(), channel)),
                StartAnswer::Answer(answer) => state.owe(answer),
            }
        }
        if core::mem::take(&mut state.again) {
            state.owe(Answer::Again);
        }
        if state.stopping {
            let (port, _) = harness.parts();
            match stop_answer(port.poll_stop(&mut cx, now)) {
                None => {}
                Some(Answer::StopUnconfirmed(stop)) => match stop.cause {
                    // The stop failed: the next turn is the pump's next round.
                    StopCause::Device(_) | StopCause::Released => {
                        state.stopping = false;
                        state.again = true;
                        state.owe(Answer::StopUnconfirmed(stop));
                    }
                    StopCause::Deadline | StopCause::CoreEnabled if !state.late => {
                        state.late = true;
                        state.owe(Answer::StopUnconfirmed(stop));
                    }
                    StopCause::Deadline | StopCause::CoreEnabled => {}
                },
                Some(answer) => {
                    state.stopping = false;
                    state.owe(answer);
                }
            }
        }
        // A fault, with the port's answer to what a stop can do about it.
        let (port, _) = harness.parts();
        if port.fault().is_none() {
            state.fault_reported = false;
        } else if !state.fault_reported {
            state.fault_reported = true;
            if let Some(answer) = recovery_answer(port.recover_for_stop()) {
                state.owe(answer);
            }
        }
        // What a refused retirement named came home: fewer classes out.
        if let Some(classes) = state.refused
            && port.outstanding().classes() & classes != classes
        {
            state.refused = None;
            state.owe(Answer::Returned);
        }
        if state.drain_refused && harness.refusals_left() == 0 {
            state.drain_refused = false;
            state.owe(Answer::Room);
        }
        // The port's own service, and the fixture's progress.
        let (port, sink) = harness.parts();
        let facts = port.poll_facts(&mut cx);
        let _served = port.service_tx(facts.tx.unwrap_or(TxFacts::DUE), facts.tbtt, now, sink);
        harness.settle();
        let _ended = harness.take_completed();
        self.feed(machine, harness);
    }

    /// Turn until the machine reports `until` or the case's steps run out.
    fn until<H: DataPortHarness<'id, Port = P>>(
        &mut self,
        machine: &mut Machine<'id, Channel<'id, P>, FaultOf<'id, P>>,
        harness: &mut H,
        until: Reported,
    ) -> Result<(), Violation> {
        for _ in 0..PROGRESS_STEPS {
            if self.reports().any(|report| report == until) {
                return Ok(());
            }
            if self.reports().any(|report| report == Reported::NotExpected) {
                return Err(Violation::NotExpected);
            }
            self.turn(machine, harness);
        }
        Err(Violation::Progress(until))
    }
}

/// One case of the suite.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Case {
    /// From a running port with traffic, a restart ends reinstalled and the
    /// port runs again.
    Restart,
    /// A release ends released.
    Release,
    /// A retirement refused for a unit out reports `held` with its class
    /// and ends once the unit comes home.
    RetireHeld,
    /// A stop that fails once is asked again and the goal is reached.
    StopFailsOnce,
}

impl Case {
    /// Every case, in suite order.
    pub const ALL: [Self; 4] = [
        Self::Restart,
        Self::Release,
        Self::RetireHeld,
        Self::StopFailsOnce,
    ];
}

/// What a case found wrong.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Violation {
    /// The machine did not report this within the case's steps.
    Progress(Reported),
    /// The machine had no transition for an event the pump sent.
    NotExpected,
    /// The machine or the port ended elsewhere than the case expects.
    Ended(Phase),
    /// A held report did not name what the case holds.
    Held,
    /// The fixture could not set up the case.
    Fixture,
}

/// Run `case` on a fresh fixture, under a machine built on the fixture's
/// stop.
///
/// # Errors
///
/// What the case found wrong.
pub fn check<'id, H: DataPortHarness<'id>>(harness: &mut H, case: Case) -> Result<(), Violation> {
    let stopped = harness.take_stopped().ok_or(Violation::Fixture)?;
    let mut place = MaybeUninit::uninit();
    let machine = Machine::place(&mut place, stopped);
    let mut pump = Pump::<H::Port>::default();
    let channel = harness.channel();
    pump.request(machine, harness, Request::Start(channel));
    pump.until(machine, harness, Reported::Started)?;
    match case {
        Case::Restart => restart(&mut pump, machine, harness, channel),
        Case::Release => {
            pump.request(machine, harness, Request::Release(Network::Stays));
            pump.until(machine, harness, Reported::Released)?;
            ended(machine, Phase::Released)
        }
        Case::RetireHeld => retire_held(&mut pump, machine, harness),
        Case::StopFailsOnce => {
            harness.fail_next_stop();
            pump.request(machine, harness, Request::Halt);
            pump.until(machine, harness, Reported::HeldRadio)?;
            pump.until(machine, harness, Reported::Halted)?;
            ended(machine, Phase::Stopped)
        }
    }
}

fn ended<'id, C: Copy, F>(machine: &Machine<'id, C, F>, phase: Phase) -> Result<(), Violation> {
    if machine.phase() == phase {
        Ok(())
    } else {
        Err(Violation::Ended(machine.phase()))
    }
}

fn restart<'id, H: DataPortHarness<'id>>(
    pump: &mut Pump<'id, H::Port>,
    machine: &mut Machine<'id, Channel<'id, H::Port>, FaultOf<'id, H::Port>>,
    harness: &mut H,
    channel: Channel<'id, H::Port>,
) -> Result<(), Violation> {
    // Traffic: a station MPDU on the air, and received units the fixture
    // returns, when the restart comes.
    harness.hold_air(true);
    let attempt = harness.station_mpdu().ok_or(Violation::Fixture)?;
    let now = harness.now();
    let (port, sink) = harness.parts();
    port.submit(attempt, now, sink)
        .map_err(|_| Violation::Fixture)?
        .map_err(|_| Violation::Fixture)?;
    pump.turn(machine, harness);
    pump.request(machine, harness, Request::Restart { epoch: 2 });
    harness.hold_air(false);
    pump.until(machine, harness, Reported::Reinstalled)?;
    ended(machine, Phase::Stopped)?;
    // The new installation runs.
    pump.request(machine, harness, Request::Start(channel));
    let started = pump.reports().filter(|r| *r == Reported::Started).count();
    for _ in 0..PROGRESS_STEPS {
        if pump.reports().filter(|r| *r == Reported::Started).count() > started {
            return ended(machine, Phase::Running);
        }
        pump.turn(machine, harness);
    }
    Err(Violation::Progress(Reported::Started))
}

fn retire_held<'id, H: DataPortHarness<'id>>(
    pump: &mut Pump<'id, H::Port>,
    machine: &mut Machine<'id, Channel<'id, H::Port>, FaultOf<'id, H::Port>>,
    harness: &mut H,
) -> Result<(), Violation> {
    // A unit the fixture holds when the release comes.
    harness.deliver(1);
    let mut unit = None;
    for _ in 0..PROGRESS_STEPS {
        let now = harness.now();
        let (port, _) = harness.parts();
        if let Ok(RxPull::Unit { frame, .. }) = port.receive(1, now) {
            unit = Some(frame);
            break;
        }
        harness.settle();
    }
    let unit = unit.ok_or(Violation::Fixture)?;
    pump.request(machine, harness, Request::Release(Network::Stays));
    let held = |pump: &Pump<'id, H::Port>| {
        pump.reports().any(|report| {
            matches!(report, Reported::HeldOutstanding(classes) if classes & class::RECEIVED != 0)
        })
    };
    for _ in 0..PROGRESS_STEPS {
        if held(pump) {
            break;
        }
        pump.turn(machine, harness);
    }
    if !held(pump) {
        return Err(Violation::Held);
    }
    ended(machine, Phase::Settling)?;
    // The unit comes home: the retirement is asked again, and the release
    // ends.
    let (port, _) = harness.parts();
    if port.recycle(unit).is_err() {
        return Err(Violation::Fixture);
    }
    pump.until(machine, harness, Reported::Released)?;
    ended(machine, Phase::Released)
}

#[cfg(test)]
mod tests {
    //! The design's section 7: every variant of every answer of the
    //! contract beside the model's event, through the pump's mapping.

    use super::*;
    use oer_protocols_ieee80211_contracts_port::{ArenaState, RunPhase, StateSerial};

    type F = ();

    fn stopped() -> Stopped<'static> {
        Stopped::issue(StateSerial::FIRST)
    }
    fn retired() -> Retired<'static> {
        Retired::issue(StateSerial::FIRST)
    }
    fn stop(cause: StopCause<F>) -> StopUnconfirmed<F> {
        StopUnconfirmed {
            cause,
            outstanding: out(None),
        }
    }
    fn out(fault: Option<F>) -> Outstanding<F> {
        Outstanding {
            phase: RunPhase::Stopped,
            fault,
            received: 1,
            lent: 0,
            removals: 0,
            retained_groups: 0,
            unconsumed: 0,
            transmit_out: 0,
            queued: [0; oer_protocols_ieee80211_contracts_port::ACCESS_CATEGORIES],
            running: 0,
            arenas: ArenaState::Returnable,
        }
    }
    fn name(answer: Option<Answer<'static, F>>) -> &'static str {
        answer.map_or("none", |answer| answer.model_event())
    }

    #[test]
    fn every_answer_of_the_contract_has_the_models_event() {
        let started = |poll| match start_answer::<F>(poll) {
            StartAnswer::Pending(_) => "none",
            StartAnswer::Answer(answer) => answer.model_event(),
        };
        assert_eq!(started(StartPoll::Started), "start_ok");
        assert_eq!(
            started(StartPoll::Pending(Starting::issue(StateSerial::FIRST))),
            "none"
        );
        assert_eq!(
            started(StartPoll::Refused(stopped(), StartFailure::Device)),
            "start_refused"
        );
        assert_eq!(
            started(StartPoll::Unstopped {
                cause: StartFailure::Deadline,
                stop: stop(StopCause::CoreEnabled),
            }),
            "start_unstopped"
        );
        assert_eq!(
            started(StartPoll::Stale(StaleProof {
                proof: stopped().into(),
            })),
            "stale"
        );

        assert_eq!(name(stop_answer::<F>(Poll::Pending)), "none");
        assert_eq!(name(stop_answer(Poll::Ready(Ok(stopped())))), "stop_ok");
        for (cause, event) in [
            (StopCause::Deadline, "stop_late"),
            (StopCause::CoreEnabled, "stop_late"),
            (StopCause::Device(()), "stop_failed"),
            (StopCause::Released, "stop_released"),
        ] {
            assert_eq!(name(stop_answer(Poll::Ready(Err(stop(cause))))), event);
        }

        assert_eq!(name(unconsumed_answer(Ok(Unconsumed::None))), "none");
        assert_eq!(
            name(unconsumed_answer(Ok(Unconsumed::Kept))),
            "drain_refused"
        );
        assert_eq!(
            name(unconsumed_answer(Err(StaleProof { proof: () }))),
            "stale"
        );

        for (cleaned, event) in [
            (Ok(()), "drained"),
            (Err(CleanupRefused::Stale), "stale"),
            (Err(CleanupRefused::Faulted(())), "drain_faulted"),
            (Err(CleanupRefused::Unavailable(())), "drain_later"),
            (Err(CleanupRefused::Held(())), "drain_later"),
        ] {
            assert_eq!(cleanup_answer::<F>(cleaned).model_event(), event);
        }

        assert_eq!(retire_answer::<F>(Ok(retired())).model_event(), "retired");
        assert_eq!(
            retire_answer(Err(ProofRefused::Refused(stopped(), out(Some(()))))).model_event(),
            "retire_refused_fault"
        );
        assert_eq!(
            retire_answer(Err(ProofRefused::Refused(stopped(), out(None)))).model_event(),
            "retire_refused"
        );
        assert_eq!(
            retire_answer::<F>(Err(ProofRefused::Stale(StaleProof { proof: stopped() })))
                .model_event(),
            "stale"
        );

        assert_eq!(
            reinstall_answer::<F>(Ok(stopped())).model_event(),
            "reinstalled"
        );
        for why in [
            ReinstallRefused::SameEpoch,
            ReinstallRefused::ChannelUnset,
            ReinstallRefused::Receipt,
            ReinstallRefused::Resources,
        ] {
            assert_eq!(
                reinstall_answer::<F>(Err(ProofRefused::Refused(retired(), why))).model_event(),
                "reinstall_refused"
            );
        }
        assert_eq!(
            reinstall_answer::<F>(Err(ProofRefused::Stale(StaleProof { proof: retired() })))
                .model_event(),
            "stale"
        );

        assert_eq!(release_answer::<(), F>(Ok(())).model_event(), "released");
        assert_eq!(
            release_answer::<(), F>(Err(StaleProof { proof: retired() })).model_event(),
            "stale"
        );

        assert_eq!(name(recovery_answer::<F>(Recovery::Clear)), "none");
        assert_eq!(
            name(recovery_answer::<F>(Recovery::Recovered(
                oer_protocols_ieee80211_contracts_port::Recovering::issue(StateSerial::FIRST)
            ))),
            "fault_recovered"
        );
        assert_eq!(
            name(recovery_answer(Recovery::Kept((), out(Some(()))))),
            "fault_kept"
        );
    }
}
