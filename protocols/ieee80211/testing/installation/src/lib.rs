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
//! retirement, room in the sink after a refused drain, and the retry time
//! of a failed stop (the next turn).
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
    CleanupRefused, DataPort, Outstanding, ProofRefused, Recovery, ReinstallRefused, Retired,
    RxPull, StartFailure, StartPoll, StartProof, StopCause, StopUnconfirmed, Stopped,
    SupportedChannel, TxFacts, Unconsumed, outstanding::class,
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
    Room,
    NetworkStopped,
    Retired(Retired<'id>),
    RetireRefused(Stopped<'id>, Outstanding<F>),
    Returned,
    Reinstalled(Stopped<'id>),
    ReinstallRefused(Retired<'id>, ReinstallRefused),
    Released,
    Stale,
    Retry,
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
    /// A failed stop waits for its retry time.
    retry: bool,
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
        self.state.retry = false;
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
        let kept = match port.take_unconsumed(stopped, sink) {
            Err(_) => return self.state.owe(Answer::Stale),
            Ok(Unconsumed::Kept) => true,
            Ok(Unconsumed::None) => false,
        };
        let cleaned = port.clean(stopped, sink);
        let _ended = self.harness.take_completed();
        let answer = match cleaned {
            Ok(Err(CleanupRefused::Stale)) => Answer::Stale,
            Ok(Ok(())) if !kept => Answer::Drained,
            _ => {
                self.state.drain_refused = true;
                Answer::DrainRefused
            }
        };
        self.state.owe(answer);
    }
    fn stop_network(&mut self) {
        self.state.owe(Answer::NetworkStopped);
    }
    fn retire(&mut self, stopped: Stopped<'id>) {
        let (port, _) = self.harness.parts();
        let answer = match port.retire(stopped) {
            Ok(retired) => Answer::Retired(retired),
            Err(ProofRefused::Refused(stopped, outstanding)) => {
                self.state.refused = Some(outstanding.classes());
                Answer::RetireRefused(stopped, outstanding)
            }
            Err(ProofRefused::Stale(_)) => Answer::Stale,
        };
        self.state.owe(answer);
    }
    fn reinstall(&mut self, retired: Retired<'id>, _epoch: u32) {
        // The harness keeps its own epochs, each new.
        let answer = match self.harness.reinstall(retired) {
            Ok(stopped) => Answer::Reinstalled(stopped),
            Err(ProofRefused::Refused(retired, why)) => Answer::ReinstallRefused(retired, why),
            Err(ProofRefused::Stale(_)) => Answer::Stale,
        };
        self.state.owe(answer);
    }
    fn release(&mut self, retired: Retired<'id>) {
        let (port, _) = self.harness.parts();
        let answer = match port.release(retired) {
            Ok(_) => Answer::Released,
            Err(_) => Answer::Stale,
        };
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
                retry: false,
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
                Answer::Retry => machine.step(Event::Retry, &mut carrier),
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
            match port.poll_start(&mut cx, proof, channel, now) {
                StartPoll::Started => state.owe(Answer::Started),
                StartPoll::Pending(starting) => state.starting = Some((starting.into(), channel)),
                StartPoll::Refused(stopped, why) => state.owe(Answer::StartRefused(stopped, why)),
                StartPoll::Unstopped { cause, stop } => {
                    state.owe(Answer::StartUnstopped(cause, stop));
                }
                StartPoll::Stale(_) => state.owe(Answer::Stale),
            }
        }
        if state.stopping {
            let (port, _) = harness.parts();
            match port.poll_stop(&mut cx, now) {
                Poll::Ready(Ok(stopped)) => {
                    state.stopping = false;
                    state.owe(Answer::Stopped(stopped));
                }
                Poll::Ready(Err(stop)) => match stop.cause {
                    StopCause::Released => {
                        state.stopping = false;
                        state.owe(Answer::StopReleased);
                    }
                    StopCause::Device(_) => {
                        // The stop failed: the next turn is its retry time.
                        state.stopping = false;
                        state.retry = true;
                        state.owe(Answer::StopUnconfirmed(stop));
                    }
                    StopCause::Deadline | StopCause::CoreEnabled if !state.late => {
                        state.late = true;
                        state.owe(Answer::StopUnconfirmed(stop));
                    }
                    StopCause::Deadline | StopCause::CoreEnabled => {}
                },
                Poll::Pending => {}
            }
        } else if core::mem::take(&mut state.retry) {
            state.owe(Answer::Retry);
        }
        // A fault, with the port's answer to what a stop can do about it.
        let (port, _) = harness.parts();
        if port.fault().is_none() {
            state.fault_reported = false;
        } else if !state.fault_reported {
            state.fault_reported = true;
            match port.recover_for_stop() {
                Recovery::Clear => {}
                Recovery::Recovered(_) => state.owe(Answer::FaultTakenBack),
                Recovery::Kept(fault, outstanding) => {
                    state.owe(Answer::FaultKept(fault, outstanding));
                }
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
