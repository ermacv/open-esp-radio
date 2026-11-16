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
    )
)]

//! The installation machine: one sans-IO machine a port, which decides
//! every transition of the port's installation for every role.
//!
//! It touches no port, no frame and no clock. [`Machine::step`] takes one
//! [`Input`], something that comes later, and returns a [`Step`] that
//! borrows the machine. A step that asks carries a [`Resume`], whose one
//! method takes the answer, the port contract's own value, and returns the
//! next step, until the step rests ([`Step::Rest`]). What answers at once
//! is asked and answered inside the step; nothing is kept between the two,
//! and while a step is out the machine cannot be stepped.
//!
//! The machine is the one holder of the port's proofs and of the stop's
//! right. A position holds exactly the proof its name says; a proof a step
//! carries out is the step's, and the machine's position is then
//! "stopping", which holds none, so a step that is dropped leaves a state a
//! stop mends (a stopped port answers its stop with a proof). The one
//! exception is a release, which takes the right with the retirement's
//! proof: a step dropped there leaves a retired port nobody can release,
//! with nothing running.
//!
//! A controller asks from its [`Permit`], which only the machine makes (one
//! at its placement, one at each end but a release's): so a request while
//! another is open, a run of an installation without tokens and a request
//! after a release cannot be written, the machine refuses nothing, and
//! each request is closed by exactly one [`Ended`].

#[cfg(test)]
mod tests;

use core::marker::PhantomData;
use core::mem::{self, MaybeUninit};

use oer_portable_time::{Duration, Instant};
use oer_protocols_ieee80211_contracts_port::{
    CleanupRefused, FaultKept, GateClosed, Outstanding, ProofRefused, Recovered, ReinstallRefused,
    ReleaseRefused, Retired, StaleProof, StartFailure, StartPoll, StartProgress, StartProof,
    StartRefused, Starting, StopRight, StopUnconfirmed, Stopped,
};

/// The brand of one port's installations, invariant in `'id`.
type Brand<'id> = PhantomData<fn(&'id ()) -> &'id ()>;

/// The controller's right to ask anything: its installation's tokens are
/// held. Affine, of no size, made by the machine alone.
#[must_use = "a permit is the controller's only way to ask"]
#[derive(Debug)]
pub struct Live<'id> {
    brand: Brand<'id>,
}

/// The controller's right to ask a restart or a release: its
/// installation's tokens are asked for or ended, or it is retired.
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
    /// A restart or a release.
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
enum Aim<C> {
    Run(C),
    Halt,
    Restart,
    Release(Network),
}

/// One request of the controller, made from its permit, with a deadline of
/// its choice or none.
#[must_use = "a request holds the controller's permit"]
#[derive(Debug)]
pub struct Request<'id, C> {
    aim: Aim<C>,
    deadline: Option<Instant>,
    brand: Brand<'id>,
}

impl<'id, C> Request<'id, C> {
    /// The radio running on `channel`: a start of a stopped radio.
    pub const fn run(permit: Live<'id>, channel: C, deadline: Option<Instant>) -> Self {
        let Live { brand } = permit;
        Self {
            aim: Aim::Run(channel),
            deadline,
            brand,
        }
    }

    /// A running radio moved to `channel`, asked under the transmit gate
    /// the caller closed: the data stays as it is, and nothing is told to
    /// the data plane, since a closed gate publishes nothing while the
    /// radio stops and starts again.
    pub const fn retune<P: oer_protocols_radio_port::PoolPath>(
        permit: Live<'id>,
        channel: C,
        _closed: &GateClosed<'_, P>,
        deadline: Option<Instant>,
    ) -> Self {
        let Live { brand } = permit;
        Self {
            aim: Aim::Run(channel),
            deadline,
            brand,
        }
    }

    /// The radio stopped, the data home, the installation kept.
    pub const fn halt(permit: Live<'id>, deadline: Option<Instant>) -> Self {
        let Live { brand } = permit;
        Self {
            aim: Aim::Halt,
            deadline,
            brand,
        }
    }

    /// The next installation, stopped; the network stays.
    pub const fn restart(permit: Permit<'id>, deadline: Option<Instant>) -> Self {
        Self {
            aim: Aim::Restart,
            deadline,
            brand: brand_of(permit),
        }
    }

    /// The port without an installation, the network staying or stopping.
    pub const fn release(permit: Permit<'id>, network: Network, deadline: Option<Instant>) -> Self {
        Self {
            aim: Aim::Release(network),
            deadline,
            brand: brand_of(permit),
        }
    }
}

