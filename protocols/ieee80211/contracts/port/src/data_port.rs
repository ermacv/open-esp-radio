//! The data port: the radio as the station's datapath actor owns it.
//!
//! One task, the actor, owns a [`DataPort`] by value for an installation's
//! whole life and calls it in its turns. Nothing else borrows the port:
//! the station's lifecycle sends each key, configuration, gate and setting
//! operation as a [`PortCommand`] message, which the actor hands to
//! [`DataPort::command`] in order.
//!
//! # Turns
//!
//! The actor waits on the port's signals ([`DataPort::poll_facts`]) and one
//! alarm at [`DataPort::deadline`]. A turn reads the clock once and passes
//! that `now` to every call of the turn. No method of the port wakes its
//! caller's task, reads the clock or loops over a queue: what the caller
//! must do again it learns from the value a call returns.
//!
//! - [`DataPort::service_tx`] serves the sampled transmit facts (or a due
//!   deadline): single-MPDU terminals and TBTT go to the turn's
//!   [`PortSink`]; a data arena's end comes back to the caller directly.
//! - [`DataPort::receive`] gives one received unit, at most `room` held at
//!   once, or says when the ring has more.
//! - [`DataPort::command`] runs one command, or holds it at its barrier.
//! - [`DataPort::poll_start`] and [`DataPort::poll_stop`] move the radio
//!   between stopped and running within their bounds.
//!
//! # Errors
//!
//! A refusal ([`CommandProgress::Refused`], [`SubmitRefusal`] with the
//! attempt's withdrawn terminal in the sink,
//! [`LendRefused`](crate::LendRefused), [`StartRefused`]) changed nothing
//! and returns every owner. [`DataPort::Fault`] names a broken contract of
//! the backend's own state; it keeps its observations, and the backend
//! keeps the owners it holds. A command the port did not take comes back
//! beside its fault ([`CommandFailed::Returned`]).
//!
//! # Rates
//!
//! A rate is checked against the port's channel, which only a start
//! changes. [`DataPort::rate`] lends a [`PortRate`] for the borrow of the
//! port, so a rate checked in one turn is not used after the port moved
//! on:
//!
//! ```compile_fail
//! use oer_protocols_ieee80211_contracts_port::{DataPort, PhyRate, PortRate};
//! fn keep<'id, P: DataPort<'id>>(port: &P, rate: PhyRate) -> Option<PortRate<'static, P::Caps>> {
//!     port.rate(rate)
//! }
//! ```
//!
//! Its compiling twin uses the rate within the borrow:
//!
//! ```
//! use oer_protocols_ieee80211_contracts_port::{DataPort, PhyRate};
//! fn check<'id, P: DataPort<'id>>(port: &P, rate: PhyRate) -> Option<PhyRate> {
//!     port.rate(rate).map(|rate| rate.get())
//! }
//! ```

use core::task::{Context, Poll};

use oer_portable_time::Instant;
use oer_protocols_ieee80211_wire::address::IndividualAddress;
use oer_protocols_radio_port::ClockError;

use crate::{
    AdmittedAttempt, AmpduCapabilities, AmpduRate, Channel, CommandTypes, GateClosed, GateOpen,
    Ieee80211Capabilities, Ieee80211Instant, LendRefused, LowerMacTerminal, PhyRate,
    PortBlockAckSlot, PortCommand, PortRate, PortTypes, RecycleRefused, Station, SupportedBlockAck,
    SupportedChannel, TbttEvent,
    arena::{ArenaCompleted, TxArena},
};

/// The transmit events a port sampled: its interrupt word, or none for a
/// service that a deadline asks for.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct TxFacts {
    events: u32,
    arena_ended: bool,
}

impl TxFacts {
    /// No event: the service a due deadline asks for.
    pub const DUE: Self = Self {
        events: 0,
        arena_ended: false,
    };

    /// The backend's sampled event word `bits`.
    pub const fn events(bits: u32) -> Self {
        Self {
            events: bits,
            arena_ended: false,
        }
    }

    /// The event word.
    pub const fn bits(self) -> u32 {
        self.events
    }

    /// The events of both samples.
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self {
            events: self.events | other.events,
            arena_ended: self.arena_ended || other.arena_ended,
        }
    }

    /// These events, of which [`DataPort::service_arena`] already returned
    /// the published arena's end: the arena on the air now is the next one.
    #[must_use]
    pub const fn arena_ended(self) -> Self {
        Self {
            arena_ended: true,
            ..self
        }
    }

    /// Whether [`Self::arena_ended`] marked these events.
    pub const fn is_arena_ended(self) -> bool {
        self.arena_ended
    }
}

