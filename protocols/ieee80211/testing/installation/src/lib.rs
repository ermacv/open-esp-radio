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
    CleanupRefused, DataPort, Outstanding, PortAnswer, PortTypes, ProofRefused, Recovery,
    ReinstallRefused, Retired, RxPull, StaleProof, StartFailure, StartPoll, StartProof, Starting,
    StopCause, StopUnconfirmed, Stopped, SupportedChannel, TxFacts, Unconsumed, outstanding::class,
};
use oer_protocols_ieee80211_core_installation::{
    Asked, Drain, DrainAnswer, End, EndTokens, Event, FaultAnswer, Machine, Network, Outbox, Phase,
    Reinstall, ReinstallAnswer, Release, ReleaseAnswer, Request, Retire, RetireAnswer, Start,
    StartAnswer, Status, Stop, StopAnswer, StopData, StopNetwork, Unreached, Waiting,
};
use oer_protocols_ieee80211_testing_conformance::data_port::{DataPortHarness, PROGRESS_STEPS};

/// The channel the machine starts the port on.
type Channel<'id, P> = SupportedChannel<<P as DataPort<'id>>::Caps>;
/// The port's fault.
type FaultOf<'id, P> = <P as DataPort<'id>>::Fault;

/// A start's answer, owned until it is fed.
pub(crate) enum StartOwned<'id, F> {
    Started,
    Refused(Stopped<'id>, StartFailure<F>),
    Unstopped(StartFailure<F>, StopUnconfirmed<F>),
    Stale,
}

/// What a stop's poll answered.
pub(crate) enum StopPolled<'id, F> {
    /// It goes on: no answer yet.
    Pending,
    /// It goes on and said so (`Deadline`, `CoreEnabled`).
    GoesOn(StopUnconfirmed<F>),
    /// Its final answer.
    Final(StopOwned<'id, F>),
}

/// A stop's final answer, owned until it is fed.
pub(crate) enum StopOwned<'id, F> {
    Stopped(Stopped<'id>),
    Failed(StopUnconfirmed<F>),
    Released,
}

/// A drain's answer, owned until it is fed.
pub(crate) enum DrainOwned<F> {
    Drained,
    Refused,
    Later,
    Faulted(F),
    Stale,
}

/// A retirement's answer, owned until it is fed.
pub(crate) enum RetireOwned<'id, F> {
    Retired(Retired<'id>),
    Refused(Stopped<'id>, Outstanding<F>),
    Stale,
}

/// A fault, with the port's answer, owned until it is fed.
pub(crate) enum FaultOwned<F> {
    TakenBack,
    Kept(F, Outstanding<F>),
}

/// One answer the pump owes the machine, with its ask's ticket.
enum Owed<'id, F> {
    Start(Asked<Start>, StartOwned<'id, F>),
    Stop(Asked<Stop>, StopOwned<'id, F>),
    StopGoesOn(StopUnconfirmed<F>),
    DataStopped(Asked<StopData>),
    Ready(Asked<EndTokens>),
    Drain(Asked<Drain>, DrainOwned<F>),
    NetworkStopped(Asked<StopNetwork>),
    Retire(Asked<Retire>, RetireOwned<'id, F>),
    Reinstall(Asked<Reinstall>, ReinstallAnswer<'id>),
    Release(Asked<Release>, ReleaseAnswer),
    Fault(FaultOwned<F>),
    Room,
    Returned,
    Again,
}

/// The model's name of each answer's event (the design's section 7).
pub(crate) trait ModelEvent {
    fn model_event(&self) -> &'static str;
}

impl<F> ModelEvent for StartOwned<'_, F> {
    fn model_event(&self) -> &'static str {
        match self {
            Self::Started => "start_ok",
            Self::Refused(..) => "start_refused",
            Self::Unstopped(..) => "start_unstopped",
            Self::Stale => "stale",
        }
    }
}

impl<F> ModelEvent for StopPolled<'_, F> {
    fn model_event(&self) -> &'static str {
        match self {
            Self::Pending => "none",
            Self::GoesOn(_) => "stop_late",
            Self::Final(StopOwned::Stopped(_)) => "stop_ok",
            Self::Final(StopOwned::Failed(_)) => "stop_failed",
            Self::Final(StopOwned::Released) => "stop_released",
        }
    }
}

impl<F> ModelEvent for DrainOwned<F> {
    fn model_event(&self) -> &'static str {
        match self {
            Self::Drained => "drained",
            Self::Refused => "drain_refused",
            Self::Later => "drain_later",
            Self::Faulted(_) => "drain_faulted",
            Self::Stale => "stale",
        }
    }
}

