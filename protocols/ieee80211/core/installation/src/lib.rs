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
//! the [`Report`]s of the request it serves. The machine is the one holder
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

/// One event of the machine. What is larger than two words comes by
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
    /// A sink refused an unconsumed item, or the cleanup did not finish.
    DrainRefused,
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
    /// A stop that failed may be asked again: the pump's retry time.
    Retry,
}

/// The kind of an [`Event`], as a report of one the machine has no
/// transition for names it.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum EventKind {
    /// [`Event::Request`].
    Request,
    /// [`Event::Started`].
    Started,
    /// [`Event::StartRefused`].
    StartRefused,
    /// [`Event::StartUnstopped`].
    StartUnstopped,
    /// [`Event::Stopped`].
    Stopped,
    /// [`Event::StopUnconfirmed`].
    StopUnconfirmed,
    /// [`Event::StopReleased`].
    StopReleased,
    /// [`Event::Fault`].
    Fault,
    /// [`Event::DataStopped`].
    DataStopped,
    /// [`Event::Ready`].
    Ready,
    /// [`Event::Drained`].
    Drained,
    /// [`Event::DrainRefused`].
    DrainRefused,
    /// [`Event::Room`].
    Room,
    /// [`Event::NetworkStopped`].
    NetworkStopped,
    /// [`Event::Retired`].
    Retired,
    /// [`Event::RetireRefused`].
    RetireRefused,
    /// [`Event::Returned`].
    Returned,
    /// [`Event::Reinstalled`].
    Reinstalled,
    /// [`Event::ReinstallRefused`].
    ReinstallRefused,
    /// [`Event::Released`].
    Released,
    /// [`Event::Stale`].
    Stale,
    /// [`Event::Deadline`].
    Deadline,
    /// [`Event::Retry`].
    Retry,
}

impl<C, F> Event<'_, '_, C, F> {
    /// Its kind.
    pub const fn kind(&self) -> EventKind {
        match self {
            Self::Request { .. } => EventKind::Request,
            Self::Started => EventKind::Started,
            Self::StartRefused(..) => EventKind::StartRefused,
            Self::StartUnstopped { .. } => EventKind::StartUnstopped,
            Self::Stopped(_) => EventKind::Stopped,
            Self::StopUnconfirmed(_) => EventKind::StopUnconfirmed,
            Self::StopReleased => EventKind::StopReleased,
            Self::Fault(_) => EventKind::Fault,
            Self::DataStopped => EventKind::DataStopped,
            Self::Ready => EventKind::Ready,
            Self::Drained => EventKind::Drained,
            Self::DrainRefused => EventKind::DrainRefused,
            Self::Room => EventKind::Room,
            Self::NetworkStopped => EventKind::NetworkStopped,
            Self::Retired(_) => EventKind::Retired,
            Self::RetireRefused(..) => EventKind::RetireRefused,
            Self::Returned => EventKind::Returned,
            Self::Reinstalled(_) => EventKind::Reinstalled,
            Self::ReinstallRefused(..) => EventKind::ReinstallRefused,
            Self::Released => EventKind::Released,
            Self::Stale => EventKind::Stale,
            Self::Deadline => EventKind::Deadline,
            Self::Retry => EventKind::Retry,
        }
    }
}

