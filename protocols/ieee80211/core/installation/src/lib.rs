#![no_std]
#![forbid(unsafe_code)]
#![deny(
    clippy::let_underscore_must_use,
    clippy::let_underscore_untyped,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented,
    clippy::disallowed_macros,
    clippy::disallowed_methods
)]
#![cfg_attr(
    test,
    allow(
        clippy::let_underscore_must_use,
        clippy::let_underscore_untyped,
        clippy::arithmetic_side_effects,
        clippy::indexing_slicing,
        clippy::panic,
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::unreachable,
        clippy::todo,
        clippy::unimplemented,
        clippy::disallowed_macros,
        clippy::disallowed_methods,
    )
)]

//! The installation machine: one sans-IO machine a port, which decides
//! every transition of the port's installation for every role.
//!
//! It touches no port, no frame and no clock. A step takes one [`Event`]
//! and writes what is to be done into the [`Outbox`] the pump lends: the
//! asks of the port, the data plane, the controller and the network, and
//! the request's answer in its three kinds: a refusal, the one final
//! outcome ([`End`] or [`Unreached`]), and the [`Status`] while it goes
//! on. The machine is the one holder
//! of the port's proofs: each phase that holds one holds it by type
//! ([`Phase`]), and an ask that takes a proof takes it from the machine.
//!
//! The machine is its design's executable model
//! (`design/installation-machine-model.py` of the notes), transition for
//! transition; the crate's tests walk the model's search over it and
//! compare its table with the model's (`tests/model-table.txt`).

use core::marker::PhantomData;

use oer_portable_time::Instant;
use oer_protocols_ieee80211_contracts_port::{
    Outstanding, ReinstallRefused, Retired, StartFailure, StopUnconfirmed, Stopped,
};

#[cfg(test)]
mod tests;

/// What a controller asks of the installation, `C` the channel it runs on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Request<C> {
    /// Start the stopped radio on the channel.
    Start(C),
    /// Move the running radio to the channel: a stop, then a start there.
    Retune(C),
    /// Stop and stay installed.
    Halt,
    /// Go to the next installation, at TSF epoch `epoch`; the network stays.
    Restart {
        /// The next installation's TSF epoch.
        epoch: u32,
    },
    /// End the port's installations.
    Release(Network),
}

/// What a release does to the network.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Network {
    /// It stays, for the port made again.
    Stays,
    /// It stops before the release is asked.
    Stops,
}

/// What the machine works toward, `C` the channel of a run.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Goal<C> {
    /// A start, or a retune's start, on the channel.
    Run(C),
    /// A halt.
    Halt,
    /// A restart at the epoch.
    Restart {
        /// The next installation's TSF epoch.
        epoch: u32,
    },
    /// A release.
    Release(Network),
}

impl<C> Goal<C> {
    /// The order of the endings: a stronger one takes a weaker one's place.
    const fn rank(&self) -> u8 {
        match self {
            Self::Run(_) => 0,
            Self::Halt => 1,
            Self::Restart { .. } => 2,
            Self::Release(Network::Stays) => 3,
            Self::Release(Network::Stops) => 4,
        }
    }

    /// Whether the goal retires the installation.
    const fn retires(&self) -> bool {
        matches!(self, Self::Restart { .. } | Self::Release(_))
    }
}

/// The machine's phase, as a report names it.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Phase {
    /// The radio is stopped: the machine holds the stop's proof.
    Stopped,
    /// A start is asked.
    Starting,
    /// The radio runs.
    Running,
    /// A stop is asked.
    Stopping,
    /// The radio stopped; the machine waits for the data, the tokens, the
    /// unconsumed items or what a retirement named, holding the stop's
    /// proof.
    Settling,
    /// A retirement is asked.
    Retiring,
    /// The installation retired: the machine holds the retirement's proof.
    Retired,
    /// A reinstall is asked.
    Reinstalling,
    /// A release is asked.
    Releasing,
    /// The port released its installations.
    Released,
}

/// How a start ended without the radio running.
#[derive(Debug, Eq, PartialEq)]
pub enum StartEnd<'a, F> {
    /// The port refused it; the radio is stopped.
    Refused(StartFailure<F>),
    /// It began and failed for `cause`, and the stop of what it began did
    /// not confirm: the machine asked the stop again.
    Unstopped {
        /// Why the start failed.
        cause: StartFailure<F>,
        /// What the stop of the begun start observed.
        stop: &'a StopUnconfirmed<F>,
    },
    /// The port did not take the proof.
    Stale,
    /// A fault ended it.
    Fault,
}