/// What the port's signals held when the actor polled them.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Facts {
    /// Transmit events to serve with [`DataPort::service_tx`].
    pub tx: Option<TxFacts>,
    /// A TBTT of the station's schedule, served with the transmit events.
    pub tbtt: bool,
    /// The receive ring has work for [`DataPort::receive`].
    pub rx: bool,
}

impl Facts {
    /// Whether the signals held nothing.
    pub const fn is_empty(&self) -> bool {
        self.tx.is_none() && !self.tbtt && !self.rx
    }
}

/// What one [`DataPort::receive`] gave.
pub enum RxPull<F, M> {
    /// One received MPDU, which the caller recycles or lends.
    Unit {
        /// The frame, from its header to the end of its body.
        frame: F,
        /// What the hardware observed of it.
        meta: M,
    },
    /// The ring holds nothing to take now; its next unit raises the
    /// receive signal.
    Empty,
    /// The ring has work only from this instant on, which the caller's
    /// alarm keeps; an instant not after the call's is work the call left
    /// at its own bound, which the caller takes after a repost.
    Later(Instant),
}

/// Where the port puts what a call produced besides its return value, in
/// the order it produced it: the turn's outbox.
///
/// A sink may refuse a terminal or a unit it has no room for now: it comes
/// back, and the port keeps it, with every owner and its outcome, in a
/// place of its own token. The port offers what it keeps again, oldest
/// first and before anything newer, at its next call with a sink while the
/// radio has not confirmed a stop, and after a confirmed stop only through
/// [`DataPort::take_unconsumed`]. The port signals nothing for what it
/// keeps: a sink that refuses registers its own wake for room, then checks
/// its room again, before its task sleeps, and its task then makes a call
/// with a sink.
pub trait PortSink<'id, T: PortTypes<'id>> {
    /// A command's or a single MPDU's terminal.
    ///
    /// # Errors
    ///
    /// The sink has no room for it now: the terminal, whole, which the port
    /// keeps.
    fn terminal(
        &mut self,
        terminal: LowerMacTerminal<'id, T>,
    ) -> Result<(), LowerMacTerminal<'id, T>>;
    /// A TBTT of the station's schedule.
    fn tbtt(&mut self, event: TbttEvent);
    /// A unit a group mutation's receive cut drained: it precedes the
    /// mutation's terminal.
    ///
    /// # Errors
    ///
    /// The sink has no room for it now: the unit and its metadata, which
    /// the port keeps, counted among the caller's held units.
    fn received(
        &mut self,
        frame: T::RxBuffer,
        meta: T::RxMeta,
    ) -> Result<(), (T::RxBuffer, T::RxMeta)>;
}

/// A terminal or a received unit a port call could neither give its sink
/// nor keep, whole: the call took no further owner after it, and what it
/// had not taken stays in its place for a later call.
pub enum Unkept<'id, T: PortTypes<'id>> {
    /// A terminal, with every owner it carries.
    Terminal(LowerMacTerminal<'id, T>),
    /// A received unit and its metadata, still owed to a recycle.
    Received(T::RxBuffer, T::RxMeta),
}

/// What became of a call's input beside an [`Unkept`].
pub enum Input<I, R> {
    /// The call found the unkept item before it accepted its input: the
    /// input, whole, with every owner it carries.
    NotAccepted(I),
    /// The call had accepted its input: its own answer.
    Accepted(R),
}

/// A port call that stopped on an [`Unkept`], and its input's state. A
/// separate error from the port's `Copy` fault: the port records no fault
/// for it.
#[must_use = "an unkept item and an unaccepted input hold owners"]
pub struct Unkeeping<'id, T: PortTypes<'id>, I, R> {
    /// The item the call could not keep.
    pub unkept: Unkept<'id, T>,
    /// The call's input, whole, or its answer.
    pub input: Input<I, R>,
}

/// Names the item, not its owners, which carry no `Debug`.
impl<'id, T: PortTypes<'id>> core::fmt::Debug for Unkept<'id, T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::Terminal(_) => "Unkept::Terminal",
            Self::Received(..) => "Unkept::Received",
        })
    }
}

