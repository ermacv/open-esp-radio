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
    clippy::disallowed_methods,
    clippy::struct_excessive_bools,
    clippy::fn_params_excessive_bools,
    clippy::wildcard_enum_match_arm
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
        clippy::wildcard_enum_match_arm,
        clippy::type_complexity,
    )
)]

//! The installation machine: one sans-IO machine a port, which decides
//! every transition of the port's installation for every role, over the
//! radio's life of the port's contract (`design/port-contract-2026-11-16.md`,
//! section 3).
//!
//! It touches no port, no frame and no clock. [`Machine::step`] takes one
//! [`Input`], something that comes later, and returns a [`Step`] that
//! borrows the machine. A step that asks carries the right the call takes
//! and a [`Resume`], whose one method takes the port's answer, the
//! contract's own value, and returns the next step, until the step rests
//! ([`Step::Rest`]). No answer is an error: a start or a retune that failed
//! answers the radio stopped, and a stop always ends.
//!
//! The machine is the one holder of the radio's rights and of the stop's
//! right, and of the data's rights while the data is stopped. A position
//! holds exactly the right its name says; a right a step carries out is
//! the step's, and the machine's position is then out until the answer
//! gives the next one. A step that is dropped leaves its right with it.
//!
//! A controller asks from its [`Permit`], which only the machine makes (one
//! at its placement, one at each end but a release's), typed by the radio's
//! position: a run of a running radio, a retune without the installation's
//! closed gate, a request while another is open and a request after a
//! release cannot be written, the machine refuses nothing, and each request
//! is closed by exactly one [`Ended`].
//!
//! The port's one indication, "the port needs a stop" ([`Input::NeedsStop`]),
//! is answered by the stop the machine has: a start or a retune in progress
//! is given up and its request ends; a running radio's run is interrupted,
//! the controller is told, the data stops and then the radio.

#[cfg(test)]
mod tests;

use core::marker::PhantomData;
use core::mem::{self, MaybeUninit};

use oer_portable_time::Instant;
use oer_protocols_ieee80211_contracts_port::GateLent;
use oer_protocols_ieee80211_contracts_port::life::{
    Retired, RetunePoll, Retuning, Running, StartFailure, StartPoll, Starting, StopPoll, StopRight,
    Stopped, Stopping, UnitsHome,
};

/// The brand of one port's installations, invariant in `'id`.
type Brand<'id> = PhantomData<fn(&'id ()) -> &'id ()>;

/// The controller's right to ask anything of a stopped radio: a run, a
/// halt, a release. Affine, of no size, made by the machine alone.
#[must_use = "a permit is the controller's only way to ask"]
#[derive(Debug)]
pub struct LiveStopped<'id> {
    brand: Brand<'id>,
}

/// The controller's right to ask anything of a running radio: a retune, a
/// halt, a release.
#[must_use = "a permit is the controller's only way to ask"]
#[derive(Debug)]
pub struct LiveRunning<'id> {
    brand: Brand<'id>,
}

/// The controller's right to ask anything: its installation's tokens are
/// held. Typed by the radio's position when it was made.
#[must_use = "a permit is the controller's only way to ask"]
#[derive(Debug)]
pub enum Live<'id> {
    /// The radio is not running.
    Stopped(LiveStopped<'id>),
    /// The radio runs.
    Running(LiveRunning<'id>),
}

/// The controller's right to ask a release: its installation's tokens are
/// asked for or ended, or it is retired.
#[must_use = "a permit is the controller's only way to ask"]
#[derive(Debug)]
pub struct Spent<'id> {
    brand: Brand<'id>,
}

/// What the controller may ask next.
#[must_use = "a permit is the controller's only way to ask"]
#[derive(Debug)]
pub enum Permit<'id> {
    /// Anything.
    Live(Live<'id>),
    /// A release.
    Spent(Spent<'id>),
}

/// Whether a release stops the network first.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Network {
    /// The network stays.
    Stays,
    /// The network is told to stop, and the release waits for its stop.
    Stops,
}

/// What a request asks.
#[derive(Clone, Copy, Debug)]
enum Aim<'g, 'id, C> {
    Run(C),
    /// A retune, under the gate it is begun with.
    Retune(C, GateLent<'g, 'id>),
    Halt,
    Release(Network),
}

/// One request of the controller, made from its permit, with a deadline of
/// its choice or none.
#[must_use = "a request holds the controller's permit"]
#[derive(Debug)]
pub struct Request<'g, 'id, C> {
    aim: Aim<'g, 'id, C>,
    deadline: Option<Instant>,
    brand: Brand<'id>,
}

impl<'g, 'id, C> Request<'g, 'id, C> {
    /// The radio running on `channel`: a start of the radio, which is not
    /// running.
    pub const fn run(permit: LiveStopped<'id>, channel: C, deadline: Option<Instant>) -> Self {
        let LiveStopped { brand } = permit;
        Self {
            aim: Aim::Run(channel),
            deadline,
            brand,
        }
    }