impl<F> ModelEvent for RetireOwned<'_, F> {
    fn model_event(&self) -> &'static str {
        match self {
            Self::Retired(_) => "retired",
            Self::Refused(_, outstanding) if outstanding.fault.is_some() => "retire_refused_fault",
            Self::Refused(..) => "retire_refused",
            Self::Stale => "stale",
        }
    }
}

impl ModelEvent for ReinstallAnswer<'_> {
    fn model_event(&self) -> &'static str {
        match self {
            Self::Reinstalled(_) => "reinstalled",
            Self::Refused(..) => "reinstall_refused",
            Self::Stale => "stale",
        }
    }
}

impl ModelEvent for ReleaseAnswer {
    fn model_event(&self) -> &'static str {
        match self {
            Self::Released => "released",
            Self::Stale => "stale",
        }
    }
}

impl<F> ModelEvent for FaultOwned<F> {
    fn model_event(&self) -> &'static str {
        match self {
            Self::TakenBack => "fault_recovered",
            Self::Kept(..) => "fault_kept",
        }
    }
}

/// What a start's poll answered: the start goes on with its proof, or its
/// answer.
pub(crate) enum StartPolled<'id, F> {
    Pending(Starting<'id>),
    Answer(StartOwned<'id, F>),
}

/// `poll_start`'s answer, as the machine's event.
pub(crate) fn start_answer<'id, F>(poll: StartPoll<'id, F>) -> StartPolled<'id, F> {
    StartPolled::Answer(match poll {
        StartPoll::Started => StartOwned::Started,
        StartPoll::Pending(starting) => return StartPolled::Pending(starting),
        StartPoll::Refused(stopped, why) => StartOwned::Refused(stopped, why),
        StartPoll::Unstopped { cause, stop } => StartOwned::Unstopped(cause, stop),
        StartPoll::Stale(_) => StartOwned::Stale,
    })
}

/// `poll_stop`'s answer, as the machine's event.
pub(crate) fn stop_answer<'id, F>(
    poll: Poll<Result<Stopped<'id>, StopUnconfirmed<F>>>,
) -> StopPolled<'id, F> {
    match poll {
        Poll::Pending => StopPolled::Pending,
        Poll::Ready(Ok(stopped)) => StopPolled::Final(StopOwned::Stopped(stopped)),
        Poll::Ready(Err(stop)) => match stop.cause {
            // The stop goes on: the ask stays in flight.
            StopCause::Deadline | StopCause::CoreEnabled => StopPolled::GoesOn(stop),
            StopCause::Device(_) => StopPolled::Final(StopOwned::Failed(stop)),
            StopCause::Released => StopPolled::Final(StopOwned::Released),
        },
    }
}

/// `take_unconsumed`'s answer: none when the drain goes on to the cleanup.
pub(crate) fn unconsumed_answer<F>(
    taken: Result<Unconsumed, StaleProof<()>>,
) -> Option<DrainOwned<F>> {
    match taken {
        Ok(Unconsumed::None) => None,
        Ok(Unconsumed::Kept) => Some(DrainOwned::Refused),
        Err(StaleProof { proof: () }) => Some(DrainOwned::Stale),
    }
}

/// `clean`'s answer, as the machine's event.
pub(crate) fn clean_answer<'id, T: PortTypes<'id>, F>(
    cleaned: PortAnswer<'id, T, (), Result<(), CleanupRefused<F>>>,
) -> DrainOwned<F> {
    match cleaned {
        Ok(cleaned) => cleanup_answer(cleaned),
        // An item the sink refused.
        Err(_) => DrainOwned::Refused,
    }
}

/// The cleanup's own answer, as the machine's event.
pub(crate) fn cleanup_answer<F>(cleaned: Result<(), CleanupRefused<F>>) -> DrainOwned<F> {
    match cleaned {
        Ok(()) => DrainOwned::Drained,
        Err(CleanupRefused::Stale) => DrainOwned::Stale,
        Err(CleanupRefused::Faulted(fault)) => DrainOwned::Faulted(fault),
        Err(CleanupRefused::Unavailable(_) | CleanupRefused::Held(_)) => DrainOwned::Later,
    }
}

/// `retire`'s answer, as the machine's event.
pub(crate) fn retire_answer<'id, F>(
    retired: Result<Retired<'id>, ProofRefused<Stopped<'id>, Outstanding<F>>>,
) -> RetireOwned<'id, F> {
    match retired {
        Ok(retired) => RetireOwned::Retired(retired),
        Err(ProofRefused::Refused(stopped, outstanding)) => {
            RetireOwned::Refused(stopped, outstanding)
        }
        Err(ProofRefused::Stale(_)) => RetireOwned::Stale,
    }
}