/// What the machine waits for, as a `held` report names it.
#[derive(Debug, Eq, PartialEq)]
pub enum Waiting<'a, F> {
    /// The radio: its stop did not confirm, with what the port has out.
    Radio(&'a StopUnconfirmed<F>),
    /// A fault the port keeps, with what it leaves out.
    Fault {
        /// The fault.
        fault: &'a F,
        /// What the port has out.
        outstanding: &'a Outstanding<F>,
    },
    /// The unconsumed items: a sink refused them, and room is waited for.
    Unconsumed,
    /// The cleanup: its next step could not run, or the hardware still
    /// holds what it releases; it is asked again at the pump's next round.
    Cleanup,
    /// The cleanup: the port holds its fault; it is asked again once the
    /// fault is taken back.
    CleanupFault(&'a F),
    /// What is out: the retirement's refusal.
    Outstanding(&'a Outstanding<F>),
    /// A proof the port did not take: a stop gives the next.
    Stale,
    /// The request's deadline passed in the phase.
    Deadline(Phase),
}

/// The port's answer to what a stop can do about its fault.
/// `Recovery::Clear` (no fault) is no event, and this type has no value
/// for it.
#[derive(Debug, Eq, PartialEq)]
pub enum FaultAnswer<'a, F> {
    /// The port took the fault back: a stop returns its owners.
    TakenBack,
    /// The port keeps it, with what it leaves out.
    Kept {
        /// The fault.
        fault: &'a F,
        /// What the port has out.
        outstanding: &'a Outstanding<F>,
    },
}

/// The ticket of one ask of kind `K`: handed to the pump with the ask, and
/// carried back by the ask's answer. It is affine and of no size, and only
/// the machine makes one, so an answer without its ask, and a second answer
/// to one ask, cannot be written.
#[derive(Debug)]
pub struct Asked<K>(PhantomData<fn() -> K>);

impl<K> Asked<K> {
    /// A ticket: the machine's alone.
    const fn new() -> Self {
        Self(PhantomData)
    }
}

/// The ask of a start.
#[derive(Debug)]
pub enum Start {}
/// The ask of a stop.
#[derive(Debug)]
pub enum Stop {}
/// The ask of the data plane's stop.
#[derive(Debug)]
pub enum StopData {}
/// The ask of the controller's tokens.
#[derive(Debug)]
pub enum EndTokens {}
/// The ask of a drain: what a sink refused, then the cleanup.
#[derive(Debug)]
pub enum Drain {}
/// The ask of the network's stop.
#[derive(Debug)]
pub enum StopNetwork {}
/// The ask of a retirement.
#[derive(Debug)]
pub enum Retire {}
/// The ask of a reinstall.
#[derive(Debug)]
pub enum Reinstall {}
/// The ask of a release.
#[derive(Debug)]
pub enum Release {}

/// The port's answer to a start (`poll_start`, the design's section 7).
#[derive(Debug)]
pub enum StartAnswer<'a, 'id, F> {
    /// The radio runs.
    Started,
    /// The port refused it: the stop's proof back, with why.
    Refused(Stopped<'id>, StartFailure<F>),
    /// It began and failed, and its stop did not confirm.
    Unstopped {
        /// Why it failed.
        cause: StartFailure<F>,
        /// What the stop of the begun start observed.
        stop: &'a StopUnconfirmed<F>,
    },
    /// The port did not take the proof.
    Stale,
}

/// The port's final answer to a stop (`poll_stop`); one that goes on is
/// [`Event::StopGoesOn`].
#[derive(Debug)]
pub enum StopAnswer<'a, 'id, F> {
    /// The stop confirmed.
    Stopped(Stopped<'id>),
    /// It failed for the device's fault: asked again at [`Event::Again`].
    Failed(&'a StopUnconfirmed<F>),
    /// The port holds no installation.
    Released,
}

/// The port's answer to a drain (`take_unconsumed`, then `clean`).
#[derive(Debug)]
pub enum DrainAnswer<'a, F> {
    /// Nothing is kept and the stopped port is clean.
    Drained,
    /// A sink refused an item: asked again at [`Event::Room`].
    Refused,
    /// The cleanup's next step could not run, or the hardware still holds
    /// what it releases (`Unavailable`, `Held`): asked again at
    /// [`Event::Again`].
    Later,
    /// The port holds its fault (`Faulted`): asked again once the fault is
    /// taken back.
    Faulted(&'a F),
    /// The port did not take the proof the drain borrowed.
    Stale,
}

/// The port's answer to a retirement.
#[derive(Debug)]
pub enum RetireAnswer<'a, 'id, F> {
    /// The retirement's proof.
    Retired(Retired<'id>),
    /// Refused: the stop's proof back, with what is out.
    Refused(Stopped<'id>, &'a Outstanding<F>),
    /// The port did not take the proof.
    Stale,
}

/// The port's answer to a reinstall.
#[derive(Debug)]
pub enum ReinstallAnswer<'id> {
    /// The next installation's stop.
    Reinstalled(Stopped<'id>),
    /// Refused: the retirement's proof back, with why.
    Refused(Retired<'id>, ReinstallRefused),
    /// The port did not take the proof.
    Stale,
}

/// The port's answer to a release.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReleaseAnswer {
    /// The port released its installations.
    Released,
    /// The port did not take the proof.
    Stale,
}

/// One event of the machine: a request, an answer with its ask's ticket,
/// a fault, or "look again". What is larger than two words comes by
/// reference.
#[derive(Debug)]
pub enum Event<'a, 'id, C, F> {
    /// A controller's request, with the deadline of its ending.
    Request {
        /// The request.
        request: Request<C>,
        /// When the machine reports `held` for want of progress.
        deadline: Option<Instant>,
    },
    /// The start's answer.
    Start(Asked<Start>, StartAnswer<'a, 'id, F>),
    /// The stop's final answer.
    Stop(Asked<Stop>, StopAnswer<'a, 'id, F>),
    /// The stop goes on (`Deadline`, `CoreEnabled`): it borrows the stop's
    /// ticket, which a final answer spends.
    StopGoesOn(&'a Asked<Stop>, &'a StopUnconfirmed<F>),
    /// The data plane stopped.
    DataStopped(Asked<StopData>),
    /// The controller ended its tokens.
    Ready(Asked<EndTokens>),
    /// The drain's answer.
    Drain(Asked<Drain>, DrainAnswer<'a, F>),
    /// The network stopped.
    NetworkStopped(Asked<StopNetwork>),
    /// The retirement's answer.
    Retire(Asked<Retire>, RetireAnswer<'a, 'id, F>),
    /// The reinstall's answer.
    Reinstall(Asked<Reinstall>, ReinstallAnswer<'id>),
    /// The release's answer.
    Release(Asked<Release>, ReleaseAnswer),
    /// A fault of the port, with what a stop can do about it.
    Fault(FaultAnswer<'a, F>),
    /// Look again: a sink that refused may have room.
    Room,
    /// Look again: something a refused retirement named may be home.
    Returned,
    /// Look again: the pump's next bounded round after an answer that said
    /// "later".
    Again,
    /// The request's deadline the outbox named passed.
    Deadline,
}

/// One event as the rules take it: the public [`Event`] with its ticket
/// spent.
#[derive(Debug)]
enum Ev<'a, 'id, C, F> {
    /// A controller's request, with the deadline of its ending.
    Request {
        /// The request.
        request: Request<C>,
        /// When the machine reports `held` for want of progress.
        deadline: Option<Instant>,
    },
    /// The start ran.
    Started,
    /// The port refused the start: the stop's proof back, with why.
    StartRefused(Stopped<'id>, StartFailure<F>),
    /// The start began and failed, and its stop did not confirm.
    StartUnstopped {
        /// Why the start failed.
        cause: StartFailure<F>,
        /// What the stop of the begun start observed.
        stop: &'a StopUnconfirmed<F>,
    },
    /// The stop confirmed.
    Stopped(Stopped<'id>),
    /// The stop did not confirm: it goes on, or it failed.
    StopUnconfirmed(&'a StopUnconfirmed<F>),
    /// The port answered the stop "released": it holds no installation.
    StopReleased,
    /// A fault of the port, with what a stop can do about it.
    Fault(FaultAnswer<'a, F>),
    /// The data plane stopped.
    DataStopped,
    /// The controller ended its tokens.
    Ready,
    /// The unconsumed items were taken and the stopped port cleaned.
    Drained,
    /// A sink refused an unconsumed item.
    DrainRefused,
    /// The cleanup's next step could not run, or the hardware still holds
    /// what it releases (`Unavailable`, `Held`): asked again at
    /// [`Event::Again`].
    DrainLater,
    /// The port holds its fault, so the cleanup wrote nothing (`Faulted`):
    /// asked again once the fault is taken back.
    DrainFaulted(&'a F),
    /// A sink that refused has room again.
    Room,
    /// The network stopped.
    NetworkStopped,
    /// The retirement's proof.
    Retired(Retired<'id>),
    /// The retirement was refused: the stop's proof back, with what is out.
    RetireRefused(Stopped<'id>, &'a Outstanding<F>),
    /// Something a refused retirement named came home.
    Returned,
    /// The next installation's stop.
    Reinstalled(Stopped<'id>),
    /// The reinstall was refused: the retirement's proof back, with why.
    ReinstallRefused(Retired<'id>, ReinstallRefused),
    /// The port released its installations.
    Released,
    /// The port did not take a proof the machine gave or lent.
    Stale,
    /// The request's deadline the outbox named passed.
    Deadline,
    /// The pump's next bounded round after an answer that said "later": a
    /// stop that failed, a cleanup that could not run.
    Again,
}

/// How an accepted request ended as asked.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum End {
    /// The radio runs.
    Started,
    /// The radio stopped and its data is home.
    Halted,
    /// The next installation is installed and stopped.
    Reinstalled,
    /// The port released its installations.
    Released,
}

/// How an accepted request ended otherwise.
#[derive(Debug, Eq, PartialEq)]
pub enum Unreached<'a, C, F> {
    /// The start ended without the radio running.
    StartRefused(StartEnd<'a, F>),
    /// The port refused the next installation; the machine holds the
    /// retired one.
    RestartRefused(ReinstallRefused),
    /// A stronger ending took its place.
    Superseded(Goal<C>),
    /// The port turned out released: a stop answered so.
    Released,
}

/// What is true now, while a request goes on: never an answer.
#[derive(Debug, Eq, PartialEq)]
pub enum Status<'a, F> {
    /// A fault ended the run: the controller says what it wants next.
    Interrupted,
    /// What the machine waits for.
    Held(Waiting<'a, F>),
}

/// What a step's rules write: one method an ask, and one for each kind of
/// report. The machine turns it into the [`Outbox`] with the asks' tickets.
trait Asks<'id, C, F> {
    /// Start the radio on `channel`, with the stop's proof.
    fn start(&mut self, stopped: Stopped<'id>, channel: C);
    /// Stop the radio.
    fn stop(&mut self);
    /// Stop the data plane.
    fn stop_data(&mut self);
    /// End the controller's tokens.
    fn end_tokens(&mut self);
    /// Take what a sink refused, then clean the stopped port.
    fn drain(&mut self, stopped: &Stopped<'id>);
    /// Stop the network.
    fn stop_network(&mut self);
    /// Retire the stopped port.
    fn retire(&mut self, stopped: Stopped<'id>);
    /// Install the next installation at `epoch`.
    fn reinstall(&mut self, retired: Retired<'id>, epoch: u32);
    /// Release the port's installations.
    fn release(&mut self, retired: Retired<'id>);
    /// A request was not accepted: nothing changed.
    fn refused(&mut self, request: Request<C>, phase: Phase);
    /// The one final answer of an accepted request.
    fn ended(&mut self, end: Result<End, Unreached<'_, C, F>>);
    /// What is true now, while a request goes on: never an answer.
    fn status(&mut self, status: Status<'_, F>);
    /// When the pump sends [`Event::Deadline`]; `None`, never.
    fn deadline(&mut self, at: Option<Instant>);
}

/// What the pump lends a step: one method an ask, each with its ticket, and
/// one for each kind of report: a refusal, an outcome, a status.
pub trait Outbox<'id, C, F> {
    /// Start the radio on `channel`, with the stop's proof.
    fn start(&mut self, asked: Asked<Start>, stopped: Stopped<'id>, channel: C);
    /// A stop ends the start in progress: its ticket back, so no answer of
    /// it is left to give.
    fn cancel_start(&mut self) -> Asked<Start>;
    /// Stop the radio.
    fn stop(&mut self, asked: Asked<Stop>);
    /// Stop the data plane.
    fn stop_data(&mut self, asked: Asked<StopData>);
    /// End the controller's tokens.
    fn end_tokens(&mut self, asked: Asked<EndTokens>);
    /// Take what a sink refused, then clean the stopped port.
    fn drain(&mut self, asked: Asked<Drain>, stopped: &Stopped<'id>);
    /// Stop the network.
    fn stop_network(&mut self, asked: Asked<StopNetwork>);
    /// Retire the stopped port.
    fn retire(&mut self, asked: Asked<Retire>, stopped: Stopped<'id>);
    /// Install the next installation at `epoch`.
    fn reinstall(&mut self, asked: Asked<Reinstall>, retired: Retired<'id>, epoch: u32);
    /// Release the port's installations.
    fn release(&mut self, asked: Asked<Release>, retired: Retired<'id>);
    /// A request was not accepted: nothing changed.
    fn refused(&mut self, request: Request<C>, phase: Phase);
    /// The one final answer of an accepted request.
    fn ended(&mut self, end: Result<End, Unreached<'_, C, F>>);
    /// What is true now, while a request goes on: never an answer.
    fn status(&mut self, status: Status<'_, F>);
    /// When the pump sends [`Event::Deadline`]; `None`, never.
    fn deadline(&mut self, at: Option<Instant>);
}

/// The phase with the proof it holds.
#[derive(Debug)]
enum State<'id> {
    Stopped(Stopped<'id>),
    Starting,
    Running,
    Stopping,
    Settling(Stopped<'id>),
    Retiring,
    Retired(Retired<'id>),
    Reinstalling,
    Releasing,
    Released,
}

impl State<'_> {
    const fn phase(&self) -> Phase {
        match self {
            Self::Stopped(_) => Phase::Stopped,
            Self::Starting => Phase::Starting,
            Self::Running => Phase::Running,
            Self::Stopping => Phase::Stopping,
            Self::Settling(_) => Phase::Settling,
            Self::Retiring => Phase::Retiring,
            Self::Retired(_) => Phase::Retired,
            Self::Reinstalling => Phase::Reinstalling,
            Self::Releasing => Phase::Releasing,
            Self::Released => Phase::Released,
        }
    }
}

/// The data plane, as the machine asked it.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum Data {
    Stopped,
    Running,
    Stopping,
}

/// The network's stop, as the machine asked it.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum Net {
    Idle,
    Stopping,
    Stopped,
}

/// What a retirement still waits for.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
struct Waits {
    /// The controller's tokens.
    ready: bool,
    /// The unconsumed items and the cleanup.
    drained: bool,
    /// What a refused retirement named.
    returns: bool,
}

/// The asks in flight whose tickets the pump holds: a start (a stop takes
/// its ticket back), a stop and a drain (neither is asked twice).
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
struct Flight {
    start: bool,
    stop: bool,
    drain: bool,
}

/// The rules' asks, with their tickets, into the pump's outbox.
struct Flighted<'o, O> {
    out: &'o mut O,
    flight: Flight,
}

impl<'id, C, F, O: Outbox<'id, C, F>> Asks<'id, C, F> for Flighted<'_, O> {
    fn start(&mut self, stopped: Stopped<'id>, channel: C) {
        self.flight.start = true;
        self.out.start(Asked::new(), stopped, channel);
    }
    fn stop(&mut self) {
        // A stop ends a start in progress: no answer of it is left to come.
        if core::mem::take(&mut self.flight.start) {
            let Asked(_) = self.out.cancel_start();
        }
        if !self.flight.stop {
            self.flight.stop = true;
            self.out.stop(Asked::new());
        }
    }
    fn stop_data(&mut self) {
        self.out.stop_data(Asked::new());
    }
    fn end_tokens(&mut self) {
        self.out.end_tokens(Asked::new());
    }
    fn drain(&mut self, stopped: &Stopped<'id>) {
        if !self.flight.drain {
            self.flight.drain = true;
            self.out.drain(Asked::new(), stopped);
        }
    }
    fn stop_network(&mut self) {
        self.out.stop_network(Asked::new());
    }
    fn retire(&mut self, stopped: Stopped<'id>) {
        self.out.retire(Asked::new(), stopped);
    }
    fn reinstall(&mut self, retired: Retired<'id>, epoch: u32) {
        self.out.reinstall(Asked::new(), retired, epoch);
    }
    fn release(&mut self, retired: Retired<'id>) {
        self.out.release(Asked::new(), retired);
    }
    fn refused(&mut self, request: Request<C>, phase: Phase) {
        self.out.refused(request, phase);
    }
    fn ended(&mut self, end: Result<End, Unreached<'_, C, F>>) {
        self.out.ended(end);
    }
    fn status(&mut self, status: Status<'_, F>) {
        self.out.status(status);
    }
    fn deadline(&mut self, at: Option<Instant>) {
        self.out.deadline(at);
    }
}

impl Waits {
    const fn any(self) -> bool {
        self.ready || self.drained || self.returns
    }
}

/// The installation machine of one port: `C` the channel, `F` the port's
/// fault. Built in place ([`Machine::place`]) and reset in place
/// ([`Machine::reset`]); a step takes `&mut self`.
#[derive(Debug)]
pub struct Machine<'id, C, F> {
    state: State<'id>,
    goal: Option<Goal<C>>,
    data: Data,
    net: Net,
    /// This installation's tokens and unconsumed items were asked for.
    ended: bool,
    waits: Waits,
    flight: Flight,
    fault: PhantomData<fn() -> F>,
}

impl<'id, C: Copy, F> Machine<'id, C, F> {
    /// The machine of a fresh installation, stopped as `stopped` proves,
    /// written into `place`.
    pub fn place<'p>(
        place: &'p mut core::mem::MaybeUninit<Self>,
        stopped: Stopped<'id>,
    ) -> &'p mut Self {
        place.write(Self {
            state: State::Stopped(stopped),
            goal: None,
            data: Data::Stopped,
            net: Net::Idle,
            ended: false,
            waits: Waits {
                ready: false,
                drained: false,
                returns: false,
            },
            flight: Flight {
                start: false,
                stop: false,
                drain: false,
            },
            fault: PhantomData,
        })
    }

    /// Reset the machine in place for the port made again, stopped as
    /// `stopped` proves.
    pub fn reset(&mut self, stopped: Stopped<'id>) {
        self.state = State::Stopped(stopped);
        self.goal = None;
        self.data = Data::Stopped;
        self.net = Net::Idle;
        self.ended = false;
        self.waits = Waits::default();
        self.flight = Flight::default();
    }

    /// The phase.
    pub const fn phase(&self) -> Phase {
        self.state.phase()
    }

    /// Take the state out for a transition, leaving the released phase,
    /// which holds nothing, until the transition writes the next.
    fn take(&mut self) -> State<'id> {
        core::mem::replace(&mut self.state, State::Released)
    }

    /// One event. An answer spends its ask's ticket; a stop and a drain in
    /// flight are not asked again, and a stop takes a start's ticket back.
    pub fn step(&mut self, event: Event<'_, 'id, C, F>, out: &mut impl Outbox<'id, C, F>) {
        let mut flight = self.flight;
        let ev = match event {
            Event::Request { request, deadline } => Ev::Request { request, deadline },
            Event::Start(Asked(_), answer) => {
                flight.start = false;
                match answer {
                    StartAnswer::Started => Ev::Started,
                    StartAnswer::Refused(stopped, why) => Ev::StartRefused(stopped, why),
                    StartAnswer::Unstopped { cause, stop } => Ev::StartUnstopped { cause, stop },
                    StartAnswer::Stale => Ev::Stale,
                }
            }
            Event::Stop(Asked(_), answer) => {
                flight.stop = false;
                match answer {
                    StopAnswer::Stopped(stopped) => Ev::Stopped(stopped),
                    StopAnswer::Failed(stop) => Ev::StopUnconfirmed(stop),
                    StopAnswer::Released => Ev::StopReleased,
                }
            }
            // The stop goes on, still in flight.
            Event::StopGoesOn(Asked(_), stop) => Ev::StopUnconfirmed(stop),
            Event::DataStopped(Asked(_)) => Ev::DataStopped,
            Event::Ready(Asked(_)) => Ev::Ready,
            Event::Drain(Asked(_), answer) => {
                flight.drain = false;
                match answer {
                    DrainAnswer::Drained => Ev::Drained,
                    DrainAnswer::Refused => Ev::DrainRefused,
                    DrainAnswer::Later => Ev::DrainLater,
                    DrainAnswer::Faulted(fault) => Ev::DrainFaulted(fault),
                    DrainAnswer::Stale => Ev::Stale,
                }
            }
            Event::NetworkStopped(Asked(_)) => Ev::NetworkStopped,
            Event::Retire(Asked(_), answer) => match answer {
                RetireAnswer::Retired(retired) => Ev::Retired(retired),
                RetireAnswer::Refused(stopped, outstanding) => {
                    Ev::RetireRefused(stopped, outstanding)
                }
                RetireAnswer::Stale => Ev::Stale,
            },
            Event::Reinstall(Asked(_), answer) => match answer {
                ReinstallAnswer::Reinstalled(stopped) => Ev::Reinstalled(stopped),
                ReinstallAnswer::Refused(retired, why) => Ev::ReinstallRefused(retired, why),
                ReinstallAnswer::Stale => Ev::Stale,
            },
            Event::Release(Asked(_), answer) => match answer {
                ReleaseAnswer::Released => Ev::Released,
                ReleaseAnswer::Stale => Ev::Stale,
            },
            Event::Fault(answer) => Ev::Fault(answer),
            Event::Room => Ev::Room,
            Event::Returned => Ev::Returned,
            Event::Again => Ev::Again,
            Event::Deadline => Ev::Deadline,
        };
        let mut out = Flighted { out, flight };
        self.rules(ev, &mut out);
        self.flight = out.flight;
    }

    fn rules(&mut self, event: Ev<'_, 'id, C, F>, out: &mut impl Asks<'id, C, F>) {
        // Each answer comes with its ask's ticket: the phase that asked it
        // is the phase it finds.
        let phase = self.phase();
        match event {
            Ev::Request { request, deadline } => self.request(request, deadline, out),
            Ev::DataStopped => {
                self.data = Data::Stopped;
                self.proceed(out);
            }
            Ev::NetworkStopped => {
                self.net = Net::Stopped;
                if matches!(self.goal, Some(Goal::Release(_))) {
                    match self.take() {
                        State::Retired(retired) => self.after_retired(retired, out),
                        other => self.state = other,
                    }
                }
            }
            Ev::Ready => {
                self.waits.ready = false;
                self.proceed(out);
            }
            Ev::Drained => {
                self.waits.drained = false;
                self.proceed(out);
            }
            Ev::DrainRefused => {
                out.status(Status::Held(Waiting::Unconsumed));
            }
            Ev::DrainLater => {
                out.status(Status::Held(Waiting::Cleanup));
            }
            Ev::DrainFaulted(fault) => {
                out.status(Status::Held(Waiting::CleanupFault(fault)));
            }
            Ev::Again => {
                // The pump's next round after an answer that said "later".
                if phase == Phase::Stopping {
                    out.stop();
                }
                if self.waits.drained
                    && let State::Stopped(stopped) | State::Settling(stopped) = &self.state
                {
                    out.drain(stopped);
                }
            }
            // Look again: nothing is done when no drain waits.
            Ev::Room => match &self.state {
                State::Stopped(stopped) | State::Settling(stopped) if self.waits.drained => {
                    out.drain(stopped);
                }
                _ => {}
            },
            Ev::Deadline => out.status(Status::Held(Waiting::Deadline(phase))),
            Ev::Fault(answer) => self.fault(answer, out),
            Ev::Stale => self.stale(out),
            Ev::Started => {
                self.state = State::Running;
                self.goal = None;
                self.data = Data::Running;
                out.ended(Ok(End::Started));
            }
            Ev::StartRefused(stopped, failure) => {
                // The radio is stopped; after a retune the data still runs
                // and stops now.
                out.ended(Err(Unreached::StartRefused(StartEnd::Refused(failure))));
                self.settle(stopped, None, out);
            }
            Ev::StartUnstopped { cause, stop } => {
                // The port is stopping and gave no proof: the stop goes on.
                self.state = State::Stopping;
                self.goal = None;
                out.stop();
                out.ended(Err(Unreached::StartRefused(StartEnd::Unstopped {
                    cause,
                    stop,
                })));
            }
            Ev::Stopped(stopped) => {
                if let Some(Goal::Run(channel)) = self.goal {
                    self.state = State::Starting;
                    out.start(stopped, channel);
                    return;
                }
                let asked = self.settle(stopped, self.goal, out);
                // The drain a stale proof lost is asked with the new one.
                if !asked
                    && self.waits.drained
                    && let State::Settling(stopped) = &self.state
                {
                    out.drain(stopped);
                }
            }
            Ev::StopReleased => {
                // The port holds no installation: that is the machine's
                // state too; the open request ends with it.
                match self.goal {
                    Some(Goal::Release(_)) => out.ended(Ok(End::Released)),
                    Some(_) => out.ended(Err(Unreached::Released)),
                    None => {}
                }
                self.state = State::Released;
                self.goal = None;
            }
            Ev::StopUnconfirmed(stop) => {
                out.status(Status::Held(Waiting::Radio(stop)));
            }
            Ev::Retired(retired) => {
                self.after_retired(retired, out);
            }
            Ev::RetireRefused(stopped, outstanding) => {
                self.state = State::Settling(stopped);
                self.waits.returns = true;
                out.status(Status::Held(Waiting::Outstanding(outstanding)));
            }
            // Look again: nothing is done when no retirement waits.
            Ev::Returned => {
                if phase == Phase::Settling && self.waits.returns {
                    self.waits.returns = false;
                    self.proceed(out);
                }
            }
            Ev::Reinstalled(stopped) => {
                self.ended = false;
                if let Some(Goal::Restart { .. }) = self.goal {
                    self.state = State::Stopped(stopped);
                    self.goal = None;
                    out.ended(Ok(End::Reinstalled));
                    return;
                }
                // A release was asked meanwhile: the new installation ends
                // too.
                let _asked = self.settle(stopped, self.goal, out);
            }
            Ev::ReinstallRefused(retired, why) => {
                if let Some(Goal::Restart { .. }) = self.goal {
                    self.state = State::Retired(retired);
                    self.goal = None;
                    out.ended(Err(Unreached::RestartRefused(why)));
                    return;
                }
                self.after_retired(retired, out);
            }
            Ev::Released => {
                self.state = State::Released;
                self.goal = None;
                out.ended(Ok(End::Released));
            }
        }
    }

    fn request(
        &mut self,
        request: Request<C>,
        deadline: Option<Instant>,
        out: &mut impl Asks<'id, C, F>,
    ) {
        let phase = self.phase();
        let refused = (request, phase);
        if phase == Phase::Released {
            return out.refused(refused.0, refused.1);
        }
        let goal = match request {
            Request::Start(channel) => {
                if !matches!(self.state, State::Stopped(_)) {
                    return out.refused(refused.0, refused.1);
                }
                let State::Stopped(stopped) = self.take() else {
                    return out.refused(refused.0, refused.1);
                };
                self.state = State::Starting;
                self.goal = Some(Goal::Run(channel));
                out.start(stopped, channel);
                return;
            }
            Request::Retune(channel) => {
                if phase != Phase::Running {
                    return out.refused(refused.0, refused.1);
                }
                self.state = State::Stopping;
                self.goal = Some(Goal::Run(channel));
                out.stop();
                return;
            }
            Request::Halt => Goal::Halt,
            Request::Restart { epoch } => Goal::Restart { epoch },
            Request::Release(network) => Goal::Release(network),
        };
        // An ending: equal or weaker than the one in progress is refused,
        // stronger takes its place.
        let superseded = match self.goal {
            Some(current) if goal.rank() <= current.rank() => {
                return out.refused(refused.0, refused.1);
            }
            current => current,
        };
        if phase == Phase::Releasing || (phase == Phase::Retired && matches!(goal, Goal::Halt)) {
            return out.refused(refused.0, refused.1);
        }
        if let Some(current) = superseded {
            out.ended(Err(Unreached::Superseded(current)));
        }
        out.deadline(deadline);
        match self.take() {
            State::Stopped(stopped) => {
                if matches!(goal, Goal::Halt) {
                    self.state = State::Stopped(stopped);
                    out.ended(Ok(End::Halted));
                    return;
                }
                let _asked = self.settle(stopped, Some(goal), out);
            }
            State::Starting => {
                self.state = State::Stopping;
                self.goal = Some(goal);
                out.stop();
            }
            State::Running => {
                self.state = State::Stopping;
                self.goal = Some(goal);
                self.data = Data::Stopping;
                out.stop();
                out.stop_data();
            }
            State::Stopping => {
                // The data of a retune under way stops when the radio has.
                self.state = State::Stopping;
                self.want_network(goal, out);
                self.goal = Some(goal);
            }
            State::Settling(stopped) => {
                let _asked = self.settle(stopped, Some(goal), out);
            }
            state @ (State::Retiring | State::Reinstalling) => {
                self.state = state;
                self.want_network(goal, out);
                self.goal = Some(goal);
            }
            State::Retired(retired) => {
                self.want_network(goal, out);
                self.goal = Some(goal);
                self.after_retired(retired, out);
            }
            // Refused above.
            state @ (State::Releasing | State::Released) => self.state = state,
        }
    }

    /// A release that stops the network asks its stop once.
    fn want_network(&mut self, goal: Goal<C>, out: &mut impl Asks<'id, C, F>) {
        if matches!(goal, Goal::Release(Network::Stops)) && self.net == Net::Idle {
            out.stop_network();
            self.net = Net::Stopping;
        }
    }

    /// A stopped installation, holding the stop's proof, goes to `goal`:
    /// whether it asked the drain.
    fn settle(
        &mut self,
        stopped: Stopped<'id>,
        goal: Option<Goal<C>>,
        out: &mut impl Asks<'id, C, F>,
    ) -> bool {
        if self.data == Data::Running {
            self.data = Data::Stopping;
            out.stop_data();
        }
        let mut drained = false;
        if goal.as_ref().is_some_and(Goal::retires) && !self.ended {
            self.ended = true;
            self.waits.ready = true;
            self.waits.drained = true;
            out.end_tokens();
            out.drain(&stopped);
            drained = true;
        }
        if let Some(goal) = goal {
            self.want_network(goal, out);
        }
        self.state = State::Settling(stopped);
        self.goal = goal;
        self.proceed(out);
        drained
    }

    /// A settling state goes on once nothing it waits for is left.
    fn proceed(&mut self, out: &mut impl Asks<'id, C, F>) {
        if !matches!(self.state, State::Settling(_)) || self.data != Data::Stopped {
            return;
        }
        let State::Settling(stopped) = self.take() else {
            return;
        };
        match self.goal {
            None => self.state = State::Stopped(stopped),
            Some(Goal::Halt) => {
                self.state = State::Stopped(stopped);
                self.goal = None;
                out.ended(Ok(End::Halted));
            }
            Some(_) if self.waits.any() => self.state = State::Settling(stopped),
            Some(_) => {
                self.state = State::Retiring;
                out.retire(stopped);
            }
        }
    }

    /// Holding the retirement's proof: reinstall, release, or wait.
    fn after_retired(&mut self, retired: Retired<'id>, out: &mut impl Asks<'id, C, F>) {
        match self.goal {
            Some(Goal::Restart { epoch }) => {
                self.state = State::Reinstalling;
                out.reinstall(retired, epoch);
            }
            Some(Goal::Release(Network::Stops)) if self.net != Net::Stopped => {
                self.state = State::Retired(retired);
            }
            Some(Goal::Release(_)) => {
                self.state = State::Releasing;
                out.release(retired);
            }
            _ => {
                self.state = State::Retired(retired);
                self.goal = None;
            }
        }
    }

    /// A fault of the port, in any phase.
    fn fault(&mut self, answer: FaultAnswer<'_, F>, out: &mut impl Asks<'id, C, F>) {
        let phase = self.phase();
        let kept = match answer {
            FaultAnswer::TakenBack => None,
            FaultAnswer::Kept { fault, outstanding } => Some((fault, outstanding)),
        };
        if matches!(phase, Phase::Starting | Phase::Running) {
            // The run, or the start, ends by a stop.
            out.status(Status::Interrupted);
            if let Some((fault, outstanding)) = kept {
                out.status(Status::Held(Waiting::Fault { fault, outstanding }));
            }
            out.stop();
            if self.data == Data::Running {
                self.data = Data::Stopping;
                out.stop_data();
            }
            if phase == Phase::Starting {
                out.ended(Err(Unreached::StartRefused(StartEnd::Fault)));
            }
            self.state = State::Stopping;
            self.goal = None;
            return;
        }
        // A cleanup the fault refused is asked again.
        if kept.is_none()
            && self.waits.drained
            && let State::Stopped(stopped) | State::Settling(stopped) = &self.state
        {
            out.drain(stopped);
        }
        match kept {
            Some((fault, outstanding)) => {
                out.status(Status::Held(Waiting::Fault { fault, outstanding }));
            }
            // The fault was what the retirement named: ask it again.
            None if phase == Phase::Settling && self.waits.returns => {
                self.waits.returns = false;
                self.proceed(out);
            }
            None => {}
        }
    }

    /// A proof the port did not take: it is gone, a stop gives the next.
    fn stale(&mut self, out: &mut impl Asks<'id, C, F>) {
        // From a settling it is the drain's answer: the stop's proof the
        // machine still holds is the stale one, and goes.
        out.status(Status::Held(Waiting::Stale));
        if matches!(self.goal, Some(Goal::Run(_))) {
            self.goal = None;
            out.ended(Err(Unreached::StartRefused(StartEnd::Stale)));
        }
        self.state = State::Stopping;
        out.stop();
    }
}