const fn brand_of(permit: Permit<'_>) -> Brand<'_> {
    match permit {
        Permit::Live(Live { brand }) | Permit::Spent(Spent { brand }) => brand,
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
pub enum Input<'id, C> {
    /// A request of the controller.
    Request(Request<'id, C>),
    /// "Look again": the pump's turn. It alone sees a request's deadline,
    /// when the step's `now` is not before it.
    Turn,
    /// The port holds a fault.
    Fault,
    /// The controller ended its tokens.
    TokensEnded(TokensEnded<'id>),
    /// The network stopped.
    NetworkStopped,
}

/// The data plane's answer to "stop the data".
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DataStop {
    /// The data is home.
    Stopped,
    /// It is coming home; the plane's completions wake.
    Going,
}

/// What the machine waits for, with what the port said: a status, never
/// an end.
#[derive(Debug, Eq, PartialEq)]
pub enum Waiting<F> {
    /// Nothing.
    Nothing,
    /// The radio's stop, unconfirmed.
    Radio(StopUnconfirmed<F>),
    /// A fault the port keeps.
    Fault(FaultKept<F>),
    /// The cleanup, which cannot finish now.
    Cleanup(CleanupRefused<F>),
    /// What is out, which a retirement waits for.
    Outstanding(Outstanding<F>),
}

/// The class of what is waited for, without what the port said.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Waited {
    /// Nothing.
    Nothing,
    /// The radio's stop.
    Radio,
    /// A fault the port keeps.
    Fault,
    /// The cleanup.
    Cleanup,
    /// What is out.
    Outstanding,
}

impl<F> Waiting<F> {
    /// Its class.
    pub const fn class(&self) -> Waited {
        match self {
            Self::Nothing => Waited::Nothing,
            Self::Radio(_) => Waited::Radio,
            Self::Fault(_) => Waited::Fault,
            Self::Cleanup(_) => Waited::Cleanup,
            Self::Outstanding(_) => Waited::Outstanding,
        }
    }
}

/// What a request reached.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum End {
    /// The radio runs on the channel asked.
    Started,
    /// The radio is stopped and the data home.
    Halted,
    /// The next installation is installed, stopped.
    Reinstalled,
}

/// Why a request did not reach what it asked.
#[derive(Debug, Eq, PartialEq)]
pub enum Unreached<F> {
    /// The port refused the start, and the radio is stopped.
    StartRefused(StartFailure<F>),
    /// The port's start failed after it began, and its stop is not
    /// confirmed: the machine stops the radio next.
    StartUnstopped {
        /// Why the start failed.
        cause: StartFailure<F>,
        /// What the stop of the begun start observed.
        stop: StopUnconfirmed<F>,
    },
    /// A fault of the port ended the start; the machine stops the radio.
    StartInterrupted,
    /// The port refused the reinstall: the installation stays retired.
    RestartRefused(ReinstallRefused),
    /// The deadline passed, while the machine waited for this.
    Deadline(Waited),
}

/// The one end of the open request.
#[derive(Debug)]
pub enum Ended<'id, F> {
    /// The request ended, and the permit it returns.
    Request(Result<End, Unreached<F>>, Permit<'id>),
    /// The port is released: the end of the port, and of the stop's right.
    /// It returns no permit.
    Released,
}

/// A message to another party; it has no answer.
#[derive(Debug)]
pub enum Tell<'id> {
    /// To the controller: end your tokens of this installation.
    EndTokens(Order<'id>),
    /// To the network: stop.
    StopNetwork,
    /// To the data plane: the data flows.
    DataFlows,
}

/// What comes after a step that rests.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Rest {
    /// An input, or the wake of whoever answered "not yet", or a turn at
    /// the instant: the request's deadline, or the retry time after an
    /// answer that names nothing that wakes.
    Wake(Option<Instant>),
    /// A turn in the pump's next bounded round: the port moved, and the
    /// pass is made again.
    Round,
}

/// A step of the machine: one thing asked, told or said, or its rest.
#[must_use = "a step goes on only through its resume"]
pub enum Step<'m, 'id, C, F> {
    /// Ask the port's start with the proof, on the channel.
    Start(StartProof<'id>, C, Resume<'m, 'id, C, F, AskStart<C>>),
    /// Poll the port's stop with the stop's right [`Resume::right`] lends;
    /// its answer is [`Resume::answer`] or [`Resume::pending`].
    Stop(Resume<'m, 'id, C, F, AskStop>),
    /// Ask the data plane to stop the data.
    StopData(Resume<'m, 'id, C, F, AskStopData<'id>>),
    /// Clean what the stop left, under the stop's proof, which comes back
    /// with the answer.
    Clean(Stopped<'id>, Resume<'m, 'id, C, F, AskClean>),
    /// Retire the installation.
    Retire(Stopped<'id>, Resume<'m, 'id, C, F, AskRetire>),
    /// Reinstall the next installation: [`Resume::installed`] with the
    /// next installation's stop, or [`Resume::refused`].
    Reinstall(Retired<'id>, Resume<'m, 'id, C, F, AskReinstall>),
    /// Release the port: it takes the stop's right too, which a refusal
    /// gives back.
    Release(Retired<'id>, StopRight<'id>, Releasing<'m, 'id, C, F>),
    /// The release's end: the end of the port, and of the stop's right. It
    /// returns no permit, and the step rests.
    Released(Ended<'id, F>, Rest),
    /// Ask what a stop can do about the port's fault, with the stop's right
    /// [`Resume::right`] lends.
    Recover(Resume<'m, 'id, C, F, AskRecover>),
    /// Pass this on to its party, and go on.
    Tell(Tell<'id>, Resume<'m, 'id, C, F, Told<'id>>),
    /// The one end of the open request: to the controller, and go on.
    Ended(Ended<'id, F>, Resume<'m, 'id, C, F, Told<'id>>),
    /// To the controller: the run is interrupted, and go on.
    Interrupted(Resume<'m, 'id, C, F, Told<'id>>),
    /// What is waited for changed: [`Resume::waiting`] reads it.
    Held(Resume<'m, 'id, C, F, Said<'id>>),
    /// The step is over, and what comes next.
    Rest(Rest),
}

/// The port that holds an installation, held until the ask `K` is answered
/// or the message passed.
#[must_use = "a step goes on only through its resume"]
pub struct Resume<'m, 'id, C, F, K> {
    port: Port<'m, 'id, C, F>,
    now: Instant,
    kind: K,
}

/// The whole machine, held while its release is asked.
#[must_use = "a step goes on only through its resume"]
pub struct Releasing<'m, 'id, C, F> {
    machine: &'m mut Machine<'id, C, F>,
    now: Instant,
}

/// A start is asked.
pub struct AskStart<C> {
    channel: C,
    run: Run,
}

/// A stop is polled.
pub struct AskStop {
    after: AfterStop,
    run: Run,
}

/// The data's stop is asked.
pub struct AskStopData<'id> {
    before: DataBefore<'id>,
    run: Run,
}

/// What the data's stop is asked before.
enum DataBefore<'id> {
    /// The pass towards the goal of the stopped installation.
    Goal(Stopped<'id>),
    /// The radio's stop of an ending: its first poll follows.
    Stop,
    /// The recovery of a fault that ended a run.
    Recovery,
}

/// The cleanup is asked.
pub struct AskClean {
    run: Run,
}

/// A retirement is asked.
pub struct AskRetire {
    run: Run,
}

/// A reinstall is asked.
pub struct AskReinstall(());

/// A recovery is asked.
pub struct AskRecover {
    after: AfterRecover,
}

/// A message is passed on; the step goes on towards the goal.
pub struct Told<'id> {
    next: Next<'id>,
}

/// A status is said; the step goes on as its order says.
pub struct Said<'id> {
    next: Next<'id>,
}

/// The machine of one port's installations. Built in its place, reached by
/// `&mut`.
pub struct Machine<'id, C, F> {
    life: Life<'id, C>,
    goal: Goal<C>,
    waiting: Waiting<F>,
    retry: Duration,
}

/// The port's installations, and the stop's right while they last.
enum Life<'id, C> {
    /// The port holds an installation; the machine holds the stop's right.
    Port {
        right: StopRight<'id>,
        at: At<'id, C>,
    },
    /// A release is asked: the right is out with the proof.
    Releasing,
    /// The port is released.
    Released,
}

/// A view of a port that holds an installation: the stop's right, the
/// position, the goal and what is waited for. Every function of a step but
/// the release's works on it.
struct Port<'m, 'id, C, F> {
    right: &'m mut StopRight<'id>,
    at: &'m mut At<'id, C>,
    goal: &'m mut Goal<C>,
    waiting: &'m mut Waiting<F>,
    retry: Duration,
}

/// The position of the installation, each holding the proof its name says.
enum At<'id, C> {
    Stopped(Stopped<'id>, Run),
    Starting(Starting<'id>, Run, C),
    /// A running radio's data flows and its drain is owed.
    Running(Tokens),
    /// Holds no proof: a stop gives one.
    Stopping(Run),
    /// A retired installation's data is stopped, its tokens ended and its
    /// drain done.
    Retired(Retired<'id>),
}

impl<C> At<'_, C> {
    /// The facts of the installation at this position: what stands in its
    /// place, as "stopping", while a step carries its proof out.
    const fn facts(&self) -> Run {
        match self {
            Self::Stopped(_, run) | Self::Starting(_, run, _) | Self::Stopping(run) => *run,
            Self::Running(tokens) => Run {
                data: Data::Flowing,
                tokens: *tokens,
                drain: Drain::Owed,
            },
            Self::Retired(_) => DONE,
        }
    }
}

/// The facts of an installation that are not its position.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Run {
    data: Data,
    tokens: Tokens,
    drain: Drain,
}

/// The facts of a retired installation, or of one whose proof was lost
/// after its retirement.
const DONE: Run = Run {
    data: Data::Stopped,
    tokens: Tokens::Ended,
    drain: Drain::Done,
};

/// The data plane.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Data {
    Flowing,
    Stopping,
    Stopped,
}

/// The installation's tokens with the controller.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Tokens {
    Held,
    Asked,
    Ended,
}

/// The cleanup a run leaves to the port.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Drain {
    Owed,
    Done,
}

/// The open request.
#[derive(Clone, Copy, Debug)]
enum Goal<C> {
    None,
    Open {
        want: Want<C>,
        deadline: Option<Instant>,
    },
}

impl<C> Goal<C> {
    const fn deadline(&self) -> Option<Instant> {
        match self {
            Self::Open { deadline, .. } => *deadline,
            Self::None => None,
        }
    }

    /// The rest `again` names, at `now`: a wake names the request's
    /// deadline; a timer the earlier of the deadline and the retry time.
    fn rest(&self, again: Again, now: Instant, retry: Duration) -> Rest {
        match again {
            Again::Wake => Rest::Wake(self.deadline()),
            Again::Round => Rest::Round,
            Again::Timer => {
                let retry = now.saturating_add(retry);
                Rest::Wake(Some(match self.deadline() {
                    Some(deadline) => deadline.min(retry),
                    None => retry,
                }))
            }
        }
    }
}

/// What the open request wants.
#[derive(Clone, Copy, Debug)]
enum Want<C> {
    Run(C),
    Halt,
    Restart,
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

/// How a step goes on after a message or a status.
enum Next<'id> {
    /// Towards the goal.
    Advance,
    /// The step rests.
    Rest(Again),
    /// The fault's step asks the recovery.
    RecoverForFault,
    /// The fault's step of a running radio: the data stops first, then
    /// the recovery.
    StopDataForFault(Run),
    /// After a recovery, as its ask said, the fault kept or gone.
    AfterRecovery(AfterRecover, Kept),
    /// The drain of the stopped installation, after its tokens were asked.
    Drain(Stopped<'id>, Run),
    /// The start's end, after the data was told it flows.
    EndStarted,
}

/// What comes next, before the instant is known.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Again {
    /// An input or a wake.
    Wake,
    /// The next round.
    Round,
    /// The retry time.
    Timer,
}

/// Where a stop's answer goes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AfterStop {
    /// The pass towards the goal goes on.
    Pass,
    /// The pass goes on, the data's stop asked in this step already.
    PassDataAsked,
    /// The fault's step: it rests.
    Fault,
}