/// `reinstall`'s answer, as the machine's event.
pub(crate) fn reinstall_answer<'id>(
    reinstalled: Result<Stopped<'id>, ProofRefused<Retired<'id>, ReinstallRefused>>,
) -> ReinstallAnswer<'id> {
    match reinstalled {
        Ok(stopped) => ReinstallAnswer::Reinstalled(stopped),
        Err(ProofRefused::Refused(
            retired,
            why @ (ReinstallRefused::SameEpoch
            | ReinstallRefused::ChannelUnset
            | ReinstallRefused::Receipt
            | ReinstallRefused::Resources),
        )) => ReinstallAnswer::Refused(retired, why),
        Err(ProofRefused::Stale(_)) => ReinstallAnswer::Stale,
    }
}

/// `release`'s answer, as the machine's event.
pub(crate) fn release_answer<'id, R>(
    released: Result<R, StaleProof<Retired<'id>>>,
) -> ReleaseAnswer {
    match released {
        Ok(_) => ReleaseAnswer::Released,
        Err(StaleProof { proof: _ }) => ReleaseAnswer::Stale,
    }
}

/// `recover_for_stop`'s answer, as the machine's event: none without a
/// fault.
pub(crate) fn recovery_answer<'id, F>(recovery: Recovery<'id, F>) -> Option<FaultOwned<F>> {
    match recovery {
        Recovery::Clear => None,
        // The pump drops the recovery's proof: no call takes it.
        Recovery::Recovered(_) => Some(FaultOwned::TakenBack),
        Recovery::Kept(fault, outstanding) => Some(FaultOwned::Kept(fault, outstanding)),
    }
}

/// The answers a step's asks can owe at once, and more.
const OWED: usize = 8;

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
}

/// The reports a case keeps.
const LOG: usize = 32;

/// The pump's own state: what it owes the machine and what it polls.
struct State<'id, P: DataPort<'id>> {
    owed: [Option<Owed<'id, FaultOf<'id, P>>>; OWED],
    /// A start in progress, with its ticket, its proof and its channel.
    starting: Option<(Asked<Start>, StartProof<'id>, Channel<'id, P>)>,
    /// A stop in progress, with its ticket.
    stopping: Option<Asked<Stop>>,
    /// A stop that goes on was reported: not again until it ends.
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
    fn owe(&mut self, owed: Owed<'id, FaultOf<'id, P>>) {
        if let Some(place) = self.owed.iter_mut().find(|place| place.is_none()) {
            *place = Some(owed);
        }
    }