/// What the machine reports of the request it serves, or of what it waits
/// for.
#[derive(Debug, Eq, PartialEq)]
pub enum Report<'a, C, F> {
    /// The request was refused at once, by the phase that refuses it.
    Refused {
        /// The request.
        request: Request<C>,
        /// The phase.
        phase: Phase,
    },
    /// The goal in progress was replaced by a stronger ending: its request
    /// is closed.
    Superseded(Goal<C>),
    /// The radio runs.
    Started,
    /// The start ended without the radio running.
    StartRefused(StartEnd<'a, F>),
    /// The radio stopped and its data is home.
    Halted,
    /// The next installation is installed and stopped.
    Reinstalled,
    /// The port refused the next installation; the machine holds the
    /// retired one.
    RestartRefused(ReinstallRefused),
    /// The port released its installations.
    Released,
    /// The port answered a stop "released": the open request, not a
    /// release, ends with the port.
    EndedByRelease(Goal<C>),
    /// A fault ended the run: the controller says what it wants next.
    Interrupted,
    /// What the machine waits for; never an end.
    Held(Waiting<'a, F>),
    /// The machine has no transition for the event in its phase: it changed
    /// nothing. The pump sent what no ask of the machine called for.
    NotExpected {
        /// The event.
        event: EventKind,
        /// The phase.
        phase: Phase,
    },
}

/// What the pump lends a step: one method an ask, one for a report.
pub trait Outbox<'id, C, F> {
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
    /// A report.
    fn report(&mut self, report: Report<'_, C, F>);
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

    /// One event.
    pub fn step(&mut self, event: Event<'_, 'id, C, F>, out: &mut impl Outbox<'id, C, F>) {
        let phase = self.phase();
        let kind = event.kind();
        let not_expected = Report::NotExpected { event: kind, phase };
        match event {
            Event::Request { request, deadline } => self.request(request, deadline, out),
            Event::DataStopped => {
                if self.data != Data::Stopping {
                    return out.report(not_expected);
                }
                self.data = Data::Stopped;
                self.proceed(out);
            }
            Event::NetworkStopped => {
                if self.net != Net::Stopping {
                    return out.report(not_expected);
                }
                self.net = Net::Stopped;
                if matches!(self.goal, Some(Goal::Release(_))) {
                    match self.take() {
                        State::Retired(retired) => self.after_retired(retired, out),
                        other => self.state = other,
                    }
                }
            }
            Event::Ready => {
                if !self.waits.ready {
                    return out.report(not_expected);
                }
                self.waits.ready = false;
                self.proceed(out);
            }
            Event::Drained => {
                if !self.waits.drained {
                    return out.report(not_expected);
                }
                self.waits.drained = false;
                self.proceed(out);
            }
            Event::DrainRefused => {
                if !self.waits.drained {
                    return out.report(not_expected);
                }
                out.report(Report::Held(Waiting::Unconsumed));
            }
            Event::Room => match &self.state {
                State::Stopped(stopped) | State::Settling(stopped) if self.waits.drained => {
                    out.drain(stopped);
                }
                _ => out.report(not_expected),
            },
            Event::Deadline => out.report(Report::Held(Waiting::Deadline(phase))),
            Event::Fault(answer) => self.fault(answer, out),
            Event::Stale => self.stale(out),
            Event::Started if phase == Phase::Starting => {
                self.state = State::Running;
                self.goal = None;
                self.data = Data::Running;
                out.report(Report::Started);
            }
            Event::StartRefused(stopped, failure) if phase == Phase::Starting => {
                // The radio is stopped; after a retune the data still runs
                // and stops now.
                out.report(Report::StartRefused(StartEnd::Refused(failure)));
                self.settle(stopped, None, out);
            }
            Event::StartUnstopped { cause, stop } if phase == Phase::Starting => {
                // The port is stopping and gave no proof: the stop goes on.
                self.state = State::Stopping;
                self.goal = None;
                out.stop();
                out.report(Report::StartRefused(StartEnd::Unstopped { cause, stop }));
            }
            Event::Stopped(stopped) if phase == Phase::Stopping => {
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
            Event::StopReleased if phase == Phase::Stopping => {
                // The port holds no installation: that is the machine's
                // state too; the open request ends with it.
                match self.goal {
                    Some(Goal::Release(_)) => out.report(Report::Released),
                    Some(goal) => out.report(Report::EndedByRelease(goal)),
                    None => {}
                }
                self.state = State::Released;
                self.goal = None;
            }
            Event::StopUnconfirmed(stop) if phase == Phase::Stopping => {
                out.report(Report::Held(Waiting::Radio(stop)));
            }
            Event::Retry if phase == Phase::Stopping => out.stop(),
            Event::Retired(retired) if phase == Phase::Retiring => {
                self.after_retired(retired, out);
            }
            Event::RetireRefused(stopped, outstanding) if phase == Phase::Retiring => {
                self.state = State::Settling(stopped);
                self.waits.returns = true;
                out.report(Report::Held(Waiting::Outstanding(outstanding)));
            }
            Event::Returned if phase == Phase::Settling => {
                if !self.waits.returns {
                    return out.report(not_expected);
                }
                self.waits.returns = false;
                self.proceed(out);
            }
            Event::Reinstalled(stopped) if phase == Phase::Reinstalling => {
                self.ended = false;
                if let Some(Goal::Restart { .. }) = self.goal {
                    self.state = State::Stopped(stopped);
                    self.goal = None;
                    out.report(Report::Reinstalled);
                    return;
                }
                // A release was asked meanwhile: the new installation ends
                // too.
                let _asked = self.settle(stopped, self.goal, out);
            }
            Event::ReinstallRefused(retired, why) if phase == Phase::Reinstalling => {
                if let Some(Goal::Restart { .. }) = self.goal {
                    self.state = State::Retired(retired);
                    self.goal = None;
                    out.report(Report::RestartRefused(why));
                    return;
                }
                self.after_retired(retired, out);
            }
            Event::Released if phase == Phase::Releasing => {
                self.state = State::Released;
                self.goal = None;
                out.report(Report::Released);
            }
            _ => out.report(not_expected),
        }
    }

    fn request(
        &mut self,
        request: Request<C>,
        deadline: Option<Instant>,
        out: &mut impl Outbox<'id, C, F>,
    ) {
        let phase = self.phase();
        let refused = Report::Refused { request, phase };
        if phase == Phase::Released {
            return out.report(refused);
        }
        let goal = match request {
            Request::Start(channel) => {
                if !matches!(self.state, State::Stopped(_)) {
                    return out.report(refused);
                }
                let State::Stopped(stopped) = self.take() else {
                    return out.report(refused);
                };
                self.state = State::Starting;
                self.goal = Some(Goal::Run(channel));
                out.start(stopped, channel);
                return;
            }
            Request::Retune(channel) => {
                if phase != Phase::Running {
                    return out.report(refused);
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
                return out.report(refused);
            }
            current => current,
        };
        if phase == Phase::Releasing || (phase == Phase::Retired && matches!(goal, Goal::Halt)) {
            return out.report(refused);
        }
        if let Some(current) = superseded {
            out.report(Report::Superseded(current));
        }
        out.deadline(deadline);
        match self.take() {
            State::Stopped(stopped) => {
                if matches!(goal, Goal::Halt) {
                    self.state = State::Stopped(stopped);
                    out.report(Report::Halted);
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
    fn want_network(&mut self, goal: Goal<C>, out: &mut impl Outbox<'id, C, F>) {
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
        out: &mut impl Outbox<'id, C, F>,
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
    fn proceed(&mut self, out: &mut impl Outbox<'id, C, F>) {
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
                out.report(Report::Halted);
            }
            Some(_) if self.waits.any() => self.state = State::Settling(stopped),
            Some(_) => {
                self.state = State::Retiring;
                out.retire(stopped);
            }
        }
    }

    /// Holding the retirement's proof: reinstall, release, or wait.
    fn after_retired(&mut self, retired: Retired<'id>, out: &mut impl Outbox<'id, C, F>) {
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
    fn fault(&mut self, answer: FaultAnswer<'_, F>, out: &mut impl Outbox<'id, C, F>) {
        let phase = self.phase();
        let kept = match answer {
            FaultAnswer::TakenBack => None,
            FaultAnswer::Kept { fault, outstanding } => Some((fault, outstanding)),
        };
        if matches!(phase, Phase::Starting | Phase::Running) {
            // The run, or the start, ends by a stop.
            out.report(Report::Interrupted);
            if let Some((fault, outstanding)) = kept {
                out.report(Report::Held(Waiting::Fault { fault, outstanding }));
            }
            out.stop();
            if self.data == Data::Running {
                self.data = Data::Stopping;
                out.stop_data();
            }
            if phase == Phase::Starting {
                out.report(Report::StartRefused(StartEnd::Fault));
            }
            self.state = State::Stopping;
            self.goal = None;
            return;
        }
        match kept {
            Some((fault, outstanding)) => {
                out.report(Report::Held(Waiting::Fault { fault, outstanding }));
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
    fn stale(&mut self, out: &mut impl Outbox<'id, C, F>) {
        let phase = self.phase();
        if !matches!(
            phase,
            Phase::Starting
                | Phase::Settling
                | Phase::Retiring
                | Phase::Reinstalling
                | Phase::Releasing
        ) {
            out.report(Report::NotExpected {
                event: EventKind::Stale,
                phase,
            });
            return;
        }
        // From a settling it is the drain's answer: the stop's proof the
        // machine still holds is the stale one, and goes.
        out.report(Report::Held(Waiting::Stale));
        if matches!(self.goal, Some(Goal::Run(_))) {
            self.goal = None;
            out.report(Report::StartRefused(StartEnd::Stale));
        }
        self.state = State::Stopping;
        out.stop();
    }
}