/// Where a recovery's answer goes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AfterRecover {
    /// The fault's step: it polls the stop of a radio stopping.
    Fault,
    /// An answer that named a fault: the pass is made again.
    Again,
}

/// Whether the port keeps its fault.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Kept {
    Kept,
    Gone,
}

impl<'id, C: Copy, F> Machine<'id, C, F> {
    /// The machine of the port's first installation, stopped as `stopped`
    /// proves, with the stop's right the port gave beside it, written into
    /// `place`; and the controller's first permit. `retry` is the
    /// composition's interval for the answers that name nothing that wakes.
    pub fn place<'p>(
        place: &'p mut MaybeUninit<Self>,
        stopped: Stopped<'id>,
        right: StopRight<'id>,
        retry: Duration,
    ) -> (&'p mut Self, Live<'id>) {
        let machine = place.write(Self {
            life: Life::Port {
                right,
                at: At::Stopped(
                    stopped,
                    Run {
                        data: Data::Stopped,
                        tokens: Tokens::Held,
                        drain: Drain::Done,
                    },
                ),
            },
            goal: Goal::None,
            waiting: Waiting::Nothing,
            retry,
        });
        (machine, Live { brand: PhantomData })
    }

    /// What is waited for: a status, read at any time.
    pub const fn waiting(&self) -> &Waiting<F> {
        &self.waiting
    }

    /// One input, then towards the goal as far as the answers allow.
    ///
    /// Whether the port holds an installation is asked here, once a step:
    /// a released machine, and one whose release is asked, rest on every
    /// input. A retired installation's release, which takes the stop's
    /// right out of the machine, is asked here too, by a turn's step, where
    /// the machine is whole; everything else is the port's view.
    pub fn step(&mut self, input: Input<'id, C>, now: Instant) -> Step<'_, 'id, C, F> {
        if self.releases(&input, now) {
            return self.release(now);
        }
        let Self {
            life,
            goal,
            waiting,
            retry,
        } = self;
        match life {
            Life::Port { right, at } => Port {
                right,
                at,
                goal,
                waiting,
                retry: *retry,
            }
            .step(input, now),
            Life::Releasing | Life::Released => Step::Rest(goal.rest(Again::Wake, now, *retry)),
        }
    }

    /// Whether this step asks a retired installation's release: a turn's
    /// step alone asks it, at its beginning, when the goal releases with
    /// the network staying or stopped and no deadline passed. Every other
    /// input goes through the view, whose retired arm rests with a round.
    fn releases(&self, input: &Input<'id, C>, now: Instant) -> bool {
        let retired = matches!(
            self.life,
            Life::Port {
                at: At::Retired(_),
                ..
            }
        );
        let due = match self.goal {
            Goal::Open {
                want: Want::Release(NetworkStep::Stays | NetworkStep::Stopped),
                deadline,
            } => deadline.is_none_or(|deadline| now < deadline),
            Goal::Open { .. } | Goal::None => false,
        };
        retired && due && matches!(input, Input::Turn)
    }

    /// The release of a retired installation, which takes the right with
    /// the retirement's proof.
    fn release(&mut self, now: Instant) -> Step<'_, 'id, C, F> {
        match mem::replace(&mut self.life, Life::Releasing) {
            Life::Port {
                right,
                at: At::Retired(retired),
            } => Step::Release(retired, right, Releasing { machine: self, now }),
            // `releases` named a retired installation: anything else is
            // given back as it was, and the step rests with a round, so that
            // a turn reads the life again.
            life @ (Life::Port { .. } | Life::Releasing | Life::Released) => {
                self.life = life;
                Step::Rest(Rest::Round)
            }
        }
    }
}