    /// The running radio moved to `channel`, asked under the transmit gate
    /// of this installation the caller closed and lends for the step the
    /// request is given to, where the port's retune begins under it: the
    /// data stays as it is, and nothing is told to the data plane.
    pub const fn retune(
        permit: LiveRunning<'id>,
        channel: C,
        gate: GateLent<'g, 'id>,
        deadline: Option<Instant>,
    ) -> Self {
        let LiveRunning { brand } = permit;
        Self {
            aim: Aim::Retune(channel, gate),
            deadline,
            brand,
        }
    }

    /// The radio stopped and the data home; the installation kept.
    pub const fn halt(permit: Live<'id>, deadline: Option<Instant>) -> Self {
        Self {
            aim: Aim::Halt,
            deadline,
            brand: live_brand(permit),
        }
    }

    /// The port without an installation, the network staying or stopping.
    pub const fn release(permit: Permit<'id>, network: Network, deadline: Option<Instant>) -> Self {
        let brand = match permit {
            Permit::Live(live) => live_brand(live),
            Permit::Spent(Spent { brand }) => brand,
        };
        Self {
            aim: Aim::Release(network),
            deadline,
            brand,
        }
    }
}

const fn live_brand(permit: Live<'_>) -> Brand<'_> {
    match permit {
        Live::Stopped(LiveStopped { brand }) | Live::Running(LiveRunning { brand }) => brand,
    }
}

/// What the controller needs to end its tokens: their end is made only
/// from it.
#[must_use = "the controller ends its tokens from the order"]
#[derive(Debug)]
pub struct Order<'id> {
    brand: Brand<'id>,
}

impl<'id> Order<'id> {
    /// The controller ended its installation's tokens.
    pub const fn ended(self) -> TokensEnded<'id> {
        let Self { brand } = self;
        TokensEnded { brand }
    }
}

/// The controller's tokens of this installation are ended.
#[derive(Debug)]
pub struct TokensEnded<'id> {
    brand: Brand<'id>,
}

/// What comes later.
#[derive(Debug)]
pub enum Input<'g, 'id, C> {
    /// A request of the controller.
    Request(Request<'g, 'id, C>),
    /// "Look again": the pump's turn. It alone sees a request's deadline,
    /// when the step's `now` is not before it.
    Turn,
    /// The port's one indication: the port needs a stop. It carries
    /// nothing.
    NeedsStop,
    /// The controller ended its tokens.
    TokensEnded(TokensEnded<'id>),
    /// The network stopped.
    NetworkStopped,
}

/// The data plane's answer to "stop the data".
#[derive(Debug)]
pub enum DataStop<D> {
    /// The data is home, with its rights.
    Stopped(D),
    /// It is coming home; the plane's completions wake.
    Going,
}

/// What the machine waits for: a status, never an end.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Waited {
    /// Nothing.
    Nothing,
    /// The units the network holds, which a retirement waits for.
    Units,
}

/// What a request reached.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum End {
    /// The radio runs on the channel asked.
    Started,
    /// The radio is stopped and the data home.
    Halted,
}

/// Why a request did not reach what it asked.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Unreached {
    /// The start or the retune failed, and the radio is stopped.
    StartRefused(StartFailure),
    /// The port needed a stop: the start or the retune is given up.
    StartInterrupted,
    /// The deadline passed, while the machine waited for this.
    Deadline(Waited),
}

/// The one end of the open request.
#[derive(Debug)]
pub enum Ended<'id> {
    /// The request ended, and the permit it returns.
    Request(Result<End, Unreached>, Permit<'id>),
    /// The port is released: the end of the port, and of the stop's right.
    /// It returns no permit.
    Released,
}

/// A message to another party; it has no answer.
#[derive(Debug)]
pub enum Tell<'id, D> {
    /// To the controller: end your tokens of this installation.
    EndTokens(Order<'id>),
    /// To the network: stop.
    StopNetwork,
    /// To the data plane: the data flows, with its rights.
    DataFlows(D),
}

/// What comes after a step that rests.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Rest {
    /// An input, or the wake of whoever answered "not yet", or a turn at
    /// the request's deadline.
    Wake(Option<Instant>),
    /// A turn in the pump's next bounded round: the port moved, and the
    /// pass is made again.
    Round,
}