/// Names the input's state, not its owners.
impl<I, R> core::fmt::Debug for Input<I, R> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::NotAccepted(_) => "Input::NotAccepted",
            Self::Accepted(_) => "Input::Accepted",
        })
    }
}

impl<'id, T: PortTypes<'id>, I, R> core::fmt::Debug for Unkeeping<'id, T, I, R> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Unkeeping")
            .field("unkept", &self.unkept)
            .field("input", &self.input)
            .finish()
    }
}

/// The answer of a producing port call with the input `I`: its own answer
/// `R`, or the [`Unkept`] it stopped on beside its input.
pub type PortAnswer<'id, T, I, R> = Result<R, Unkeeping<'id, T, I, R>>;

/// The answer of [`DataPort::service_tx`] of the port `P`. Its input is
/// the call itself (`()`): [`Input::NotAccepted`] says the call served
/// nothing, as it found an item an earlier call could not keep.
pub type ServiceAnswer<'id, P> = PortAnswer<
    'id,
    <P as DataPort<'id>>::Types,
    (),
    Result<Option<PortCompleted<'id, P>>, <P as DataPort<'id>>::Fault>,
>;

/// The answer of [`DataPort::command`] of the port `P`.
pub type CommandAnswer<'id, P> = PortAnswer<
    'id,
    <P as DataPort<'id>>::Types,
    PortCommand<'id, <P as DataPort<'id>>::Types, <P as DataPort<'id>>::Caps>,
    Result<
        CommandProgress<'id, <P as DataPort<'id>>::Types, <P as DataPort<'id>>::Caps>,
        PortCommandFailed<'id, P>,
    >,
>;

/// Why [`DataPort::submit`] gave no answer of its own: the [`Unkept`] it
/// stopped on beside its attempt or its answer, or its attempt, never
/// accepted, when the port had no right to hand out the attempt's
/// terminal.
#[must_use = "an unkept item and an unaccepted attempt hold owners"]
pub enum SubmitHalt<'id, T: PortTypes<'id>, A> {
    /// The call stopped on an item it could neither give its sink nor keep.
    Unkeeping(Unkeeping<'id, T, A, Result<(), SubmitRefusal>>),
    /// The port held no right to hand out the attempt's terminal, so it did
    /// not accept the attempt: the attempt, whole, with every owner it
    /// carries. Nothing ran and the port keeps nothing new.
    NotAccepted(A),
}

/// Names the halt, not its owners.
impl<'id, T: PortTypes<'id>, A> core::fmt::Debug for SubmitHalt<'id, T, A> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Unkeeping(unkeeping) => f.debug_tuple("Unkeeping").field(unkeeping).finish(),
            Self::NotAccepted(_) => f.write_str("SubmitHalt::NotAccepted"),
        }
    }
}

/// The answer of [`DataPort::submit`] with the attempt `A`: its own answer,
/// or why it gave none ([`SubmitHalt`]).
pub type SubmitAnswer<'id, T, A> = Result<Result<(), SubmitRefusal>, SubmitHalt<'id, T, A>>;

/// What [`DataPort::take_unconsumed`] left with the port.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Unconsumed {
    /// The port keeps nothing a sink refused.
    None,
    /// The sink refused the oldest again: it and everything newer stay
    /// with the port, in order.
    Kept,
}

/// Why [`DataPort::take_unconsumed`] offered nothing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnconsumedRefused {
    /// The radio has not confirmed a stop: the port offers what it keeps at
    /// its calls with a sink.
    NotStopped,
}

/// Why [`DataPort::clean_stopped`] did not finish the cleanup the stop
/// left to the port.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CleanupRefused<F> {
    /// The radio has not confirmed a stop: nothing was done.
    NotStopped,
    /// The port holds the backend's contract fault: the cleanup wrote
    /// nothing to the hardware, and the port does not retire.
    Faulted(F),
    /// The cleanup's next step cannot run now, for the backend's cause:
    /// that step wrote nothing, and a later call takes it.
    Unavailable(F),
    /// The cleanup's one hardware write was issued and the hardware still
    /// holds what that write releases, the backend's cause: the port keeps
    /// the owners it concerns and does not retire. A later call reads the
    /// hardware again and writes nothing; nothing repeats the write.
    Held(F),
}