impl<'m, 'id, C: Copy, F> Port<'m, 'id, C, F> {
    fn resume<K>(self, now: Instant, kind: K) -> Resume<'m, 'id, C, F, K> {
        Resume {
            port: self,
            now,
            kind,
        }
    }

    fn rest(&self, again: Again, now: Instant) -> Rest {
        self.goal.rest(again, now, self.retry)
    }

    /// Take the position's proof out for a step: "stopping", with the same
    /// facts, stands in its place.
    fn take(&mut self) -> At<'id, C> {
        let facts = self.at.facts();
        mem::replace(self.at, At::Stopping(facts))
    }

    /// The permit an end returns: what may be asked next.
    const fn permit(&self) -> Permit<'id> {
        match self.at.facts().tokens {
            Tokens::Held => match self.at {
                At::Retired(_) => Permit::Spent(Spent { brand: PhantomData }),
                At::Stopped(..) | At::Starting(..) | At::Running(_) | At::Stopping(_) => {
                    Permit::Live(Live { brand: PhantomData })
                }
            },
            Tokens::Asked | Tokens::Ended => Permit::Spent(Spent { brand: PhantomData }),
        }
    }

    /// The open request ends.
    fn end(
        self,
        result: Result<End, Unreached<F>>,
        next: Next<'id>,
        now: Instant,
    ) -> Step<'m, 'id, C, F> {
        *self.goal = Goal::None;
        let permit = self.permit();
        Step::Ended(
            Ended::Request(result, permit),
            self.resume(now, Told { next }),
        )
    }

    /// What is waited for, said when its class changed; then `next`.
    fn say(self, waiting: Waiting<F>, next: Next<'id>, now: Instant) -> Step<'m, 'id, C, F> {
        let changed = self.waiting.class() != waiting.class();
        *self.waiting = waiting;
        if changed {
            Step::Held(self.resume(now, Said { next }))
        } else {
            self.go(next, now)
        }
    }

    /// Clear what is waited for when it is of `class`.
    fn clear(&mut self, class: Waited) {
        if self.waiting.class() == class {
            *self.waiting = Waiting::Nothing;
        }
    }

    fn go(self, next: Next<'id>, now: Instant) -> Step<'m, 'id, C, F> {
        match next {
            Next::Advance => self.advance(now),
            Next::Rest(again) => Step::Rest(self.rest(again, now)),
            Next::RecoverForFault => self.recover(AfterRecover::Fault, now),
            Next::StopDataForFault(run) => Step::StopData(self.resume(
                now,
                AskStopData {
                    before: DataBefore::Recovery,
                    run,
                },
            )),
            Next::AfterRecovery(after, kept) => self.after_recovery(after, kept, now),
            Next::Drain(stopped, run) => self.drain(stopped, run, now),
            Next::EndStarted => self.end(Ok(End::Started), Next::Rest(Again::Wake), now),
        }
    }

    fn step(self, input: Input<'id, C>, now: Instant) -> Step<'m, 'id, C, F> {
        match input {
            Input::Request(request) => self.request(request, now),
            Input::Turn => match *self.goal {
                Goal::Open {
                    deadline: Some(deadline),
                    ..
                } if deadline <= now => self.deadline(now),
                Goal::Open { .. } | Goal::None => self.advance(now),
            },
            Input::Fault => self.fault(now),
            Input::TokensEnded(TokensEnded { brand: _ }) => {
                match self.at {
                    At::Stopped(_, run) | At::Starting(_, run, _) | At::Stopping(run) => {
                        run.tokens = Tokens::Ended;
                    }
                    At::Running(tokens) => *tokens = Tokens::Ended,
                    // A retired installation's tokens are ended.
                    At::Retired(_) => {}
                }
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

    fn request(self, request: Request<'id, C>, now: Instant) -> Step<'m, 'id, C, F> {
        let Request {
            aim,
            deadline,
            brand: _,
        } = request;
        let want = match aim {
            Aim::Run(channel) => Want::Run(channel),
            Aim::Halt => Want::Halt,
            Aim::Restart => Want::Restart,
            Aim::Release(Network::Stays) => Want::Release(NetworkStep::Stays),
            Aim::Release(Network::Stops) => Want::Release(NetworkStep::Told),
        };
        *self.goal = Goal::Open { want, deadline };
        // Every request stops a running radio: a run on another channel is
        // a retune, and a run on the same one is answered by the start.
        if let At::Running(tokens) = *self.at {
            *self.at = At::Stopping(Run {
                data: Data::Flowing,
                tokens,
                drain: Drain::Owed,
            });
        }
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
            Aim::Release(Network::Stays) | Aim::Run(_) | Aim::Halt | Aim::Restart => {
                self.advance(now)
            }
        }
    }

    /// The open request's deadline passed: it ends with what is waited
    /// for, and a start under way is stopped.
    fn deadline(self, now: Instant) -> Step<'m, 'id, C, F> {
        if let At::Starting(_, run, _) = *self.at {
            // The start's proof is dropped: the stop gives the next one.
            *self.at = At::Stopping(run);
        }
        let waited = self.waiting.class();
        self.end(Err(Unreached::Deadline(waited)), Next::Advance, now)
    }

    /// The port holds a fault: it ends a run and a start, by a stop, and
    /// the machine asks what a stop can do about it.
    fn fault(self, now: Instant) -> Step<'m, 'id, C, F> {
        match *self.at {
            At::Starting(_, run, _) => {
                *self.at = At::Stopping(run);
                self.end(Err(Unreached::StartInterrupted), Next::RecoverForFault, now)
            }
            At::Running(tokens) => {
                // The radio leaves "running": the data stops first.
                let run = Run {
                    data: Data::Flowing,
                    tokens,
                    drain: Drain::Owed,
                };
                *self.at = At::Stopping(run);
                Step::Interrupted(self.resume(
                    now,
                    Told {
                        next: Next::StopDataForFault(run),
                    },
                ))
            }
            At::Stopped(..) | At::Stopping(_) | At::Retired(_) => {
                self.recover(AfterRecover::Fault, now)
            }
        }
    }

    /// Ask what a stop can do about the port's fault.
    fn recover(self, after: AfterRecover, now: Instant) -> Step<'m, 'id, C, F> {
        Step::Recover(self.resume(now, AskRecover { after }))
    }

    fn after_recovery(self, after: AfterRecover, kept: Kept, now: Instant) -> Step<'m, 'id, C, F> {
        let again = match kept {
            Kept::Kept => Again::Wake,
            Kept::Gone => Again::Round,
        };
        match (after, &*self.at) {
            // The fault's step polls the stop of a radio stopping once.
            (AfterRecover::Fault, At::Stopping(run)) => {
                let run = *run;
                Step::Stop(self.resume(
                    now,
                    AskStop {
                        after: AfterStop::Fault,
                        run,
                    },
                ))
            }
            (
                AfterRecover::Fault,
                At::Stopped(..) | At::Starting(..) | At::Running(_) | At::Retired(_),
            )
            | (AfterRecover::Again, _) => Step::Rest(self.rest(again, now)),
        }
    }

    /// A proof the port did not take is gone: a stop gives one again.
    fn lost(self, run: Run, now: Instant) -> Step<'m, 'id, C, F> {
        *self.at = At::Stopping(run);
        Step::Rest(self.rest(Again::Round, now))
    }

    /// Towards the goal, as far as the answers allow.
    fn advance(mut self, now: Instant) -> Step<'m, 'id, C, F> {
        match self.take() {
            At::Running(tokens) => {
                *self.at = At::Running(tokens);
                Step::Rest(self.rest(Again::Wake, now))
            }
            At::Starting(starting, run, channel) => Step::Start(
                StartProof::Starting(starting),
                channel,
                self.resume(now, AskStart { channel, run }),
            ),
            At::Stopping(run) => {
                // An ending stops the data before the radio: the data
                // plane publishes nothing into a radio that stops. A
                // retune is the one move with the data flowing, under the
                // gate its caller closed.
                if self.ending() && run.data == Data::Flowing {
                    *self.at = At::Stopping(run);
                    Step::StopData(self.resume(
                        now,
                        AskStopData {
                            before: DataBefore::Stop,
                            run,
                        },
                    ))
                } else {
                    Step::Stop(self.resume(
                        now,
                        AskStop {
                            after: AfterStop::Pass,
                            run,
                        },
                    ))
                }
            }
            At::Stopped(stopped, run) => self.stopped(stopped, run, now),
            At::Retired(retired) => self.retired(retired, now),
        }
    }

    /// Whether the open request ends the run: a halt, a restart or a
    /// release.
    fn ending(&self) -> bool {
        match *self.goal {
            Goal::Open {
                want: Want::Halt | Want::Restart | Want::Release(_),
                ..
            } => true,
            Goal::Open {
                want: Want::Run(_), ..
            }
            | Goal::None => false,
        }
    }

    /// A stopped installation, its proof out of the machine: the data first.
    fn stopped(self, stopped: Stopped<'id>, run: Run, now: Instant) -> Step<'m, 'id, C, F> {
        if run.data == Data::Stopping || (self.ending() && run.data == Data::Flowing) {
            Step::StopData(self.resume(
                now,
                AskStopData {
                    before: DataBefore::Goal(stopped),
                    run,
                },
            ))
        } else {
            self.toward(stopped, run, now)
        }
    }

    /// A stopped installation towards the goal, the data asked.
    fn toward(self, stopped: Stopped<'id>, run: Run, now: Instant) -> Step<'m, 'id, C, F> {
        match *self.goal {
            Goal::None => {
                *self.at = At::Stopped(stopped, run);
                Step::Rest(self.rest(Again::Wake, now))
            }
            Goal::Open {
                want: Want::Run(channel),
                ..
            } => {
                if run.data == Data::Stopping {
                    *self.at = At::Stopped(stopped, run);
                    Step::Rest(self.rest(Again::Wake, now))
                } else {
                    Step::Start(
                        StartProof::Stopped(stopped),
                        channel,
                        self.resume(now, AskStart { channel, run }),
                    )
                }
            }
            Goal::Open {
                want: Want::Halt, ..
            } => {
                *self.at = At::Stopped(stopped, run);
                if run.data == Data::Stopped {
                    self.end(Ok(End::Halted), Next::Rest(Again::Wake), now)
                } else {
                    Step::Rest(self.rest(Again::Wake, now))
                }
            }
            Goal::Open {
                want: Want::Restart | Want::Release(_),
                ..
            } => {
                if run.tokens == Tokens::Held {
                    let run = Run {
                        tokens: Tokens::Asked,
                        ..run
                    };
                    *self.at = At::Stopping(run);
                    Step::Tell(
                        Tell::EndTokens(Order { brand: PhantomData }),
                        self.resume(
                            now,
                            Told {
                                next: Next::Drain(stopped, run),
                            },
                        ),
                    )
                } else {
                    self.drain(stopped, run, now)
                }
            }
        }
    }

    /// The cleanup a run leaves to the port, then the retirement. The
    /// sink takes everything the cleanup frees: nothing waits with the
    /// port to be taken before or after it.
    fn drain(self, stopped: Stopped<'id>, run: Run, now: Instant) -> Step<'m, 'id, C, F> {
        match run.drain {
            Drain::Owed => Step::Clean(stopped, self.resume(now, AskClean { run })),
            Drain::Done => self.retire(stopped, run, now),
        }
    }

    /// The retirement, asked only with the data stopped, the tokens ended
    /// and the drain done.
    fn retire(self, stopped: Stopped<'id>, run: Run, now: Instant) -> Step<'m, 'id, C, F> {
        if run.data == Data::Stopped && run.tokens == Tokens::Ended && run.drain == Drain::Done {
            *self.at = At::Stopping(run);
            Step::Retire(stopped, self.resume(now, AskRetire { run }))
        } else {
            *self.at = At::Stopped(stopped, run);
            Step::Rest(self.rest(Again::Wake, now))
        }
    }

    /// A retired installation towards the goal, its proof out of the
    /// machine. Its release is decided at a step's beginning
    /// ([`Machine::step`]), where the machine is whole.
    fn retired(self, retired: Retired<'id>, now: Instant) -> Step<'m, 'id, C, F> {
        match *self.goal {
            Goal::Open {
                want: Want::Restart,
                ..
            } => Step::Reinstall(retired, self.resume(now, AskReinstall(()))),
            // A release is asked by a turn's step, at its beginning: the
            // round is that turn.
            Goal::Open {
                want: Want::Release(NetworkStep::Stays | NetworkStep::Stopped),
                ..
            } => {
                *self.at = At::Retired(retired);
                Step::Rest(self.rest(Again::Round, now))
            }
            // A release that stops the network waits for its stop; a
            // retired installation's permit asks no run and no halt.
            Goal::Open {
                want: Want::Release(NetworkStep::Told) | Want::Run(_) | Want::Halt,
                ..
            }
            | Goal::None => {
                *self.at = At::Retired(retired);
                Step::Rest(self.rest(Again::Wake, now))
            }
        }
    }
}

impl<'m, 'id, C: Copy, F> Resume<'m, 'id, C, F, AskStart<C>> {
    /// The port's answer to the start.
    pub fn answer(self, answer: StartPoll<'id, F>) -> Step<'m, 'id, C, F> {
        let Self {
            port,
            now,
            kind: AskStart { channel, run },
        } = self;
        match answer {
            Ok(StartProgress::Started) => {
                *port.at = At::Running(run.tokens);
                *port.goal = Goal::None;
                if run.data == Data::Stopped {
                    Step::Tell(
                        Tell::DataFlows,
                        port.resume(
                            now,
                            Told {
                                next: Next::EndStarted,
                            },
                        ),
                    )
                } else {
                    port.go(Next::EndStarted, now)
                }
            }
            Ok(StartProgress::Pending(starting)) => {
                *port.at = At::Starting(starting, run, channel);
                Step::Rest(port.rest(Again::Wake, now))
            }
            Err(StartRefused::Refused(stopped, failure)) => {
                *port.at = At::Stopped(stopped, run);
                port.end(
                    Err(Unreached::StartRefused(failure)),
                    Next::Rest(Again::Wake),
                    now,
                )
            }
            Err(StartRefused::Unstopped { cause, stop }) => port.end(
                Err(Unreached::StartUnstopped { cause, stop }),
                Next::Rest(Again::Round),
                now,
            ),
            Err(StartRefused::Stale(StaleProof { proof: _ })) => port.lost(run, now),
        }
    }
}

impl<'m, 'id, C: Copy, F> Resume<'m, 'id, C, F, AskStop> {
    /// The stop's right, lent for the port's stop.
    pub fn right(&mut self) -> &mut StopRight<'id> {
        &mut *self.port.right
    }

    /// The port's stop is not confirmed yet, and its waker is registered:
    /// the step rests until it wakes.
    pub fn pending(self) -> Step<'m, 'id, C, F> {
        let Self { port, now, .. } = self;
        Step::Rest(port.rest(Again::Wake, now))
    }

    /// The port's answer to the stop.
    pub fn answer(self, answer: Result<Stopped<'id>, StopUnconfirmed<F>>) -> Step<'m, 'id, C, F> {
        let Self {
            mut port,
            now,
            kind: AskStop { after, run },
        } = self;
        match answer {
            Err(unconfirmed) => {
                port.say(Waiting::Radio(unconfirmed), Next::Rest(Again::Timer), now)
            }
            Ok(stopped) => {
                port.clear(Waited::Radio);
                match after {
                    AfterStop::Pass => port.stopped(stopped, run, now),
                    AfterStop::PassDataAsked => port.toward(stopped, run, now),
                    AfterStop::Fault => {
                        *port.at = At::Stopped(stopped, run);
                        Step::Rest(port.rest(Again::Round, now))
                    }
                }
            }
        }
    }
}