/// A start the port is asked: begun from the stopped radio, or polled.
#[derive(Debug)]
pub enum StartAsk<'id, C> {
    /// Begin it on the channel.
    Begin(Stopped<'id>, C),
    /// The start in progress, asked again.
    Going(Starting<'id>),
}

/// A retune the port is asked: begun from the running radio under the
/// closed gate the request named, or polled.
#[derive(Debug)]
pub enum RetuneAsk<'g, 'id, C> {
    /// Begin it on the channel, under the gate its request lent.
    Begin(Running<'id>, C, GateLent<'g, 'id>),
    /// The retune in progress, asked again.
    Going(Retuning<'id>),
}

/// A stop the port is asked: begun from a running radio, a start or a
/// retune, or polled.
#[derive(Debug)]
pub enum StopAsk<'id> {
    /// Begin the stop of a running radio.
    Running(Running<'id>),
    /// Begin the stop of a start in progress.
    Starting(Starting<'id>),
    /// Begin the stop of a retune in progress.
    Retuning(Retuning<'id>),
    /// The stop in progress, asked again.
    Going(Stopping<'id>),
}

/// A step of the machine: one thing asked, told or said, or its rest.
#[must_use = "a step goes on only through its resume"]
pub enum Step<'m, 'id, C, D> {
    /// Ask the port's start.
    Start(StartAsk<'id, C>, Resume<'m, 'id, C, D, AskStart>),
    /// Ask the port's retune.
    Retune(RetuneAsk<'m, 'id, C>, Resume<'m, 'id, C, D, AskRetune>),
    /// Ask the port's stop, with the stop's right [`Resume::right`] lends.
    Stop(StopAsk<'id>, Resume<'m, 'id, C, D, AskStop>),
    /// Ask the data plane to stop the data.
    StopData(Resume<'m, 'id, C, D, AskStopData>),
    /// Ask the port whether the network's units are home.
    UnitsHome(Resume<'m, 'id, C, D, AskUnits>),
    /// Retire the installation.
    Retire(
        Stopped<'id>,
        D,
        UnitsHome<'id>,
        Resume<'m, 'id, C, D, AskRetire>,
    ),
    /// Release the port: it takes the stop's right too.
    Release(Retired<'id>, StopRight<'id>, Releasing<'m, 'id, C, D>),
    /// The release's end: the end of the port, and of the stop's right. It
    /// returns no permit, and the step rests.
    Released(Ended<'id>, Rest),
    /// Pass this on to its party, and go on.
    Tell(Tell<'id, D>, Resume<'m, 'id, C, D, Told>),
    /// The one end of the open request: to the controller, and go on.
    Ended(Ended<'id>, Resume<'m, 'id, C, D, Told>),
    /// To the controller: the run is interrupted, and go on.
    Interrupted(Resume<'m, 'id, C, D, Told>),
    /// What is waited for changed: [`Resume::waiting`] reads it.
    Held(Resume<'m, 'id, C, D, Said>),
    /// The step is over, and what comes next.
    Rest(Rest),
}

/// The port that holds an installation, held until the ask `K` is answered
/// or the message passed.
#[must_use = "a step goes on only through its resume"]
pub struct Resume<'m, 'id, C, D, K> {
    port: Port<'m, 'id, C, D>,
    now: Instant,
    kind: K,
}

/// The whole machine, held while its release is asked.
#[must_use = "a step goes on only through its resume"]
pub struct Releasing<'m, 'id, C, D> {
    machine: &'m mut Machine<'id, C, D>,
}

/// A start is asked.
pub struct AskStart(());

/// A retune is asked.
pub struct AskRetune(());

/// A stop is asked.
pub struct AskStop {
    asked: Asked,
}

/// The data's stop is asked.
pub struct AskStopData {
    before: DataBefore,
}

/// Whether this step asked the data's stop already.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Asked {
    Not,
    Data,
}

/// What the data's stop is asked before.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DataBefore {
    /// The radio's stop of an ending.
    Stop,
    /// The pass of the stopped installation towards its goal.
    Goal,
}

/// Whether the network's units are home is asked.
pub struct AskUnits(());

/// A retirement is asked.
pub struct AskRetire(());

/// A message is passed on; the step goes on.
pub struct Told {
    next: Next,
}

/// A status is said; the step goes on.
pub struct Said(());

/// The machine of one port's installation. Built in its place, reached by
/// `&mut`.
pub struct Machine<'id, C, D> {
    life: Life<'id, D>,
    goal: Goal<C>,
    waiting: Waited,
}

/// The port's installation, and the stop's right while it lasts.
enum Life<'id, D> {
    /// The port holds the installation; the machine holds the stop's right.
    Port {
        right: StopRight<'id>,
        at: At<'id>,
        data: Data<D>,
        tokens: Tokens,
    },
    /// A release is asked: the right is out with the retirement.
    Releasing,
    /// The port is released.
    Released,
}

/// The radio's position, each holding the right its name says.
enum At<'id> {
    Stopped(Stopped<'id>),
    Starting(Starting<'id>),
    Running(Running<'id>),
    Retuning(Retuning<'id>),
    Stopping(Stopping<'id>),
    Retired(Retired<'id>),
    /// The position's right is out with a step's ask.
    Out(Position),
}

/// A position's name, without its right.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Position {
    Stopped,
    Starting,
    Running,
    Retuning,
    Stopping,
    Retired,
}

impl At<'_> {
    const fn position(&self) -> Position {
        match self {
            Self::Stopped(_) => Position::Stopped,
            Self::Starting(_) => Position::Starting,
            Self::Running(_) => Position::Running,
            Self::Retuning(_) => Position::Retuning,
            Self::Stopping(_) => Position::Stopping,
            Self::Retired(_) => Position::Retired,
            Self::Out(position) => *position,
        }
    }
}

/// The data plane, with the data's rights while it is stopped.
enum Data<D> {
    Flowing,
    Stopping,
    Stopped(D),
    /// The installation is retired: its rights went with the retirement.
    Retired,
}

impl<D> Data<D> {
    const fn flowing(&self) -> bool {
        matches!(self, Self::Flowing)
    }

    const fn stopping(&self) -> bool {
        matches!(self, Self::Stopping)
    }

    const fn stopped(&self) -> bool {
        matches!(self, Self::Stopped(_))
    }
}

/// The installation's tokens with the controller.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Tokens {
    Held,
    Asked,
    Ended,
}

/// The open request, or the stop the port asked for.
#[derive(Clone, Copy, Debug)]
enum Goal<C> {
    None,
    /// No request: the stop the port asked for is under way.
    Service,
    Open {
        want: Want<C>,
        deadline: Option<Instant>,
    },
}

impl<C> Goal<C> {
    const fn deadline(&self) -> Option<Instant> {
        match self {
            Self::Open { deadline, .. } => *deadline,
            Self::None | Self::Service => None,
        }
    }

    /// Whether the goal ends the run: a halt, a release, or the stop the
    /// port asked for.
    const fn ends(&self) -> bool {
        match self {
            Self::Service
            | Self::Open {
                want: Want::Halt | Want::Release(_),
                ..
            } => true,
            Self::None
            | Self::Open {
                want: Want::Run(_), ..
            } => false,
        }
    }
}

/// What the open request wants.
#[derive(Clone, Copy, Debug)]
enum Want<C> {
    Run(C),
    Halt,
    Release(NetworkStep),
}

/// The network, for a release.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NetworkStep {
    /// It stays.
    Stays,
    /// It was told to stop.
    Told,
    /// It stopped.
    Stopped,
}

/// How a step goes on after a message.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Next {
    /// Towards the goal.
    Advance,
    /// The step rests with a wake.
    Wake,
    /// The start's end, after the data was told it flows.
    EndStarted,
    /// The stopped installation's release, after its tokens were asked.
    Retirement,
}

impl<'id, C: Copy, D> Machine<'id, C, D> {
    /// The machine of the port's installation, stopped as `stopped` proves,
    /// with the stop's right and the data's rights the install gave beside
    /// it, written into `place`; and the controller's first permit.
    pub fn place<'p>(
        place: &'p mut MaybeUninit<Self>,
        stopped: Stopped<'id>,
        right: StopRight<'id>,
        data: D,
    ) -> (&'p mut Self, LiveStopped<'id>) {
        let machine = place.write(Self {
            life: Life::Port {
                right,
                at: At::Stopped(stopped),
                data: Data::Stopped(data),
                tokens: Tokens::Held,
            },
            goal: Goal::None,
            waiting: Waited::Nothing,
        });
        (machine, LiveStopped { brand: PhantomData })
    }

    /// What is waited for: a status, read at any time.
    pub const fn waiting(&self) -> Waited {
        self.waiting
    }

    /// One input, then towards the goal as far as the answers allow.
    ///
    /// A retired installation's release, which takes the stop's right out
    /// of the machine, is decided here, before the step: a release is only
    /// ever asked by a step that begins retired, and nothing is said before
    /// it in that step.
    pub fn step<'s>(&'s mut self, input: Input<'s, 'id, C>, now: Instant) -> Step<'s, 'id, C, D> {
        if self.release_due(&input, now) {
            self.prepare_release(input);
            return self.release();
        }
        let Self {
            life,
            goal,
            waiting,
        } = self;
        match life {
            Life::Port {
                right,
                at,
                data,
                tokens,
            } => Port {
                right,
                at,
                data,
                tokens,
                goal,
                waiting,
            }
            .step(input, now),
            Life::Releasing | Life::Released => Step::Rest(Rest::Wake(goal.deadline())),
        }
    }

    /// Whether this input leaves a retired installation due for its
    /// release in this step: its goal releases with the network staying or
    /// stopped, and no deadline passed.
    fn release_due(&self, input: &Input<'_, 'id, C>, now: Instant) -> bool {
        let Life::Port {
            at: At::Retired(_), ..
        } = &self.life
        else {
            return false;
        };
        let passed = |deadline: Option<Instant>| deadline.is_some_and(|deadline| deadline <= now);
        match input {
            Input::Request(Request {
                aim: Aim::Release(Network::Stays),
                ..
            }) => true,
            Input::Request(_) => false,
            Input::NetworkStopped => matches!(
                self.goal,
                Goal::Open {
                    want: Want::Release(_),
                    ..
                }
            ),
            Input::Turn | Input::TokensEnded(_) | Input::NeedsStop => match self.goal {
                Goal::Open {
                    want: Want::Release(NetworkStep::Stays | NetworkStep::Stopped),
                    deadline,
                } => matches!(input, Input::TokensEnded(_) | Input::NeedsStop) || !passed(deadline),
                Goal::Open { .. } | Goal::None | Goal::Service => false,
            },
        }
    }

    /// The input's effect on a retired installation whose release is due.
    fn prepare_release(&mut self, input: Input<'_, 'id, C>) {
        match input {
            Input::Request(Request { aim, deadline, .. }) => {
                if let Aim::Release(Network::Stays) = aim {
                    self.goal = Goal::Open {
                        want: Want::Release(NetworkStep::Stays),
                        deadline,
                    };
                }
            }
            Input::NetworkStopped => {
                if let Goal::Open {
                    want: Want::Release(network @ (NetworkStep::Told | NetworkStep::Stopped)),
                    ..
                } = &mut self.goal
                {
                    *network = NetworkStep::Stopped;
                }
            }
            Input::Turn | Input::TokensEnded(_) | Input::NeedsStop => {}
        }
    }

    /// The release of a retired installation, which takes the right with
    /// the retirement.
    fn release(&mut self) -> Step<'_, 'id, C, D> {
        match mem::replace(&mut self.life, Life::Releasing) {
            Life::Port {
                right,
                at: At::Retired(retired),
                ..
            } => Step::Release(retired, right, Releasing { machine: self }),
            // `release_due` named a retired installation: anything else is
            // given back as it was.
            life @ (Life::Port { .. } | Life::Releasing | Life::Released) => {
                self.life = life;
                Step::Rest(Rest::Round)
            }
        }
    }
}

/// A view of a port that holds an installation.
struct Port<'m, 'id, C, D> {
    right: &'m mut StopRight<'id>,
    at: &'m mut At<'id>,
    data: &'m mut Data<D>,
    tokens: &'m mut Tokens,
    goal: &'m mut Goal<C>,
    waiting: &'m mut Waited,
}

impl<'m, 'id, C: Copy, D> Port<'m, 'id, C, D> {
    fn resume<K>(self, now: Instant, kind: K) -> Resume<'m, 'id, C, D, K> {
        Resume {
            port: self,
            now,
            kind,
        }
    }

    fn wake(&self) -> Step<'m, 'id, C, D> {
        Step::Rest(Rest::Wake(self.goal.deadline()))
    }

    /// The permit an end returns: what may be asked next.
    const fn permit(&self) -> Permit<'id> {
        match (*self.tokens, self.at.position()) {
            (Tokens::Held, Position::Running) => {
                Permit::Live(Live::Running(LiveRunning { brand: PhantomData }))
            }
            (
                Tokens::Held,
                Position::Stopped | Position::Starting | Position::Retuning | Position::Stopping,
            ) => Permit::Live(Live::Stopped(LiveStopped { brand: PhantomData })),
            (Tokens::Held, Position::Retired) | (Tokens::Asked | Tokens::Ended, _) => {
                Permit::Spent(Spent { brand: PhantomData })
            }
        }
    }

    /// The open request ends.
    fn end(self, result: Result<End, Unreached>, next: Next, now: Instant) -> Step<'m, 'id, C, D> {
        *self.goal = Goal::None;
        let permit = self.permit();
        Step::Ended(
            Ended::Request(result, permit),
            self.resume(now, Told { next }),
        )
    }

    fn go(self, next: Next, now: Instant) -> Step<'m, 'id, C, D> {
        match next {
            Next::Advance => self.advance(now),
            Next::Wake => self.wake(),
            Next::EndStarted => self.end(Ok(End::Started), Next::Wake, now),
            Next::Retirement => self.retirement(now),
        }
    }

    fn step(self, input: Input<'m, 'id, C>, now: Instant) -> Step<'m, 'id, C, D> {
        match input {
            Input::Request(request) => self.request(request, now),
            Input::Turn => match *self.goal {
                Goal::Open {
                    deadline: Some(deadline),
                    ..
                } if deadline <= now => {
                    *self.goal = Goal::None;
                    let waited = *self.waiting;
                    self.end(Err(Unreached::Deadline(waited)), Next::Advance, now)
                }
                Goal::Open { .. } | Goal::None | Goal::Service => self.advance(now),
            },
            Input::NeedsStop => self.needs_stop(now),
            Input::TokensEnded(TokensEnded { brand: _ }) => {
                *self.tokens = Tokens::Ended;
                self.advance(now)
            }
            Input::NetworkStopped => {
                if let Goal::Open {
                    want: Want::Release(network @ (NetworkStep::Told | NetworkStep::Stopped)),
                    ..
                } = self.goal
                {
                    *network = NetworkStep::Stopped;
                }
                self.advance(now)
            }
        }
    }

    fn request(self, request: Request<'m, 'id, C>, now: Instant) -> Step<'m, 'id, C, D> {
        let Request {
            aim,
            deadline,
            brand: _,
        } = request;
        let want = match aim {
            Aim::Run(channel) | Aim::Retune(channel, _) => Want::Run(channel),
            Aim::Halt => Want::Halt,
            Aim::Release(Network::Stays) => Want::Release(NetworkStep::Stays),
            Aim::Release(Network::Stops) => Want::Release(NetworkStep::Told),
        };
        *self.goal = Goal::Open { want, deadline };
        match aim {
            Aim::Release(Network::Stops) => Step::Tell(
                Tell::StopNetwork,
                self.resume(
                    now,
                    Told {
                        next: Next::Advance,
                    },
                ),
            ),
            Aim::Retune(channel, gate) => self.retune(channel, gate, now),
            Aim::Release(Network::Stays) | Aim::Run(_) | Aim::Halt => self.advance(now),
        }
    }

    /// A retune asked of the running radio: begun in the request's step,
    /// under the gate it lent.
    fn retune(mut self, channel: C, gate: GateLent<'m, 'id>, now: Instant) -> Step<'m, 'id, C, D> {
        match self.take() {
            At::Running(running) => Step::Retune(
                RetuneAsk::Begin(running, channel, gate),
                self.resume(now, AskRetune(())),
            ),
            at @ (At::Stopped(_)
            | At::Starting(_)
            | At::Retuning(_)
            | At::Stopping(_)
            | At::Retired(_)
            | At::Out(_)) => {
                *self.at = at;
                self.advance(now)
            }
        }
    }

    /// The port needs a stop: a start or a retune is given up and its
    /// request ends; a running radio's run is interrupted; a stop goes on.
    fn needs_stop(self, now: Instant) -> Step<'m, 'id, C, D> {
        match self.at.position() {
            Position::Starting | Position::Retuning => {
                self.end(Err(Unreached::StartInterrupted), Next::Advance, now)
            }
            Position::Running => {
                *self.goal = Goal::Service;
                Step::Interrupted(self.resume(
                    now,
                    Told {
                        next: Next::Advance,
                    },
                ))
            }
            Position::Stopping | Position::Stopped | Position::Retired => self.advance(now),
        }
    }

    /// Take the position's right out for an ask.
    fn take(&mut self) -> At<'id> {
        let position = self.at.position();
        mem::replace(self.at, At::Out(position))
    }

    /// Towards the goal, as far as the answers allow.
    fn advance(mut self, now: Instant) -> Step<'m, 'id, C, D> {
        let goal = *self.goal;
        match self.take() {
            At::Running(running) => match goal {
                // A run of the running radio is a retune, begun in its
                // request's step (`Port::retune`); it is not asked again.
                Goal::None
                | Goal::Open {
                    want: Want::Run(_), ..
                } => {
                    *self.at = At::Running(running);
                    self.wake()
                }
                Goal::Service
                | Goal::Open {
                    want: Want::Halt | Want::Release(_),
                    ..
                } => self.stop_section(StopAsk::Running(running), now),
            },
            At::Starting(starting) => match goal {
                Goal::Open { .. } => {
                    Step::Start(StartAsk::Going(starting), self.resume(now, AskStart(())))
                }
                Goal::None | Goal::Service => self.stop_section(StopAsk::Starting(starting), now),
            },
            At::Retuning(retuning) => match goal {
                Goal::Open {
                    want: Want::Run(_), ..
                } => Step::Retune(RetuneAsk::Going(retuning), self.resume(now, AskRetune(()))),
                Goal::None
                | Goal::Service
                | Goal::Open {
                    want: Want::Halt | Want::Release(_),
                    ..
                } => self.stop_section(StopAsk::Retuning(retuning), now),
            },
            At::Stopping(stopping) => self.stop_section(StopAsk::Going(stopping), now),
            At::Stopped(stopped) => {
                *self.at = At::Stopped(stopped);
                self.stopped_section(Asked::Not, now)
            }
            At::Retired(retired) => {
                *self.at = At::Retired(retired);
                // A release is decided at a step's beginning
                // (`Machine::step`); anything else waits.
                self.wake()
            }
            At::Out(position) => {
                *self.at = At::Out(position);
                self.wake()
            }
        }
    }

    /// The radio leaves its position: an ending stops the data first (a
    /// start or a retune given up stops with the data as it is: a start
    /// has none flowing, and a retune's gate is closed), then the radio.
    fn stop_section(self, ask: StopAsk<'id>, now: Instant) -> Step<'m, 'id, C, D> {
        if self.goal.ends() && self.data.flowing() {
            // The right waits with the ask, out of the machine: a stopping
            // position holds it in its name only until the data answers.
            *self.at = match ask {
                StopAsk::Running(running) => At::Running(running),
                StopAsk::Starting(starting) => At::Starting(starting),
                StopAsk::Retuning(retuning) => At::Retuning(retuning),
                StopAsk::Going(stopping) => At::Stopping(stopping),
            };
            Step::StopData(self.resume(
                now,
                AskStopData {
                    before: DataBefore::Stop,
                },
            ))
        } else {
            Step::Stop(ask, self.resume(now, AskStop { asked: Asked::Not }))
        }
    }

    /// A stopped radio towards the goal.
    fn stopped_section(self, asked: Asked, now: Instant) -> Step<'m, 'id, C, D> {
        let goal = *self.goal;
        if asked == Asked::Not && (self.data.stopping() || (goal.ends() && self.data.flowing())) {
            return Step::StopData(self.resume(
                now,
                AskStopData {
                    before: DataBefore::Goal,
                },
            ));
        }
        self.toward(now)
    }

    /// A stopped radio towards the goal, the data asked.
    fn toward(mut self, now: Instant) -> Step<'m, 'id, C, D> {
        match *self.goal {
            Goal::None => self.wake(),
            Goal::Open {
                want: Want::Run(channel),
                ..
            } => {
                if self.data.stopping() {
                    return self.wake();
                }
                match self.take() {
                    At::Stopped(stopped) => Step::Start(
                        StartAsk::Begin(stopped, channel),
                        self.resume(now, AskStart(())),
                    ),
                    other @ (At::Starting(_)
                    | At::Running(_)
                    | At::Retuning(_)
                    | At::Stopping(_)
                    | At::Retired(_)
                    | At::Out(_)) => {
                        *self.at = other;
                        self.wake()
                    }
                }
            }
            Goal::Service => {
                if self.data.stopped() {
                    *self.goal = Goal::None;
                }
                self.wake()
            }
            Goal::Open {
                want: Want::Halt, ..
            } => {
                if self.data.stopped() {
                    self.end(Ok(End::Halted), Next::Wake, now)
                } else {
                    self.wake()
                }
            }
            Goal::Open {
                want: Want::Release(_),
                ..
            } => {
                if *self.tokens == Tokens::Held {
                    *self.tokens = Tokens::Asked;
                    Step::Tell(
                        Tell::EndTokens(Order { brand: PhantomData }),
                        self.resume(
                            now,
                            Told {
                                next: Next::Retirement,
                            },
                        ),
                    )
                } else {
                    self.retirement(now)
                }
            }
        }
    }

    /// The retirement of a stopped radio, asked only with the data stopped,
    /// the tokens ended and the network's units home.
    fn retirement(self, now: Instant) -> Step<'m, 'id, C, D> {
        if !self.data.stopped() || *self.tokens != Tokens::Ended {
            return self.wake();
        }
        Step::UnitsHome(self.resume(now, AskUnits(())))
    }
}

impl<'m, 'id, C: Copy, D> Resume<'m, 'id, C, D, AskStart> {
    /// The port's answer to the start.
    pub fn answer(self, answer: StartPoll<'id>) -> Step<'m, 'id, C, D> {
        let Self { port, now, .. } = self;
        match answer {
            StartPoll::Running(running) => {
                *port.at = At::Running(running);
                *port.goal = Goal::None;
                match mem::replace(port.data, Data::Flowing) {
                    Data::Stopped(rights) => Step::Tell(
                        Tell::DataFlows(rights),
                        port.resume(
                            now,
                            Told {
                                next: Next::EndStarted,
                            },
                        ),
                    ),
                    data @ (Data::Flowing | Data::Stopping | Data::Retired) => {
                        *port.data = data;
                        port.go(Next::EndStarted, now)
                    }
                }
            }
            StartPoll::Pending(starting) => {
                *port.at = At::Starting(starting);
                port.wake()
            }
            StartPoll::Stopped(stopped, failure) => {
                *port.at = At::Stopped(stopped);
                port.end(Err(Unreached::StartRefused(failure)), Next::Wake, now)
            }
        }
    }
}

impl<'m, 'id, C: Copy, D> Resume<'m, 'id, C, D, AskRetune> {
    /// The port's answer to the retune: the data stays as it is, and
    /// nothing is told to the data plane.
    pub fn answer(self, answer: RetunePoll<'id>) -> Step<'m, 'id, C, D> {
        let Self { port, now, .. } = self;
        match answer {
            RetunePoll::Running(running) => {
                *port.at = At::Running(running);
                port.end(Ok(End::Started), Next::Wake, now)
            }
            RetunePoll::Pending(retuning) => {
                *port.at = At::Retuning(retuning);
                port.wake()
            }
            RetunePoll::Stopped(stopped, failure) => {
                *port.at = At::Stopped(stopped);
                port.end(Err(Unreached::StartRefused(failure)), Next::Wake, now)
            }
        }
    }
}

impl<'m, 'id, C: Copy, D> Resume<'m, 'id, C, D, AskStop> {
    /// The stop's right, lent for the port's stop.
    pub fn right(&mut self) -> &mut StopRight<'id> {
        &mut *self.port.right
    }

    /// The port's answer to the stop.
    pub fn answer(self, answer: StopPoll<'id>) -> Step<'m, 'id, C, D> {
        let Self {
            port,
            now,
            kind: AskStop { asked },
        } = self;
        match answer {
            StopPoll::Pending(stopping) => {
                *port.at = At::Stopping(stopping);
                port.wake()
            }
            StopPoll::Stopped(stopped) => {
                *port.at = At::Stopped(stopped);
                port.stopped_section(asked, now)
            }
        }
    }
}

impl<'m, 'id, C: Copy, D> Resume<'m, 'id, C, D, AskStopData> {
    /// The data plane's answer.
    pub fn answer(self, answer: DataStop<D>) -> Step<'m, 'id, C, D> {
        let Self {
            mut port,
            now,
            kind: AskStopData { before },
        } = self;
        *port.data = match answer {
            DataStop::Stopped(rights) => Data::Stopped(rights),
            DataStop::Going => Data::Stopping,
        };
        match before {
            DataBefore::Goal => port.toward(now),
            DataBefore::Stop => {
                let ask = match port.take() {
                    At::Running(running) => StopAsk::Running(running),
                    At::Starting(starting) => StopAsk::Starting(starting),
                    At::Retuning(retuning) => StopAsk::Retuning(retuning),
                    At::Stopping(stopping) => StopAsk::Going(stopping),
                    other @ (At::Stopped(_) | At::Retired(_) | At::Out(_)) => {
                        *port.at = other;
                        return port.wake();
                    }
                };
                Step::Stop(ask, port.resume(now, AskStop { asked: Asked::Data }))
            }
        }
    }
}

impl<'m, 'id, C: Copy, D> Resume<'m, 'id, C, D, AskUnits> {
    /// The network's units are not all home yet; the port's facts wake.
    pub fn pending(self) -> Step<'m, 'id, C, D> {
        let Self { port, now, .. } = self;
        if *port.waiting == Waited::Units {
            port.wake()
        } else {
            *port.waiting = Waited::Units;
            Step::Held(port.resume(now, Said(())))
        }
    }

    /// The network's units are home: the retirement is asked.
    pub fn home(self, units: UnitsHome<'id>) -> Step<'m, 'id, C, D> {
        let Self { mut port, now, .. } = self;
        if *port.waiting == Waited::Units {
            *port.waiting = Waited::Nothing;
        }
        let stopped = match port.take() {
            At::Stopped(stopped) => stopped,
            other @ (At::Starting(_)
            | At::Running(_)
            | At::Retuning(_)
            | At::Stopping(_)
            | At::Retired(_)
            | At::Out(_)) => {
                *port.at = other;
                return port.wake();
            }
        };
        match mem::replace(port.data, Data::Retired) {
            Data::Stopped(rights) => {
                Step::Retire(stopped, rights, units, port.resume(now, AskRetire(())))
            }
            data @ (Data::Flowing | Data::Stopping | Data::Retired) => {
                *port.data = data;
                *port.at = At::Stopped(stopped);
                port.wake()
            }
        }
    }
}

impl<'m, 'id, C: Copy, D> Resume<'m, 'id, C, D, AskRetire> {
    /// The port's answer to the retirement: the retired installation goes
    /// on from the next step's beginning, in the next round.
    pub fn answer(self, retired: Retired<'id>) -> Step<'m, 'id, C, D> {
        let Self { port, .. } = self;
        *port.at = At::Retired(retired);
        Step::Rest(Rest::Round)
    }
}

impl<'m, 'id, C: Copy, D> Releasing<'m, 'id, C, D> {
    /// The port released: the end of the port, and of the stop's right.
    pub fn answer(self) -> Step<'m, 'id, C, D> {
        let Self { machine } = self;
        machine.life = Life::Released;
        machine.goal = Goal::None;
        Step::Released(Ended::Released, Rest::Wake(None))
    }
}

impl<'m, 'id, C: Copy, D> Resume<'m, 'id, C, D, Told> {
    /// The message is passed on: the step goes on.
    pub fn go(self) -> Step<'m, 'id, C, D> {
        let Self {
            port,
            now,
            kind: Told { next },
        } = self;
        port.go(next, now)
    }
}

impl<'m, 'id, C: Copy, D> Resume<'m, 'id, C, D, Said> {
    /// What is waited for.
    pub const fn waiting(&self) -> Waited {
        *self.port.waiting
    }

    /// The status is said: the step rests.
    pub fn go(self) -> Step<'m, 'id, C, D> {
        self.port.wake()
    }
}