/// How far [`DataPort::command`] took one command.
pub enum CommandProgress<'id, T: CommandTypes<'id>, K: Ieee80211Capabilities> {
    /// The command ran; its terminal, if it has one, is in the sink.
    Done,
    /// The command waits at its barrier; the caller runs it again, before
    /// any later command, on a later call.
    Held(PortCommand<'id, T, K>),
    /// The port cannot run the command in its state; nothing changed.
    Refused(PortCommand<'id, T, K>, CommandRefusal),
}

/// Why [`DataPort::command`] refused a command.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum CommandRefusal {
    /// The port holds an earlier command at its barrier.
    Busy,
    /// The gate is closed or closing.
    GateClosed,
    /// The station's receive configuration is in progress.
    Configuring,
    /// The backend cannot apply these EDCA parameters.
    EdcaUnsupported,
    /// The radio is not running.
    NotEnabled,
    /// A permission, association, key slot or installed key of the command
    /// was issued by an earlier installation of the port: final for that
    /// token.
    StalePermission {
        /// The token's installation.
        issued: u32,
        /// The port's installation.
        installation: u32,
    },
    /// The command waited at its barrier past the backend's command bound
    /// and did not run: what its barrier began (a key's removal barrier,
    /// a receive cut, a held configuration) is undone, attempts its
    /// barrier already withdrew keep their terminals, and the command
    /// comes back with every owner. Final for this issue: the caller
    /// issues it again or ends.
    Expired,
}

/// Why [`DataPort::command`] or
/// [`AccessPointPort::access_point_command`](crate::AccessPointPort::access_point_command)
/// failed: the port's fault, and the command when the port did not take it.
pub enum CommandFailed<C, F> {
    /// The port did not take the command's owners: they come back whole
    /// beside the fault, a port that faulted before the call included, so
    /// the port keeps no command for its quarantine. A step of the command
    /// may have begun before the fault, with an unknown end: the caller
    /// never issues it again automatically.
    Returned {
        /// The command, with every owner.
        command: C,
        /// The port's fault.
        fault: F,
    },
    /// The port took the command's owners before it faulted; they stay
    /// with the port's quarantine.
    Taken(F),
}

impl<C, F> CommandFailed<C, F> {
    /// The port's fault.
    pub const fn fault(&self) -> F
    where
        F: Copy,
    {
        match self {
            Self::Returned { fault, .. } | Self::Taken(fault) => *fault,
        }
    }

    /// The same failure with its fault mapped by `map`, the command kept
    /// where it was.
    pub fn map_fault<G>(self, map: impl FnOnce(F) -> G) -> CommandFailed<C, G> {
        match self {
            Self::Returned { command, fault } => CommandFailed::Returned {
                command,
                fault: map(fault),
            },
            Self::Taken(fault) => CommandFailed::Taken(map(fault)),
        }
    }
}

/// The fault, and whether the command came back; a command carries no
/// `Debug` of its own.
impl<C, F: core::fmt::Debug> core::fmt::Debug for CommandFailed<C, F> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Returned { fault, .. } => f.debug_tuple("Returned").field(fault).finish(),
            Self::Taken(fault) => f.debug_tuple("Taken").field(fault).finish(),
        }
    }
}

/// A failed [`DataPort::command`] of the port `P`.
pub type PortCommandFailed<'id, P> = CommandFailed<
    PortCommand<'id, <P as DataPort<'id>>::Types, <P as DataPort<'id>>::Caps>,
    <P as DataPort<'id>>::Fault,
>;

/// Why [`DataPort::submit`] refused an attempt.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SubmitRefusal {
    /// The radio is not running.
    NotEnabled,
    /// The radio is stopping.
    Stopping,
    /// The attempt's beacon permission or key was issued by an earlier
    /// installation of the port: final for that token. Its
    /// terminal ends it withdrawn for that cause
    /// ([`WithdrawCause::StalePermission`](crate::WithdrawCause::StalePermission)).
    StalePermission {
        /// The token's installation.
        issued: u32,
        /// The port's installation.
        installation: u32,
    },
}

/// Why [`DataPort::poll_start`] did not start the radio. `F` is the port's
/// [`DataPort::Fault`], which names why the front end did not tune. Except
/// for [`Self::Unstopped`], nothing runs: the port is stopped and can start
/// again.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum StartRefused<F> {
    /// The radio runs already, or its stop has not confirmed yet.
    Running,
    /// The radio front end did not tune the channel, for the port's cause.
    Retune(F),
    /// The start did not end within the backend's command bound.
    Deadline,
    /// The device refused the start.
    Device,
    /// The start failed for `cause` after it began on the device or the
    /// MAC, and the stop of what it began is not confirmed (`stop`): the
    /// port is stopping, not stopped. [`DataPort::poll_stop`] goes on with
    /// that stop, and the port refuses a start
    /// ([`Self::Running`]) until the stop confirms.
    Unstopped {
        /// Why the start failed.
        cause: StartFailure<F>,
        /// What the stop of the begun start observed.
        stop: StopUnconfirmed<F>,
    },
}