impl<'m, 'id, C: Copy, F> Resume<'m, 'id, C, F, AskStopData<'id>> {
    /// The data plane's answer.
    pub fn answer(self, answer: DataStop) -> Step<'m, 'id, C, F> {
        let Self {
            port,
            now,
            kind: AskStopData { before, run },
        } = self;
        let run = Run {
            data: match answer {
                DataStop::Stopped => Data::Stopped,
                DataStop::Going => Data::Stopping,
            },
            ..run
        };
        *port.at = At::Stopping(run);
        match before {
            DataBefore::Goal(stopped) => port.toward(stopped, run, now),
            DataBefore::Stop => Step::Stop(port.resume(
                now,
                AskStop {
                    after: AfterStop::PassDataAsked,
                    run,
                },
            )),
            DataBefore::Recovery => port.recover(AfterRecover::Fault, now),
        }
    }
}

impl<'m, 'id, C: Copy, F> Resume<'m, 'id, C, F, AskClean> {
    /// The cleanup's answer, with the stop's proof it borrowed. A cleanup
    /// is always accepted; one that finished freed what it frees through
    /// the sink, and the drain is done.
    pub fn answer(
        self,
        stopped: Stopped<'id>,
        answer: Result<(), CleanupRefused<F>>,
    ) -> Step<'m, 'id, C, F> {
        let Self {
            mut port,
            now,
            kind: AskClean { run },
        } = self;
        match answer {
            Ok(()) => {
                port.clear(Waited::Cleanup);
                port.retire(
                    stopped,
                    Run {
                        drain: Drain::Done,
                        ..run
                    },
                    now,
                )
            }
            Err(CleanupRefused::Stale) => {
                drop(stopped);
                port.lost(run, now)
            }
            Err(CleanupRefused::Faulted(_)) => {
                *port.at = At::Stopped(stopped, run);
                port.recover(AfterRecover::Again, now)
            }
            Err(refused @ (CleanupRefused::Unavailable(_) | CleanupRefused::Held(_))) => {
                *port.at = At::Stopped(stopped, run);
                port.say(Waiting::Cleanup(refused), Next::Rest(Again::Timer), now)
            }
        }
    }
}

