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
/// the order it produced it: the turn's outbox. A sink takes every value;
/// one it cannot hold it keeps and reports itself, so the port never gets
/// an owner back.
pub trait PortSink<'id, T: PortTypes<'id>> {
    /// A command's or a single MPDU's terminal.
    fn terminal(&mut self, terminal: LowerMacTerminal<'id, T>);
    /// A TBTT of the station's schedule.
    fn tbtt(&mut self, event: TbttEvent);
    /// A unit a group mutation's receive cut drained: it precedes the
    /// mutation's terminal.
    fn received(&mut self, frame: T::RxBuffer, meta: T::RxMeta);
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
        stop: StopUnconfirmed,
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
/// its DMA and its interrupt work quiet. The observations say what it
/// still held.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct StopUnconfirmed {
    /// Why the stop ended unconfirmed.
    pub cause: StopCause,
    /// What the port held when it gave up.
    pub observations: StopObservations,
}

/// Why a stop ended unconfirmed.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum StopCause {
    /// The stop did not end by its deadline.
    Deadline,
    /// The device refused or failed the stop.
    Device,
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
    type Fault: Copy + core::fmt::Debug;
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
    /// # Errors
    ///
    /// The backend's contract fault.
    fn service_tx(
        &mut self,
        tx: TxFacts,
        tbtt: bool,
        now: Instant,
        sink: &mut impl PortSink<'id, Self::Types>,
    ) -> Result<Option<PortCompleted<'id, Self>>, Self::Fault>;

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
    /// # Errors
    ///
    /// Why the port did not take the attempt: the radio does not run or
    /// stops. The attempt ended at once, withdrawn and never published
    /// ([`TxOutcome::Withdrawn`](crate::TxOutcome::Withdrawn)), and its
    /// terminal, with its context, ticket and fresh buffer, is in `sink`.
    fn submit(
        &mut self,
        attempt: Self::Admitted,
        now: Instant,
        sink: &mut impl PortSink<'id, Self::Types>,
    ) -> Result<(), SubmitRefusal>;

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
    /// # Errors
    ///
    /// The backend's contract fault, with the command when the port did
    /// not take its owners ([`CommandFailed`]).
    fn command(
        &mut self,
        command: PortCommand<'id, Self::Types, Self::Caps>,
        now: Instant,
        sink: &mut impl PortSink<'id, Self::Types>,
    ) -> Result<CommandProgress<'id, Self::Types, Self::Caps>, PortCommandFailed<'id, Self>>;

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
    ) -> Poll<Result<(), StopUnconfirmed>>;

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