/// Why a start that began failed: the subset of [`StartRefused`] a start's
/// own steps produce.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum StartFailure<F> {
    /// The radio front end did not tune the channel, for the port's cause.
    Retune(F),
    /// The start did not end within the backend's command bound.
    Deadline,
    /// The device refused the start.
    Device,
}

impl<F> From<StartFailure<F>> for StartRefused<F> {
    fn from(failure: StartFailure<F>) -> Self {
        match failure {
            StartFailure::Retune(cause) => Self::Retune(cause),
            StartFailure::Deadline => Self::Deadline,
            StartFailure::Device => Self::Device,
        }
    }
}

/// The radio did not confirm its stop: the backend cannot prove the MAC,
/// its DMA and its interrupt work quiet. The cause names the port's fault
/// `F` the stop met, and the observations say what it still held.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct StopUnconfirmed<F> {
    /// Why the stop ended unconfirmed.
    pub cause: StopCause<F>,
    /// What the port held when it gave up.
    pub observations: StopObservations,
}

/// Why a stop ended unconfirmed; `F` is the port's fault.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum StopCause<F> {
    /// The stop did not end by its deadline, or its confirmation has not
    /// come yet: the stop goes on, and a later poll can confirm it.
    Deadline,
    /// The device refused or failed the stop, for the port's fault: the
    /// very fault the stop met, never one an earlier call answered.
    Device(F),
    /// A failed start had enabled the MAC: the port's next service
    /// disables it, and [`DataPort::poll_stop`] goes on from there.
    CoreEnabled,
}

/// What a port held when its stop ended unconfirmed.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct StopObservations {
    /// A data arena was on the air.
    pub arena_on_air: bool,
    /// Single MPDUs published to the hardware.
    pub published: u8,
    /// Single MPDUs admitted and not yet published.
    pub queued: u8,
    /// The station's receive configuration had not ended.
    pub configuring: bool,
}

/// The station's data arenas as a data port lends them for one turn
/// `'t`: the arenas and the rates they publish at.
pub trait PortArena<'t, 'id: 't>: TxArena<'t, 'id> {
    /// The channel the port runs on.
    fn channel(&self) -> Channel;

    /// What the backend aggregates.
    fn ampdu_capabilities(&self) -> AmpduCapabilities;

    /// The rate on the port's channel, when the capabilities send it
    /// there, for this turn.
    fn rate(&self, rate: PhyRate) -> Option<PortRate<'t, Self::Caps>> {
        if Self::Caps::CAPS.supports_rate(rate, self.channel()) {
            Some(PortRate::checked(rate))
        } else {
            None
        }
    }

    /// The rate, when the backend aggregates at it.
    ///
    /// # Errors
    ///
    /// The rate, when it does not.
    fn ampdu_rate(
        &self,
        rate: PortRate<'t, Self::Caps>,
    ) -> Result<AmpduRate<'t, Self::Caps>, PortRate<'t, Self::Caps>> {
        let capabilities = self.ampdu_capabilities();
        if capabilities.formats.contains_rate(rate.get())
            && capabilities.max_length.at(rate.get()).is_ok()
        {
            Ok(AmpduRate::checked(rate))
        } else {
            Err(rate)
        }
    }
}

/// A sealed single MPDU of the station's interface on the port `P`, as
/// [`crate::Attempt::admit`] makes it.
pub type StationSubmission<'k, 'p, 'id, P> = AdmittedAttempt<
    'id,
    <<P as DataPort<'id>>::Types as PortTypes<'id>>::TicketKind,
    <<P as DataPort<'id>>::Types as PortTypes<'id>>::Prepared<
        'k,
        'p,
        Station,
        <P as DataPort<'id>>::Caps,
    >,
    <P as DataPort<'id>>::Caps,
    <<P as DataPort<'id>>::Types as PortTypes<'id>>::Context,
    <<P as DataPort<'id>>::Types as PortTypes<'id>>::TicketPath,
>;