impl<'m, 'id, C: Copy, F> Resume<'m, 'id, C, F, AskRetire> {
    /// The port's answer to the retirement. A retirement granted ends its
    /// step with a round: the retired installation goes on from the next
    /// step's beginning.
    pub fn answer(
        self,
        answer: Result<Retired<'id>, ProofRefused<Stopped<'id>, Outstanding<F>>>,
    ) -> Step<'m, 'id, C, F> {
        let Self {
            mut port,
            now,
            kind: AskRetire { run },
        } = self;
        match answer {
            Ok(retired) => {
                port.clear(Waited::Outstanding);
                *port.at = At::Retired(retired);
                Step::Rest(port.rest(Again::Round, now))
            }
            Err(ProofRefused::Refused(stopped, outstanding)) => {
                *port.at = At::Stopped(stopped, run);
                match outstanding.fault {
                    Some(_) => port.recover(AfterRecover::Again, now),
                    None => port.say(
                        Waiting::Outstanding(outstanding),
                        Next::Rest(Again::Wake),
                        now,
                    ),
                }
            }
            Err(ProofRefused::Stale(StaleProof { proof })) => {
                drop(proof);
                port.lost(run, now)
            }
        }
    }
}

impl<'m, 'id, C: Copy, F> Resume<'m, 'id, C, F, AskReinstall> {
    /// The next installation, stopped as `stopped` proves: the end of the
    /// restart, for the controller with the next installation's
    /// permissions the port gave beside it, and the step's resume.
    pub fn installed(
        self,
        stopped: Stopped<'id>,
    ) -> (Ended<'id, F>, Resume<'m, 'id, C, F, Told<'id>>) {
        let Self { port, now, .. } = self;
        *port.at = At::Stopped(
            stopped,
            Run {
                data: Data::Stopped,
                tokens: Tokens::Held,
                drain: Drain::Done,
            },
        );
        *port.waiting = Waiting::Nothing;
        *port.goal = Goal::None;
        let permit = port.permit();
        (
            Ended::Request(Ok(End::Reinstalled), permit),
            port.resume(
                now,
                Told {
                    next: Next::Rest(Again::Wake),
                },
            ),
        )
    }

