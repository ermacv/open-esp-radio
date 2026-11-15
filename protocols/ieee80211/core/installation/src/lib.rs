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
use core::task::Poll;

use oer_portable_time::{Duration, Instant};
use oer_protocols_ieee80211_contracts_port::{
    CleanupRefused, FaultKept, Input as CallInput, Outstanding, ProofRefused, Recovered,
    ReinstallRefused, ReleaseRefused, Retired, StaleProof, StartFailure, StartPoll, StartProgress,
    StartProof, StartRefused, Starting, StopRight, StopUnconfirmed, Stopped, Unconsumed,
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
    /// The radio running on `channel`: a start from a stopped radio, a
    /// retune from a running one.
    pub const fn run(permit: Live<'id>, channel: C, deadline: Option<Instant>) -> Self {
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
    /// What a sink refused, which the port keeps.
    Unconsumed,
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
    /// What a sink refused.
    Unconsumed,
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
            Self::Unconsumed => Waited::Unconsumed,
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
    /// Poll the port's stop; [`Resume::poll`] lends the stop's right.
    Stop(Resume<'m, 'id, C, F, AskStop>),
    /// Ask the data plane to stop the data.
    StopData(Resume<'m, 'id, C, F, AskStopData<'id>>),
    /// Take what a sink refused, under the stop's proof, which comes back
    /// with the answer.
    TakeUnconsumed(Stopped<'id>, Resume<'m, 'id, C, F, AskTake>),
    /// Clean what the stop left, under the stop's proof, which comes back
    /// with the answer.
    Clean(Stopped<'id>, Resume<'m, 'id, C, F, AskClean>),
    /// The take after a cleanup that finished.
    TakeFreed(Stopped<'id>, Resume<'m, 'id, C, F, AskTakeFreed>),
    /// Retire the installation.
    Retire(Stopped<'id>, Resume<'m, 'id, C, F, AskRetire>),
    /// Reinstall the next installation.
    Reinstall(Retired<'id>, Resume<'m, 'id, C, F, AskReinstall>),
    /// Release the port: it takes the stop's right too, which a refusal
    /// gives back.
    Release(
        Retired<'id>,
        StopRight<'id>,
        Resume<'m, 'id, C, F, AskRelease>,
    ),
    /// Ask what a stop can do about the port's fault; [`Resume::recover`]
    /// lends the stop's right.
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

/// The machine, held until the ask `K` is answered or the message passed.
#[must_use = "a step goes on only through its resume"]
pub struct Resume<'m, 'id, C, F, K> {
    machine: &'m mut Machine<'id, C, F>,
    now: Instant,
    kind: K,
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
    stopped: Stopped<'id>,
    run: Run,
}

/// The unconsumed are asked.
pub struct AskTake {
    run: Run,
}

/// The cleanup is asked.
pub struct AskClean {
    run: Run,
}

/// The take after a cleanup that finished is asked.
pub struct AskTakeFreed {
    run: Run,
}

/// A retirement is asked.
pub struct AskRetire {
    run: Run,
}

/// A reinstall is asked.
pub struct AskReinstall(());

/// A release is asked.
pub struct AskRelease(());

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

/// What a run may have left unconsumed.
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
    pub fn step(&mut self, input: Input<'id, C>, now: Instant) -> Step<'_, 'id, C, F> {
        match input {
            Input::Request(request) => self.request(request, now),
            Input::Turn => match self.goal {
                Goal::Open {
                    deadline: Some(deadline),
                    ..
                } if deadline <= now => self.deadline(now),
                Goal::Open { .. } | Goal::None => self.advance(now),
            },
            Input::Fault => self.fault(now),
            Input::TokensEnded(TokensEnded { brand: _ }) => {
                match &mut self.life {
                    Life::Port {
                        at: At::Stopped(_, run) | At::Starting(_, run, _) | At::Stopping(run),
                        ..
                    } => run.tokens = Tokens::Ended,
                    Life::Port {
                        at: At::Running(tokens),
                        ..
                    } => *tokens = Tokens::Ended,
                    // A retired installation's tokens are ended; a released
                    // port holds none.
                    Life::Port {
                        at: At::Retired(_), ..
                    }
                    | Life::Releasing
                    | Life::Released => {}
                }
                self.advance(now)
            }
            Input::NetworkStopped => {
                if let Goal::Open {
                    want: Want::Release(network @ (NetworkStep::Told | NetworkStep::Stopped)),
                    ..
                } = &mut self.goal
                {
                    *network = NetworkStep::Stopped;
                }
                self.advance(now)
            }
        }
    }

    fn resume<K>(&mut self, now: Instant, kind: K) -> Resume<'_, 'id, C, F, K> {
        Resume {
            machine: self,
            now,
            kind,
        }
    }

    /// Put the installation at `at`. Only a step of a port that holds an
    /// installation puts one: a releasing or released machine has none to
    /// put, and what `at` holds is dropped.
    fn put(&mut self, at: At<'id, C>) {
        match &mut self.life {
            Life::Port { at: place, .. } => *place = at,
            Life::Releasing | Life::Released => {}
        }
    }

    const fn due(&self) -> Option<Instant> {
        match self.goal {
            Goal::Open { deadline, .. } => deadline,
            Goal::None => None,
        }
    }

    fn rest(&self, again: Again, now: Instant) -> Rest {
        match again {
            Again::Wake => Rest::Wake(self.due()),
            Again::Round => Rest::Round,
            Again::Timer => {
                let retry = now.saturating_add(self.retry);
                Rest::Wake(Some(match self.due() {
                    Some(deadline) => deadline.min(retry),
                    None => retry,
                }))
            }
        }
    }

    /// The permit an end returns: what may be asked next.
    const fn permit(&self) -> Permit<'id> {
        let tokens = match &self.life {
            Life::Port {
                at: At::Stopped(_, run) | At::Starting(_, run, _) | At::Stopping(run),
                ..
            } => run.tokens,
            Life::Port {
                at: At::Running(tokens),
                ..
            } => *tokens,
            // A retired installation's tokens are ended. A release's end
            // returns no permit and no request is open after it, so no end
            // that returns one comes from a released port.
            Life::Port {
                at: At::Retired(_), ..
            }
            | Life::Releasing
            | Life::Released => Tokens::Ended,
        };
        match tokens {
            Tokens::Held => Permit::Live(Live { brand: PhantomData }),
            Tokens::Asked | Tokens::Ended => Permit::Spent(Spent { brand: PhantomData }),
        }
    }

    /// The open request ends.
    fn end(
        &mut self,
        result: Result<End, Unreached<F>>,
        next: Next<'id>,
        now: Instant,
    ) -> Step<'_, 'id, C, F> {
        self.goal = Goal::None;
        let permit = self.permit();
        Step::Ended(
            Ended::Request(result, permit),
            self.resume(now, Told { next }),
        )
    }

    /// What is waited for, said when its class changed; then `next`.
    fn say(&mut self, waiting: Waiting<F>, next: Next<'id>, now: Instant) -> Step<'_, 'id, C, F> {
        let changed = self.waiting.class() != waiting.class();
        self.waiting = waiting;
        if changed {
            Step::Held(self.resume(now, Said { next }))
        } else {
            self.go(next, now)
        }
    }

    /// Clear what is waited for when it is of `class`.
    fn clear(&mut self, class: Waited) {
        if self.waiting.class() == class {
            self.waiting = Waiting::Nothing;
        }
    }

    fn go(&mut self, next: Next<'id>, now: Instant) -> Step<'_, 'id, C, F> {
        match next {
            Next::Advance => self.advance(now),
            Next::Rest(again) => Step::Rest(self.rest(again, now)),
            Next::RecoverForFault => Step::Recover(self.resume(
                now,
                AskRecover {
                    after: AfterRecover::Fault,
                },
            )),
            Next::AfterRecovery(after, kept) => self.after_recovery(after, kept, now),
            Next::Drain(stopped, run) => self.drain(stopped, run, now),
            Next::EndStarted => self.end(Ok(End::Started), Next::Rest(Again::Wake), now),
        }
    }

    fn request(&mut self, request: Request<'id, C>, now: Instant) -> Step<'_, 'id, C, F> {
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
        self.goal = Goal::Open { want, deadline };
        // Every request stops a running radio: a run on another channel is
        // a retune, and a run on the same one is answered by the start.
        if let Life::Port { at, .. } = &mut self.life
            && let At::Running(tokens) = *at
        {
            *at = At::Stopping(Run {
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
    fn deadline(&mut self, now: Instant) -> Step<'_, 'id, C, F> {
        if let Life::Port { at, .. } = &mut self.life
            && let At::Starting(_, run, _) = at
        {
            // The start's proof is dropped: the stop gives the next one.
            let run = *run;
            *at = At::Stopping(run);
        }
        let waited = self.waiting.class();
        self.end(Err(Unreached::Deadline(waited)), Next::Advance, now)
    }

    /// The port holds a fault: it ends a run and a start, by a stop, and
    /// the machine asks what a stop can do about it.
    fn fault(&mut self, now: Instant) -> Step<'_, 'id, C, F> {
        match &mut self.life {
            Life::Port { at, .. } => match at {
                At::Starting(_, run, _) => {
                    let run = *run;
                    *at = At::Stopping(run);
                    self.end(Err(Unreached::StartInterrupted), Next::RecoverForFault, now)
                }
                At::Running(tokens) => {
                    let tokens = *tokens;
                    *at = At::Stopping(Run {
                        data: Data::Flowing,
                        tokens,
                        drain: Drain::Owed,
                    });
                    Step::Interrupted(self.resume(
                        now,
                        Told {
                            next: Next::RecoverForFault,
                        },
                    ))
                }
                At::Stopped(..) | At::Stopping(_) | At::Retired(_) => {
                    self.go(Next::RecoverForFault, now)
                }
            },
            // A released port has no installation a fault could hold.
            Life::Releasing | Life::Released => Step::Rest(self.rest(Again::Wake, now)),
        }
    }

    fn after_recovery(
        &mut self,
        after: AfterRecover,
        kept: Kept,
        now: Instant,
    ) -> Step<'_, 'id, C, F> {
        let again = match kept {
            Kept::Kept => Again::Wake,
            Kept::Gone => Again::Round,
        };
        match after {
            AfterRecover::Again => Step::Rest(self.rest(again, now)),
            AfterRecover::Fault => match &self.life {
                // The fault's step polls the stop of a radio stopping once.
                Life::Port {
                    at: At::Stopping(run),
                    ..
                } => {
                    let run = *run;
                    Step::Stop(self.resume(
                        now,
                        AskStop {
                            after: AfterStop::Fault,
                            run,
                        },
                    ))
                }
                Life::Port {
                    at: At::Stopped(..) | At::Starting(..) | At::Running(_) | At::Retired(_),
                    ..
                }
                | Life::Releasing
                | Life::Released => Step::Rest(self.rest(again, now)),
            },
        }
    }

    /// A proof the port did not take is gone: a stop gives one again.
    fn lost(&mut self, run: Run, now: Instant) -> Step<'_, 'id, C, F> {
        self.put(At::Stopping(run));
        Step::Rest(self.rest(Again::Round, now))
    }

    /// Towards the goal, as far as the answers allow.
    fn advance(&mut self, now: Instant) -> Step<'_, 'id, C, F> {
        match mem::replace(&mut self.life, Life::Releasing) {
            Life::Port { right, at } => match at {
                At::Running(tokens) => {
                    self.life = Life::Port {
                        right,
                        at: At::Running(tokens),
                    };
                    Step::Rest(self.rest(Again::Wake, now))
                }
                At::Starting(starting, run, channel) => {
                    self.life = Life::Port {
                        right,
                        at: At::Stopping(run),
                    };
                    Step::Start(
                        StartProof::Starting(starting),
                        channel,
                        self.resume(now, AskStart { channel, run }),
                    )
                }
                At::Stopping(run) => {
                    self.life = Life::Port {
                        right,
                        at: At::Stopping(run),
                    };
                    Step::Stop(self.resume(
                        now,
                        AskStop {
                            after: AfterStop::Pass,
                            run,
                        },
                    ))
                }
                At::Stopped(stopped, run) => {
                    self.life = Life::Port {
                        right,
                        at: At::Stopping(run),
                    };
                    self.stopped(stopped, run, now)
                }
                At::Retired(retired) => self.retired(right, retired, now),
            },
            life @ (Life::Releasing | Life::Released) => {
                self.life = life;
                Step::Rest(self.rest(Again::Wake, now))
            }
        }
    }

    /// A stopped installation, its proof out of the machine: the data first.
    fn stopped(&mut self, stopped: Stopped<'id>, run: Run, now: Instant) -> Step<'_, 'id, C, F> {
        let ending = match self.goal {
            Goal::Open {
                want: Want::Halt | Want::Restart | Want::Release(_),
                ..
            } => run.data == Data::Flowing,
            Goal::Open {
                want: Want::Run(_), ..
            }
            | Goal::None => false,
        };
        if run.data == Data::Stopping || ending {
            Step::StopData(self.resume(now, AskStopData { stopped, run }))
        } else {
            self.toward(stopped, run, now)
        }
    }

    /// A stopped installation towards the goal, the data asked.
    fn toward(&mut self, stopped: Stopped<'id>, run: Run, now: Instant) -> Step<'_, 'id, C, F> {
        match self.goal {
            Goal::None => {
                self.put(At::Stopped(stopped, run));
                Step::Rest(self.rest(Again::Wake, now))
            }
            Goal::Open {
                want: Want::Run(channel),
                ..
            } => {
                if run.data == Data::Stopping {
                    self.put(At::Stopped(stopped, run));
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
                self.put(At::Stopped(stopped, run));
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
                    self.put(At::Stopping(run));
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

    /// What a run may have left unconsumed, then the retirement.
    fn drain(&mut self, stopped: Stopped<'id>, run: Run, now: Instant) -> Step<'_, 'id, C, F> {
        match run.drain {
            Drain::Owed => Step::TakeUnconsumed(stopped, self.resume(now, AskTake { run })),
            Drain::Done => self.retire(stopped, run, now),
        }
    }

    /// The retirement, asked only with the data stopped, the tokens ended
    /// and the drain done.
    fn retire(&mut self, stopped: Stopped<'id>, run: Run, now: Instant) -> Step<'_, 'id, C, F> {
        if run.data == Data::Stopped && run.tokens == Tokens::Ended && run.drain == Drain::Done {
            self.put(At::Stopping(run));
            Step::Retire(stopped, self.resume(now, AskRetire { run }))
        } else {
            self.put(At::Stopped(stopped, run));
            Step::Rest(self.rest(Again::Wake, now))
        }
    }

    /// A retired installation towards the goal.
    fn retired(
        &mut self,
        right: StopRight<'id>,
        retired: Retired<'id>,
        now: Instant,
    ) -> Step<'_, 'id, C, F> {
        match self.goal {
            Goal::Open {
                want: Want::Restart,
                ..
            } => {
                self.life = Life::Port {
                    right,
                    at: At::Stopping(DONE),
                };
                Step::Reinstall(retired, self.resume(now, AskReinstall(())))
            }
            Goal::Open {
                want: Want::Release(NetworkStep::Stays | NetworkStep::Stopped),
                ..
            } => {
                self.life = Life::Releasing;
                Step::Release(retired, right, self.resume(now, AskRelease(())))
            }
            // A release that stops the network waits for its stop; a
            // retired installation's permit asks no run and no halt.
            Goal::Open {
                want: Want::Release(NetworkStep::Told) | Want::Run(_) | Want::Halt,
                ..
            }
            | Goal::None => {
                self.life = Life::Port {
                    right,
                    at: At::Retired(retired),
                };
                Step::Rest(self.rest(Again::Wake, now))
            }
        }
    }
}

impl<'m, 'id, C: Copy, F> Resume<'m, 'id, C, F, AskStart<C>> {
    /// The port's answer to the start.
    pub fn answer(self, answer: StartPoll<'id, F>) -> Step<'m, 'id, C, F> {
        let Self {
            machine,
            now,
            kind: AskStart { channel, run },
        } = self;
        match answer {
            Ok(StartProgress::Started) => {
                machine.put(At::Running(run.tokens));
                machine.goal = Goal::None;
                if run.data == Data::Stopped {
                    Step::Tell(
                        Tell::DataFlows,
                        machine.resume(
                            now,
                            Told {
                                next: Next::EndStarted,
                            },
                        ),
                    )
                } else {
                    machine.go(Next::EndStarted, now)
                }
            }
            Ok(StartProgress::Pending(starting)) => {
                machine.put(At::Starting(starting, run, channel));
                Step::Rest(machine.rest(Again::Wake, now))
            }
            Err(StartRefused::Refused(stopped, failure)) => {
                machine.put(At::Stopped(stopped, run));
                machine.end(
                    Err(Unreached::StartRefused(failure)),
                    Next::Rest(Again::Wake),
                    now,
                )
            }
            Err(StartRefused::Unstopped { cause, stop }) => machine.end(
                Err(Unreached::StartUnstopped { cause, stop }),
                Next::Rest(Again::Round),
                now,
            ),
            Err(StartRefused::Stale(StaleProof { proof: _ })) => machine.lost(run, now),
        }
    }
}

impl<'m, 'id, C: Copy, F> Resume<'m, 'id, C, F, AskStop> {
    /// Poll the port's stop with the stop's right `poll` is lent.
    pub fn poll(
        self,
        poll: impl FnOnce(&mut StopRight<'id>) -> Poll<Result<Stopped<'id>, StopUnconfirmed<F>>>,
    ) -> Step<'m, 'id, C, F> {
        let Self {
            machine,
            now,
            kind: AskStop { after, run },
        } = self;
        let answer = match &mut machine.life {
            Life::Port { right, .. } => poll(right),
            // A stop is asked only of a port that holds an installation.
            Life::Releasing | Life::Released => Poll::Pending,
        };
        match answer {
            Poll::Pending => Step::Rest(machine.rest(Again::Wake, now)),
            Poll::Ready(Err(unconfirmed)) => {
                machine.say(Waiting::Radio(unconfirmed), Next::Rest(Again::Timer), now)
            }
            Poll::Ready(Ok(stopped)) => {
                machine.clear(Waited::Radio);
                match after {
                    AfterStop::Pass => machine.stopped(stopped, run, now),
                    AfterStop::Fault => {
                        machine.put(At::Stopped(stopped, run));
                        Step::Rest(machine.rest(Again::Round, now))
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
            machine,
            now,
            kind: AskStopData { stopped, run },
        } = self;
        let run = Run {
            data: match answer {
                DataStop::Stopped => Data::Stopped,
                DataStop::Going => Data::Stopping,
            },
            ..run
        };
        machine.put(At::Stopping(run));
        machine.toward(stopped, run, now)
    }
}

impl<'m, 'id, C: Copy, F> Resume<'m, 'id, C, F, AskTake> {
    /// The port's answer, with the stop's proof it borrowed.
    pub fn answer(
        self,
        stopped: Stopped<'id>,
        answer: Result<Unconsumed, StaleProof<()>>,
    ) -> Step<'m, 'id, C, F> {
        let Self {
            machine,
            now,
            kind: AskTake { run },
        } = self;
        match answer {
            Ok(Unconsumed::None) => Step::Clean(stopped, machine.resume(now, AskClean { run })),
            Ok(Unconsumed::Kept) => {
                machine.put(At::Stopped(stopped, run));
                machine.say(Waiting::Unconsumed, Next::Rest(Again::Wake), now)
            }
            Err(StaleProof { proof: () }) => {
                drop(stopped);
                machine.lost(run, now)
            }
        }
    }
}

impl<'m, 'id, C: Copy, F> Resume<'m, 'id, C, F, AskClean> {
    /// The port's answer, without an item it handed back, with the stop's
    /// proof it borrowed: the cleanup's own answer, or that it found the
    /// sink full and cleaned nothing.
    pub fn answer(
        self,
        stopped: Stopped<'id>,
        answer: CallInput<(), Result<(), CleanupRefused<F>>>,
    ) -> Step<'m, 'id, C, F> {
        let Self {
            machine,
            now,
            kind: AskClean { run },
        } = self;
        match answer {
            CallInput::Accepted(Ok(())) => {
                Step::TakeFreed(stopped, machine.resume(now, AskTakeFreed { run }))
            }
            CallInput::NotAccepted(()) => {
                machine.put(At::Stopped(stopped, run));
                machine.say(Waiting::Unconsumed, Next::Rest(Again::Wake), now)
            }
            CallInput::Accepted(Err(CleanupRefused::Stale)) => {
                drop(stopped);
                machine.lost(run, now)
            }
            CallInput::Accepted(Err(CleanupRefused::Faulted(_))) => {
                machine.put(At::Stopped(stopped, run));
                Step::Recover(machine.resume(
                    now,
                    AskRecover {
                        after: AfterRecover::Again,
                    },
                ))
            }
            CallInput::Accepted(Err(
                refused @ (CleanupRefused::Unavailable(_) | CleanupRefused::Held(_)),
            )) => {
                machine.put(At::Stopped(stopped, run));
                machine.say(Waiting::Cleanup(refused), Next::Rest(Again::Timer), now)
            }
        }
    }
}

impl<'m, 'id, C: Copy, F> Resume<'m, 'id, C, F, AskTakeFreed> {
    /// The port's answer, with the stop's proof it borrowed: the drain is
    /// done only when it keeps nothing of what the cleanup freed.
    pub fn answer(
        self,
        stopped: Stopped<'id>,
        answer: Result<Unconsumed, StaleProof<()>>,
    ) -> Step<'m, 'id, C, F> {
        let Self {
            machine,
            now,
            kind: AskTakeFreed { run },
        } = self;
        match answer {
            Ok(Unconsumed::None) => {
                machine.clear(Waited::Unconsumed);
                machine.clear(Waited::Cleanup);
                machine.retire(
                    stopped,
                    Run {
                        drain: Drain::Done,
                        ..run
                    },
                    now,
                )
            }
            Ok(Unconsumed::Kept) => {
                machine.put(At::Stopped(stopped, run));
                machine.say(Waiting::Unconsumed, Next::Rest(Again::Wake), now)
            }
            Err(StaleProof { proof: () }) => {
                drop(stopped);
                machine.lost(run, now)
            }
        }
    }
}

impl<'m, 'id, C: Copy, F> Resume<'m, 'id, C, F, AskRetire> {
    /// The port's answer to the retirement.
    pub fn answer(
        self,
        answer: Result<Retired<'id>, ProofRefused<Stopped<'id>, Outstanding<F>>>,
    ) -> Step<'m, 'id, C, F> {
        let Self {
            machine,
            now,
            kind: AskRetire { run },
        } = self;
        match answer {
            Ok(retired) => {
                machine.clear(Waited::Outstanding);
                match mem::replace(&mut machine.life, Life::Releasing) {
                    Life::Port { right, at: _ } => machine.retired(right, retired, now),
                    // A retirement is asked only of a port that holds an
                    // installation.
                    life @ (Life::Releasing | Life::Released) => {
                        machine.life = life;
                        drop(retired);
                        Step::Rest(machine.rest(Again::Wake, now))
                    }
                }
            }
            Err(ProofRefused::Refused(stopped, outstanding)) => {
                machine.put(At::Stopped(stopped, run));
                match outstanding.fault {
                    Some(_) => Step::Recover(machine.resume(
                        now,
                        AskRecover {
                            after: AfterRecover::Again,
                        },
                    )),
                    None => machine.say(
                        Waiting::Outstanding(outstanding),
                        Next::Rest(Again::Wake),
                        now,
                    ),
                }
            }
            Err(ProofRefused::Stale(StaleProof { proof })) => {
                drop(proof);
                machine.lost(run, now)
            }
        }
    }
}

impl<'m, 'id, C: Copy, F> Resume<'m, 'id, C, F, AskReinstall> {
    /// The port's answer to the reinstall, without the next installation's
    /// permissions, which go to the controller with the end.
    pub fn answer(
        self,
        answer: Result<Stopped<'id>, ProofRefused<Retired<'id>, ReinstallRefused>>,
    ) -> Step<'m, 'id, C, F> {
        let Self {
            machine,
            now,
            kind: AskReinstall(()),
        } = self;
        match answer {
            Ok(stopped) => {
                machine.put(At::Stopped(
                    stopped,
                    Run {
                        data: Data::Stopped,
                        tokens: Tokens::Held,
                        drain: Drain::Done,
                    },
                ));
                machine.waiting = Waiting::Nothing;
                machine.end(Ok(End::Reinstalled), Next::Rest(Again::Wake), now)
            }
            Err(ProofRefused::Refused(retired, refused)) => {
                machine.put(At::Retired(retired));
                machine.end(
                    Err(Unreached::RestartRefused(refused)),
                    Next::Rest(Again::Wake),
                    now,
                )
            }
            Err(ProofRefused::Stale(StaleProof { proof })) => {
                drop(proof);
                machine.lost(DONE, now)
            }
        }
    }
}

impl<'m, 'id, C: Copy, F> Resume<'m, 'id, C, F, AskRelease> {
    /// The port's answer to the release, without what the binding needs to
    /// give the radio up, which stays with the binding.
    pub fn answer(self, answer: Result<(), ReleaseRefused<'id>>) -> Step<'m, 'id, C, F> {
        let Self {
            machine,
            now,
            kind: AskRelease(()),
        } = self;
        match answer {
            Ok(()) => {
                machine.life = Life::Released;
                machine.goal = Goal::None;
                Step::Ended(
                    Ended::Released,
                    machine.resume(
                        now,
                        Told {
                            next: Next::Rest(Again::Wake),
                        },
                    ),
                )
            }
            Err(ReleaseRefused { proof, right }) => {
                // The proof is gone; the right came back with the refusal.
                drop(proof);
                machine.life = Life::Port {
                    right,
                    at: At::Stopping(DONE),
                };
                Step::Rest(machine.rest(Again::Round, now))
            }
        }
    }
}

impl<'m, 'id, C: Copy, F> Resume<'m, 'id, C, F, AskRecover> {
    /// Ask the port's recovery with the stop's right `recover` is lent.
    pub fn recover(
        self,
        recover: impl FnOnce(&mut StopRight<'id>) -> Result<Recovered, FaultKept<F>>,
    ) -> Step<'m, 'id, C, F> {
        let Self {
            machine,
            now,
            kind: AskRecover { after },
        } = self;
        let answer = match &mut machine.life {
            Life::Port { right, .. } => recover(right),
            // A recovery is asked only of a port that holds an installation.
            Life::Releasing | Life::Released => Ok(Recovered::Clear),
        };
        match answer {
            Err(kept) => machine.say(
                Waiting::Fault(kept),
                Next::AfterRecovery(after, Kept::Kept),
                now,
            ),
            Ok(Recovered::Clear | Recovered::TakenBack) => {
                machine.clear(Waited::Fault);
                machine.after_recovery(after, Kept::Gone, now)
            }
        }
    }
}

impl<'m, 'id, C: Copy, F> Resume<'m, 'id, C, F, Told<'id>> {
    /// The message is passed on: the step goes on.
    pub fn go(self) -> Step<'m, 'id, C, F> {
        let Self {
            machine,
            now,
            kind: Told { next },
        } = self;
        machine.go(next, now)
    }
}

impl<'m, 'id, C: Copy, F> Resume<'m, 'id, C, F, Said<'id>> {
    /// What is waited for.
    pub const fn waiting(&self) -> &Waiting<F> {
        &self.machine.waiting
    }

    /// The status is said: the step goes on as its order says.
    pub fn go(self) -> Step<'m, 'id, C, F> {
        let Self {
            machine,
            now,
            kind: Said { next },
        } = self;
        machine.go(next, now)
    }
}