    fn next(&mut self) -> Option<Owed<'id, FaultOf<'id, P>>> {
        let owed = self.owed.first_mut()?.take()?;
        self.owed.rotate_left(1);
        Some(owed)
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
    fn start(
        &mut self,
        asked: Asked<Start>,
        stopped: Stopped<'id>,
        channel: Channel<'id, H::Port>,
    ) {
        self.state.starting = Some((asked, stopped.into(), channel));
    }
    fn cancel_start(&mut self) -> Asked<Start> {
        // The start's ticket is with its poll, or with its answer owed.
        if let Some((asked, _proof, _channel)) = self.state.starting.take() {
            return asked;
        }
        let at = self
            .state
            .owed
            .iter()
            .position(|owed| matches!(owed, Some(Owed::Start(..))))
            .expect("a start in flight has its ticket with the pump");
        let Some(Owed::Start(asked, _)) = self.state.owed[at].take() else {
            unreachable!("found above");
        };
        self.state.owed[at..].rotate_left(1);
        asked
    }
    fn stop(&mut self, asked: Asked<Stop>) {
        self.state.stopping = Some(asked);
        self.state.late = false;
    }
    fn stop_data(&mut self, asked: Asked<StopData>) {
        self.state.owe(Owed::DataStopped(asked));
    }
    fn end_tokens(&mut self, asked: Asked<EndTokens>) {
        self.harness.end_tokens();
        self.state.owe(Owed::Ready(asked));
    }
    fn drain(&mut self, asked: Asked<Drain>, stopped: &Stopped<'id>) {
        let (port, sink) = self.harness.parts();
        let answer = match unconsumed_answer(port.take_unconsumed(stopped, sink)) {
            Some(answer) => answer,
            None => clean_answer(port.clean(stopped, sink)),
        };
        let _ended = self.harness.take_completed();
        match answer {
            DrainOwned::Refused => self.state.drain_refused = true,
            DrainOwned::Later => self.state.again = true,
            DrainOwned::Drained | DrainOwned::Faulted(_) | DrainOwned::Stale => {}
        }
        self.state.owe(Owed::Drain(asked, answer));
    }
    fn stop_network(&mut self, asked: Asked<StopNetwork>) {
        self.state.owe(Owed::NetworkStopped(asked));
    }
    fn retire(&mut self, asked: Asked<Retire>, stopped: Stopped<'id>) {
        let (port, _) = self.harness.parts();
        let answer = retire_answer(port.retire(stopped));
        if let RetireOwned::Refused(_, outstanding) = &answer {
            self.state.refused = Some(outstanding.classes());
        }
        self.state.owe(Owed::Retire(asked, answer));
    }
    fn reinstall(&mut self, asked: Asked<Reinstall>, retired: Retired<'id>, _epoch: u32) {
        // The harness keeps its own epochs, each new.
        let answer = reinstall_answer(self.harness.reinstall(retired));
        self.state.owe(Owed::Reinstall(asked, answer));
    }
    fn release(&mut self, asked: Asked<Release>, retired: Retired<'id>) {
        let (port, _) = self.harness.parts();
        let answer = release_answer(port.release(retired));
        self.state.owe(Owed::Release(asked, answer));
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
                owed: [const { None }; OWED],
                starting: None,
                stopping: None,
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
        while let Some(owed) = self.state.next() {
            // A stop that goes on borrows the stop's ticket, which stays.
            let stopping = match owed {
                Owed::StopGoesOn(_) => self.state.stopping.take(),
                _ => None,
            };
            let mut carrier = Carrier {
                harness: &mut *harness,
                state: &mut self.state,
            };
            match owed {
                Owed::Start(asked, answer) => {
                    let stop;
                    let answer = match answer {
                        StartOwned::Started => StartAnswer::Started,
                        StartOwned::Refused(stopped, why) => StartAnswer::Refused(stopped, why),
                        StartOwned::Unstopped(cause, unconfirmed) => {
                            stop = unconfirmed;
                            StartAnswer::Unstopped { cause, stop: &stop }
                        }
                        StartOwned::Stale => StartAnswer::Stale,
                    };
                    machine.step(Event::Start(asked, answer), &mut carrier);
                }
                Owed::Stop(asked, answer) => {
                    let failed;
                    let answer = match answer {
                        StopOwned::Stopped(stopped) => StopAnswer::Stopped(stopped),
                        StopOwned::Failed(stop) => {
                            failed = stop;
                            StopAnswer::Failed(&failed)
                        }
                        StopOwned::Released => StopAnswer::Released,
                    };
                    machine.step(Event::Stop(asked, answer), &mut carrier);
                }
                Owed::StopGoesOn(stop) => {
                    if let Some(asked) = &stopping {
                        machine.step(Event::StopGoesOn(asked, &stop), &mut carrier);
                    }
                }
                Owed::DataStopped(asked) => machine.step(Event::DataStopped(asked), &mut carrier),
                Owed::Ready(asked) => machine.step(Event::Ready(asked), &mut carrier),
                Owed::Drain(asked, answer) => {
                    let fault;
                    let answer = match answer {
                        DrainOwned::Drained => DrainAnswer::Drained,
                        DrainOwned::Refused => DrainAnswer::Refused,
                        DrainOwned::Later => DrainAnswer::Later,
                        DrainOwned::Faulted(faulted) => {
                            fault = faulted;
                            DrainAnswer::Faulted(&fault)
                        }
                        DrainOwned::Stale => DrainAnswer::Stale,
                    };
                    machine.step(Event::Drain(asked, answer), &mut carrier);
                }
                Owed::NetworkStopped(asked) => {
                    machine.step(Event::NetworkStopped(asked), &mut carrier);
                }
                Owed::Retire(asked, answer) => {
                    let out;
                    let answer = match answer {
                        RetireOwned::Retired(retired) => RetireAnswer::Retired(retired),
                        RetireOwned::Refused(stopped, outstanding) => {
                            out = outstanding;
                            RetireAnswer::Refused(stopped, &out)
                        }
                        RetireOwned::Stale => RetireAnswer::Stale,
                    };
                    machine.step(Event::Retire(asked, answer), &mut carrier);
                }
                Owed::Reinstall(asked, answer) => {
                    machine.step(Event::Reinstall(asked, answer), &mut carrier);
                }
                Owed::Release(asked, answer) => {
                    machine.step(Event::Release(asked, answer), &mut carrier);
                }
                Owed::Fault(FaultOwned::TakenBack) => {
                    machine.step(Event::Fault(FaultAnswer::TakenBack), &mut carrier);
                }
                Owed::Fault(FaultOwned::Kept(fault, outstanding)) => machine.step(
                    Event::Fault(FaultAnswer::Kept {
                        fault: &fault,
                        outstanding: &outstanding,
                    }),
                    &mut carrier,
                ),
                Owed::Room => machine.step(Event::Room, &mut carrier),
                Owed::Returned => machine.step(Event::Returned, &mut carrier),
                Owed::Again => machine.step(Event::Again, &mut carrier),
            }
            if stopping.is_some() {
                self.state.stopping = stopping;
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
        if let Some((asked, proof, channel)) = state.starting.take() {
            let (port, _) = harness.parts();
            match start_answer(port.poll_start(&mut cx, proof, channel, now)) {
                StartPolled::Pending(starting) => {
                    state.starting = Some((asked, starting.into(), channel));
                }
                StartPolled::Answer(answer) => state.owe(Owed::Start(asked, answer)),
            }
        }
        if core::mem::take(&mut state.again) {
            state.owe(Owed::Again);
        }
        if let Some(asked) = state.stopping.take() {
            let (port, _) = harness.parts();
            match stop_answer(port.poll_stop(&mut cx, now)) {
                StopPolled::Pending => state.stopping = Some(asked),
                StopPolled::GoesOn(stop) => {
                    state.stopping = Some(asked);
                    if !state.late {
                        state.late = true;
                        state.owe(Owed::StopGoesOn(stop));
                    }
                }
                StopPolled::Final(answer) => {
                    if let StopOwned::Failed(_) = answer {
                        // The next turn is the pump's next round.
                        state.again = true;
                    }
                    state.owe(Owed::Stop(asked, answer));
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
                state.owe(Owed::Fault(answer));
            }
        }
        // What a refused retirement named came home: fewer classes out.
        if let Some(classes) = state.refused
            && port.outstanding().classes() & classes != classes
        {
            state.refused = None;
            state.owe(Owed::Returned);
        }
        if state.drain_refused && harness.refusals_left() == 0 {
            state.drain_refused = false;
            state.owe(Owed::Room);
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
    fn name<A: ModelEvent>(answer: Option<A>) -> &'static str {
        answer.map_or("none", |answer| answer.model_event())
    }

    #[test]
    fn every_answer_of_the_contract_has_the_models_event() {
        let started = |poll| match start_answer::<F>(poll) {
            StartPolled::Pending(_) => "none",
            StartPolled::Answer(answer) => answer.model_event(),
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

        assert_eq!(stop_answer::<F>(Poll::Pending).model_event(), "none");
        assert_eq!(
            stop_answer::<F>(Poll::Ready(Ok(stopped()))).model_event(),
            "stop_ok"
        );
        for (cause, event) in [
            (StopCause::Deadline, "stop_late"),
            (StopCause::CoreEnabled, "stop_late"),
            (StopCause::Device(()), "stop_failed"),
            (StopCause::Released, "stop_released"),
        ] {
            assert_eq!(
                stop_answer(Poll::Ready(Err(stop(cause)))).model_event(),
                event
            );
        }

        assert_eq!(name(unconsumed_answer::<F>(Ok(Unconsumed::None))), "none");
        assert_eq!(
            name(unconsumed_answer::<F>(Ok(Unconsumed::Kept))),
            "drain_refused"
        );
        assert_eq!(
            name(unconsumed_answer::<F>(Err(StaleProof { proof: () }))),
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

        assert_eq!(reinstall_answer(Ok(stopped())).model_event(), "reinstalled");
        for why in [
            ReinstallRefused::SameEpoch,
            ReinstallRefused::ChannelUnset,
            ReinstallRefused::Receipt,
            ReinstallRefused::Resources,
        ] {
            assert_eq!(
                reinstall_answer(Err(ProofRefused::Refused(retired(), why))).model_event(),
                "reinstall_refused"
            );
        }
        assert_eq!(
            reinstall_answer(Err(ProofRefused::Stale(StaleProof { proof: retired() })))
                .model_event(),
            "stale"
        );

        assert_eq!(release_answer::<()>(Ok(())).model_event(), "released");
        assert_eq!(
            release_answer::<()>(Err(StaleProof { proof: retired() })).model_event(),
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