/// A data arena's end on the port `P`.
pub type PortCompleted<'id, P> = ArenaCompleted<
    <<P as DataPort<'id>>::Types as PortTypes<'id>>::BlockAckEvidence,
    <<P as DataPort<'id>>::Types as PortTypes<'id>>::AirDenied,
>;

/// A received unit of the port `P`.
pub type PortRxBuffer<'id, P> = <<P as DataPort<'id>>::Types as PortTypes<'id>>::RxBuffer;

/// The one receive pull of the port `P`.
pub type PortRxPull<'id, P> =
    RxPull<PortRxBuffer<'id, P>, <<P as DataPort<'id>>::Types as PortTypes<'id>>::RxMeta>;

/// The radio of one installation `'id`, owned by the station's datapath
/// actor and called only in its turns.
pub trait DataPort<'id> {
    /// The backend's capabilities marker.
    type Caps: Ieee80211Capabilities;
    /// The types the port's owners and terminals carry.
    type Types: CommandTypes<'id>;
    /// A broken contract of the backend's own state, named with its
    /// observations. The backend keeps every owner it holds; the caller
    /// ends the installation.
    type Fault: Copy + core::fmt::Debug + Eq;
    /// A station MPDU the port accepted from its sealed attempt
    /// ([`Self::admit`]), carrying no borrow of the port.
    type Admitted;
    /// The station's data arenas, lent for one turn.
    type Arena<'t>: PortArena<
            't,
            'id,
            Caps = Self::Caps,
            Memory = <Self::Types as PortTypes<'id>>::DataMemory,
            Response = <Self::Types as PortTypes<'id>>::BlockAckEvidence,
            Denial = <Self::Types as PortTypes<'id>>::AirDenied,
            Fault = Self::Fault,
        >
    where
        Self: 't,
        'id: 't;

    /// The channel the port runs on.
    fn channel(&self) -> Channel;

    /// The station interface's own address.
    fn address(&self) -> IndividualAddress;

    /// The rate on the port's channel, when the capabilities send it
    /// there. Only this check mints a [`PortRate`].
    fn rate(&self, rate: PhyRate) -> Option<PortRate<'_, Self::Caps>> {
        if Self::Caps::CAPS.supports_rate(rate, self.channel()) {
            Some(PortRate::checked(rate))
        } else {
            None
        }
    }

    /// Take the facts the port's signals hold, registering `cx` on each.
    /// It services nothing and never wakes `cx` itself.
    fn poll_facts(&mut self, cx: &mut Context<'_>) -> Facts;

    /// The earliest instant at which a call must run without a fact: a
    /// published attempt's hardware bound, an admitted MPDU's lifetime, a
    /// command's or a start's bound, a receive continuation. Kept up to
    /// date as the port's queues change, so reading it costs no scan.
    fn deadline(&self) -> Option<Instant>;

    /// Serve the transmit facts `tx` (or [`TxFacts::DUE`] at the deadline)
    /// and a sampled TBTT: completed single MPDUs go to `sink` with their
    /// terminals, expired queued ones end, the next queued ones publish.
    /// A published data arena's end returns here.
    ///
    /// The answer is the backend's contract fault, or the arena's end.
    ///
    /// # Errors
    ///
    /// An item the call could neither give `sink` nor keep, beside its
    /// answer ([`Unkeeping`]); nothing more was taken after it. An item an
    /// earlier call left refuses this one before it serves anything:
    /// [`Input::NotAccepted`] of `()`.
    fn service_tx(
        &mut self,
        tx: TxFacts,
        tbtt: bool,
        now: Instant,
        sink: &mut impl PortSink<'id, Self::Types>,
    ) -> ServiceAnswer<'id, Self>;

    /// Serve the published data arena alone on the completion events
    /// `tx`, for a caller that publishes the next arena before anything
    /// else: the arena's end when `tx` ended it. Every other service of
    /// `tx`, an abort in progress and events other than a completion stay
    /// with [`Self::service_tx`], which the caller runs next with the same
    /// `tx`.
    ///
    /// # Errors
    ///
    /// The backend's contract fault.
    fn service_arena(
        &mut self,
        tx: TxFacts,
        now: Instant,
    ) -> Result<Option<PortCompleted<'id, Self>>, Self::Fault>;

    /// Lend the station's data arenas for this turn.
    fn arena(&mut self) -> Self::Arena<'_>;

    /// Accept a sealed station attempt, dropping its borrow of the port.
    fn admit<'k, 'p>(&self, attempt: StationSubmission<'k, 'p, 'id, Self>) -> Self::Admitted;

    /// Queue an accepted station MPDU; it publishes when its queue is free,
    /// and its terminal returns its owners.
    ///
    /// The answer's refusal says why the port did not take the attempt: the
    /// radio does not run or stops. The attempt ended at once, withdrawn
    /// and never published
    /// ([`TxOutcome::Withdrawn`](crate::TxOutcome::Withdrawn)), and its
    /// terminal, with its context, ticket and fresh buffer, is in `sink`.
    ///
    /// # Errors
    ///
    /// An item the call could neither give `sink` nor keep
    /// ([`SubmitHalt::Unkeeping`]): found before the attempt was accepted,
    /// the attempt comes back whole; after, the answer. A port without the
    /// right to hand out the attempt's terminal returns the attempt whole,
    /// unaccepted ([`SubmitHalt::NotAccepted`]).
    fn submit(
        &mut self,
        attempt: Self::Admitted,
        now: Instant,
        sink: &mut impl PortSink<'id, Self::Types>,
    ) -> SubmitAnswer<'id, Self::Types, Self::Admitted>;

    /// One received unit, while the caller holds fewer than `room`.
    ///
    /// # Errors
    ///
    /// The backend's contract fault.
    fn receive(&mut self, room: u8, now: Instant) -> Result<PortRxPull<'id, Self>, Self::Fault>;

    /// Return a received unit to the ring. The port checks its count of
    /// the caller's units before it touches the unit.
    ///
    /// # Errors
    ///
    /// The port holds fewer of the caller's units than one: the unit,
    /// unchanged, with the count
    /// ([`ReceiveAccounting`](crate::ReceiveAccounting)). Nothing was taken
    /// and no fault recorded; the caller keeps the unit.
    fn recycle(
        &mut self,
        unit: PortRxBuffer<'id, Self>,
    ) -> Result<(), RecycleRefused<PortRxBuffer<'id, Self>>>;

    /// Move a received unit to the network without copying it.
    ///
    /// # Errors
    ///
    /// The unchanged unit, still owed to [`Self::recycle`].
    fn lend(
        &mut self,
        unit: PortRxBuffer<'id, Self>,
    ) -> Result<<Self::Types as PortTypes<'id>>::LentRx, LendRefused<PortRxBuffer<'id, Self>>>;

    /// Whether [`Self::lend`] refuses no unit for capacity now.
    fn lend_room(&self) -> bool;

    /// The units the network holds.
    fn lent(&self) -> usize;

    /// Ready while [`Self::lend_room`] holds; otherwise registers `cx` for
    /// the network's next return.
    fn poll_lend_room(&mut self, cx: &mut Context<'_>) -> Poll<()>;

    /// Ready once the network holds no unit; otherwise registers `cx` for
    /// its returns.
    fn poll_lent_returned(&mut self, cx: &mut Context<'_>) -> Poll<()>;

    /// Start acknowledging A-MPDUs of `agreement` in `slot`, from the
    /// port's next turn call on.
    #[must_use = "an agreement holds its slot"]
    fn add_block_ack(
        &mut self,
        slot: PortBlockAckSlot<'id, Self::Types>,
        agreement: SupportedBlockAck<Self::Caps>,
    ) -> <Self::Types as PortTypes<'id>>::BlockAck;

    /// Replace `running` with `agreement` of the same peer and TID.
    #[must_use = "an agreement holds its slot"]
    fn replace_block_ack(
        &mut self,
        running: <Self::Types as PortTypes<'id>>::BlockAck,
        agreement: SupportedBlockAck<Self::Caps>,
    ) -> <Self::Types as PortTypes<'id>>::BlockAck;

    /// End `agreement`, giving its slot back.
    #[must_use = "an ended agreement's slot is capacity"]
    fn remove_block_ack(
        &mut self,
        agreement: <Self::Types as PortTypes<'id>>::BlockAck,
    ) -> PortBlockAckSlot<'id, Self::Types>;

    /// Open the closed gate, a register write that cannot fail.
    fn open_gate(
        &mut self,
        gate: GateClosed<'id, <Self::Types as PortTypes<'id>>::GatePath>,
    ) -> GateOpen<'id, <Self::Types as PortTypes<'id>>::GatePath>;

    /// Run one command, or hold it at its barrier.
    ///
    /// The answer's failure is the backend's contract fault, with the
    /// command when the port did not take its owners ([`CommandFailed`]).
    ///
    /// # Errors
    ///
    /// An item the call could neither give `sink` nor keep ([`Unkeeping`]):
    /// found before the command was accepted, the command comes back whole;
    /// after, the answer.
    fn command(
        &mut self,
        command: PortCommand<'id, Self::Types, Self::Caps>,
        now: Instant,
        sink: &mut impl PortSink<'id, Self::Types>,
    ) -> CommandAnswer<'id, Self>;

    /// Start the stopped radio on `channel`, within the backend's command
    /// bound from the first call. A start that fails after the device began
    /// it stops what it began before it answers the refusal; a stop that
    /// does not confirm, and a start that enabled the MAC before it failed,
    /// leave the port stopping: the answer is [`StartRefused::Unstopped`]
    /// with the start's cause, and [`Self::poll_stop`] goes on with the
    /// stop.
    fn poll_start(
        &mut self,
        cx: &mut Context<'_>,
        channel: SupportedChannel<Self::Caps>,
        now: Instant,
    ) -> Poll<Result<(), StartRefused<Self::Fault>>>;

    /// Stop the radio: queued attempts end withdrawn, a published one is
    /// waited for, then the MAC, its DMA and interrupt work stop, within
    /// the backend's command bound from the first call.
    fn poll_stop(
        &mut self,
        cx: &mut Context<'_>,
        now: Instant,
    ) -> Poll<Result<(), StopUnconfirmed<Self::Fault>>>;

    /// After the radio's confirmed stop: offer what a sink refused and the
    /// port keeps to `sink`, oldest first, each once, until the sink refuses
    /// one again, which stays with everything newer. Each terminal and unit
    /// leaves the port once, with its own outcome and owners.
    ///
    /// # Errors
    ///
    /// The stop is not confirmed: nothing is offered here; the port offers
    /// what it keeps at its calls with a sink.
    fn take_unconsumed(
        &mut self,
        sink: &mut impl PortSink<'id, Self::Types>,
    ) -> Result<Unconsumed, UnconsumedRefused>;

    /// After the radio's confirmed stop: the cleanup the stop leaves to the
    /// port (a backend's write that releases hardware a failed operation
    /// left in use), by the lifecycle owner that stopped it, whether or not
    /// the datapath actor still runs. The owners the cleanup frees leave
    /// through their terminals in `sink`, each once; a terminal the sink
    /// refuses stays with the port for [`Self::take_unconsumed`]. A port
    /// whose cleanup did not finish does not retire. Nothing calls it on
    /// its own: each call is the owner's.
    ///
    /// The answer's refusal says why the cleanup did not finish
    /// ([`CleanupRefused`]).
    ///
    /// # Errors
    ///
    /// A freed terminal the call could neither give `sink` nor keep, beside
    /// its answer ([`Unkeeping`]). An item an earlier call left refuses
    /// this one before it cleans anything: [`Input::NotAccepted`] of `()`.
    fn clean_stopped(
        &mut self,
        sink: &mut impl PortSink<'id, Self::Types>,
    ) -> PortAnswer<'id, Self::Types, (), Result<(), CleanupRefused<Self::Fault>>>;

    /// Take back the fault the port last reported, so that a stop can run
    /// and return the owners it holds: whether it can. From then on the port
    /// admits and publishes nothing new, as a stopping port does, however
    /// late the stop comes; only its service of what is out goes on. A port
    /// that holds an owner no stop can return keeps its fault.
    fn recover_for_stop(&mut self) -> bool {
        false
    }

    /// A reading of the port's radio clock.
    ///
    /// # Errors
    ///
    /// The clock cannot be read now.
    fn sample_clock(&mut self) -> Result<Ieee80211Instant, ClockError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transmit_facts_merge_their_events_and_a_due_service_has_none() {
        assert_eq!(TxFacts::DUE.bits(), 0);
        let merged = TxFacts::events(0b01).union(TxFacts::events(0b10));
        assert_eq!(merged.bits(), 0b11);
        assert_eq!(TxFacts::DUE.union(merged), merged);
    }

    #[test]
    fn facts_are_empty_only_without_any_signal() {
        assert!(Facts::default().is_empty());
        for facts in [
            Facts {
                tx: Some(TxFacts::DUE),
                ..Facts::default()
            },
            Facts {
                tbtt: true,
                ..Facts::default()
            },
            Facts {
                rx: true,
                ..Facts::default()
            },
        ] {
            assert!(!facts.is_empty());
        }
    }
}