    /// The port's refusal of the reinstall.
    pub fn refused(
        self,
        refused: ProofRefused<Retired<'id>, ReinstallRefused>,
    ) -> Step<'m, 'id, C, F> {
        let Self { port, now, .. } = self;
        match refused {
            ProofRefused::Refused(retired, refused) => {
                *port.at = At::Retired(retired);
                port.end(
                    Err(Unreached::RestartRefused(refused)),
                    Next::Rest(Again::Wake),
                    now,
                )
            }
            ProofRefused::Stale(StaleProof { proof }) => {
                drop(proof);
                port.lost(DONE, now)
            }
        }
    }
}

impl<'m, 'id, C: Copy, F> Releasing<'m, 'id, C, F> {
    /// The port's answer to the release, without what the binding needs to
    /// give the radio up, which stays with the binding.
    pub fn answer(self, answer: Result<(), ReleaseRefused<'id>>) -> Step<'m, 'id, C, F> {
        let Self { machine, now } = self;
        match answer {
            Ok(()) => {
                machine.life = Life::Released;
                machine.goal = Goal::None;
                Step::Released(
                    Ended::Released,
                    machine.goal.rest(Again::Wake, now, machine.retry),
                )
            }
            Err(ReleaseRefused { proof, right }) => {
                // The proof is gone; the right came back with the refusal.
                drop(proof);
                machine.life = Life::Port {
                    right,
                    at: At::Stopping(DONE),
                };
                Step::Rest(Rest::Round)
            }
        }
    }
}

impl<'m, 'id, C: Copy, F> Resume<'m, 'id, C, F, AskRecover> {
    /// The stop's right, lent for the port's recovery.
    pub fn right(&mut self) -> &mut StopRight<'id> {
        &mut *self.port.right
    }

    /// The port's answer to the recovery.
    pub fn answer(self, answer: Result<Recovered, FaultKept<F>>) -> Step<'m, 'id, C, F> {
        let Self {
            mut port,
            now,
            kind: AskRecover { after },
        } = self;
        match answer {
            Err(kept) => port.say(
                Waiting::Fault(kept),
                Next::AfterRecovery(after, Kept::Kept),
                now,
            ),
            Ok(Recovered::Clear | Recovered::TakenBack) => {
                port.clear(Waited::Fault);
                port.after_recovery(after, Kept::Gone, now)
            }
        }
    }
}

impl<'m, 'id, C: Copy, F> Resume<'m, 'id, C, F, Told<'id>> {
    /// The message is passed on: the step goes on.
    pub fn go(self) -> Step<'m, 'id, C, F> {
        let Self {
            port,
            now,
            kind: Told { next },
        } = self;
        port.go(next, now)
    }
}

impl<'m, 'id, C: Copy, F> Resume<'m, 'id, C, F, Said<'id>> {
    /// What is waited for.
    pub const fn waiting(&self) -> &Waiting<F> {
        &*self.port.waiting
    }

    /// The status is said: the step goes on as its order says.
    pub fn go(self) -> Step<'m, 'id, C, F> {
        let Self {
            port,
            now,
            kind: Said { next },
        } = self;
        port.go(next, now)
    }
}
