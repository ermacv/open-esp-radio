//! The station's lifecycle: the station task's body beside the datapath
//! actor ([`crate::datapath`]).
//!
//! The lifecycle owns the station machine, its arbiter, the contexts of the
//! station's frames at their tickets' places, their admissions and the
//! station's group tokens. It never holds the port: every effect on the
//! radio is a [`ToActor`] message, and everything the radio returns comes
//! as a [`FromActor`] message, in order. It does no data work: the link's
//! data both ways is the actor's, so at steady transmission the lifecycle
//! wakes only on the actor's coalesced facts ([`FromActor::TxTraffic`],
//! [`FromActor::BarOwed`]), never per PPDU.
//!
//! # Waits
//!
//! One poll waits on the actor's messages, the application's commands and
//! stop, and one pinned alarm at the station's, the arbiter's and the
//! reconnect policy's deadline, re-armed only when that deadline changes.
//! Each wake reads the clock once, takes every message of the actor, the
//! due tick and one command, then steps the arbiter and the station until
//! neither owes an action, turning each action into its message.
//!
//! # Actions
//!
//! - A station frame goes as [`StationTx`] with its ticket and buffer; its
//!   context waits at its ticket's place until the frame's terminal or
//!   refusal comes back. A frame of the station's own numbering under its
//!   installed key, before the link exists, goes as [`FrameNumber::Keyed`]
//!   under the key the lifecycle lends the actor for it
//!   ([`ToActor::LendPairwise`]); the station holds no other step until the
//!   key is back.
//! - Key, configuration, gate and EDCA operations go as [`ToActor::Port`];
//!   an installation is minted by the port's issuer ([`PortCommands`]). A
//!   group key change goes as a parcel with the station's pending token,
//!   which comes back with its terminal after the actor applied it to the
//!   link's replay check: only that terminal switches the replay lane.
//! - The link goes to the actor at the handover ([`ToActor::Connect`]) and
//!   comes back ([`FromActor::LinkReturned`]); the pairwise key the actor
//!   keeps after it comes back on the station's request
//!   ([`ToActor::ReturnPairwise`]).
//! - A retune is a [`ToActor::Retune`]; a refused one goes back to the
//!   arbiter, which names the next or fails the station.
//!
//! # Ends
//!
//! The application's stop, and a failed station, take the link and the
//! pairwise key back, then stop the radio by the stop's deadline. A
//! confirmed stop retires the actor ([`ToActor::Retire`]) after the
//! application's stop, or ends it for the next installation
//! ([`ToActor::Reinstall`]) after the station failed: the composition then
//! reinstalls the port and builds the service again, without a reset
//! ([`LifecycleEnd::Restart`]). The actor's fault, an unconfirmed stop and a
//! broken contract of the actor's returns end the lifecycle with their
//! typed cause for the image to record ([`LifecycleEnd`]).

mod inbox;
mod step;

#[cfg(test)]
mod tests;

use core::future::{Future, poll_fn};
use core::pin::Pin;
use core::task::{Context, Poll};

use oer_portable_events::report;
use oer_portable_memory::{WakingSpscReceiver, WakingSpscSender};
use oer_portable_time::{Instant, Timer};
use oer_protocols_ieee80211_contracts_port::{
    CommandKind, CommandRefusal, KeyTypes, PhaseTypes, PoolMpdu, PortCommands, StartRefused,
    StopUnconfirmed, SupportedChannel, TerminalAccess,
};
use oer_protocols_ieee80211_core_arbiter::{
    Arbiter, ArbiterEnd, ArbiterNotice, NoReservation, PendingRetune, RetiredArbiter,
    StationPlaceReturns,
};
use oer_protocols_ieee80211_core_roles_sta::link::{FrameNumber, LinkReturn};
use oer_protocols_ieee80211_core_roles_sta::machine::{
    RetiredStation, StaCallbacks, StaConfigurationContext, StaDisconnectReason, StaFinalOutput,
    StaTxContext, Station,
};
use oer_protocols_ieee80211_core_roles_sta::port::{PendingStationPairwiseKey, PortStaTokens};
use oer_protocols_ieee80211_core_upper_mac::block_ack::RxBlockAckReplyBody;
use oer_protocols_ieee80211_core_upper_mac::ccmp::CcmpPacketNumberStep;
use oer_protocols_radio_port::Slot;

use crate::datapath::{
    ActorAnyBuffer, ActorCommand, ActorEpochPath, ActorFault, ActorGatePath, ActorLink,
    ActorTerminal, DatapathPort, FromActor, GroupPending, PARCELS, Parcel, Returned, StationTx,
    StopObserved, ToActor,
};
use crate::station::{
    BackendContractViolation, Notifies, Request, RunnerControl, SameHart, ServiceCommand,
    ServiceEvent, ServiceOutput, ServiceOutputSink, StationEvents, StopPolicy, TxContextStorage,
};
use crate::station::{CommandCredits, admissions::Admissions};

/// The port types of the composition `D`.
type Ty<'id, D> = <D as DatapathPort<'id>>::Types;

/// The station's tokens on the port of `D`.
pub type LifecycleTokens<'id, D, const GROUPS: usize> =
    PortStaTokens<'id, Ty<'id, D>, <D as DatapathPort<'id>>::Small, GROUPS>;

/// The station the lifecycle runs on the port of `D`.
pub type LifecycleStation<'id, D, C, const GROUPS: usize = 2> =
    Station<'id, LifecycleTokens<'id, D, GROUPS>, <D as DatapathPort<'id>>::Caps, C>;

/// The arbiter the lifecycle runs on the port of `D`.
pub type LifecycleArbiter<'id, D> =
    Arbiter<'id, <D as DatapathPort<'id>>::Caps, NoReservation, ActorGatePath<'id, D>>;

/// The context of one of the station's frames on the port of `D`.
pub type LifecycleTxContext<'id, D> = StaTxContext<'id, ActorEpochPath<'id, D>>;

/// A station frame's buffer back from the port of `D`: fresh or sent.
pub type LifecycleTxBuffer<'id, D> = PoolMpdu<'id, Ty<'id, D>, <D as DatapathPort<'id>>::Small>;

/// A constructed station and the command credits issued with it.
pub type LifecycleConstructed<'id, D, C, const GROUPS: usize> = (
    LifecycleStation<'id, D, C, GROUPS>,
    oer_protocols_ieee80211_core_roles_sta::machine::StaCommandCredits<
        'id,
        <Ty<'id, D> as PhaseTypes<'id>>::StationCommandPath,
    >,
);

/// The lifecycle's hooks of the composition's data port, beside the
/// actor's ([`DatapathPort`]).
pub trait LifecyclePort<'id>: DatapathPort<'id> {
    /// Recover the station's class of a returned buffer, fresh or sent.
    ///
    /// # Errors
    ///
    /// The buffer, unchanged, when it is of another class.
    #[allow(
        clippy::result_large_err,
        reason = "a foreign buffer comes back by value"
    )]
    fn tx_buffer(
        buffer: ActorAnyBuffer<'id, Self>,
    ) -> Result<LifecycleTxBuffer<'id, Self>, ActorAnyBuffer<'id, Self>>;
}

/// The lifecycle's ends to the actor: the counterparts of the actor's
/// [`LifecycleEnds`](crate::datapath::LifecycleEnds).
pub struct ActorEnds<
    'c,
    'id,
    D: DatapathPort<'id>,
    const TO: usize,
    const FROM: usize,
    const RETURNS: usize,
> {
    /// The lifecycle's messages.
    pub to_actor: WakingSpscSender<'c, ToActor<'id, D>, TO>,
    /// The lifecycle's large messages.
    pub parcels: WakingSpscSender<'c, Parcel<'id, D>, PARCELS>,
    /// The actor's messages.
    pub from_actor: WakingSpscReceiver<'c, FromActor<'id, D>, FROM>,
    /// The owners the actor gives back, each announced in `from_actor`.
    pub returns: WakingSpscReceiver<'c, Returned<'id, D>, RETURNS>,
    /// The station's group tokens, back with their change's end.
    pub groups: WakingSpscReceiver<'c, GroupPending<'id, ActorEpochPath<'id, D>>, 1>,
    /// The link the station hands over.
    pub link: WakingSpscSender<'c, ActorLink<'id, D>, 1>,
    /// The link the actor gives back.
    pub link_return: WakingSpscReceiver<'c, LinkReturn, 1>,
}

/// The lifecycle's ends to the application.
pub struct LifecycleIo<
    'c,
    K,
    O: ServiceOutputSink<K>,
    const COMMAND_DEPTH: usize,
    N: Notifies = SameHart,
> {
    /// The application's commands.
    pub commands: WakingSpscReceiver<'c, ServiceCommand<K>, COMMAND_DEPTH>,
    /// What the lifecycle tells the application.
    pub output: &'c mut O,
    /// The control connection: the stop request, the reconnect policy and
    /// the published state.
    pub control: RunnerControl<'c, N>,
    /// The deadline of a stop no request timed.
    pub stop: StopPolicy,
}

/// Why the station failed: the lifecycle stopped the radio and ends for
/// the next installation ([`LifecycleEnd::Restart`]).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StationFailure {
    /// The arbiter's move was refused and it could not return.
    RetuneRefused(StartRefused),
    /// A station request came while its kind's credit was out.
    CommandAdmission(Request),
    /// The reconnect policy's deadline overflowed.
    PolicyDeadlineOverflow,
    /// A validated receive Block Ack reply did not fit its frame.
    RxBlockAckEncode,
    /// The port refused a command in its state; the command's owners stay
    /// with the retired lifecycle.
    CommandRefused {
        /// The command.
        kind: CommandKind,
        /// Why.
        reason: CommandRefusal,
    },
    /// The station's key bank broke its contract.
    StationKeyBank(crate::station::contract::KeyContractViolation),
}

/// A broken contract of what the actor returned: the owner stays with the
/// retired lifecycle.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LifecycleViolation {
    /// A returned owner did not belong to an operation the lifecycle sent.
    Returned(BackendContractViolation),
    /// An announcement came without its owner: a return, a group token or
    /// a link.
    Missing,
    /// The actor's queue had no room the lifecycle had checked.
    QueueFull,
    /// The arbiter's move was refused while no move was pending.
    RetuneWithoutRequest,
}

/// How the lifecycle ended.
#[derive(Debug, Eq, PartialEq)]
pub enum LifecycleEnd<F> {
    /// The application stopped the station; the radio stopped and the
    /// actor retired its installation.
    Stopped,
    /// The station failed; the radio stopped and the actor ended for the
    /// next installation. The composition reinstalls the port and builds
    /// the service again.
    Restart(StationFailure),
    /// The radio did not start on its first channel; the actor retired.
    StartRefused(StartRefused),
    /// A stop proved nothing by its deadline.
    StopUnconfirmed(StopObserved),
    /// A retune's stop proved nothing.
    RetuneUnconfirmed(StopUnconfirmed),
    /// The actor ended on a broken contract.
    Faulted(ActorFault<F>),
    /// The actor returned an owner the lifecycle did not send.
    Contract(LifecycleViolation),
}

impl<F> LifecycleEnd<F> {
    /// Whether the composition records this end and resets: everything but
    /// an ordinary stop and a restart.
    pub const fn is_terminal(&self) -> bool {
        !matches!(self, Self::Stopped | Self::Restart(_))
    }
}

/// An owner a broken contract left with the lifecycle.
pub enum Rejected<'id, D: DatapathPort<'id>> {
    /// A terminal of no operation the lifecycle sent.
    Terminal(ActorTerminal<'id, D>),
    /// A refused command.
    Command(ActorCommand<'id, D>),
    /// A station frame's owners, without a context or of another class.
    Transmit {
        /// Its context, when one waited.
        context: Option<LifecycleTxContext<'id, D>>,
        /// Its ticket.
        ticket: LifecycleTicket<'id, D>,
        /// Its buffer.
        buffer: ActorAnyBuffer<'id, D>,
    },
    /// A group token of no pending change.
    Group(GroupPending<'id, ActorEpochPath<'id, D>>),
}

/// A transmit ticket of the port of `D`.
pub type LifecycleTicket<'id, D> = Slot<
    'id,
    <Ty<'id, D> as PhaseTypes<'id>>::TicketKind,
    <Ty<'id, D> as PhaseTypes<'id>>::TicketPath,
>;

/// The radio as the lifecycle commanded it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Radio {
    /// The first start is asked.
    Starting,
    /// It runs.
    Running,
    /// A move is asked.
    Retuning,
}

/// The arbiter, while it plans and once it ended.
#[allow(
    clippy::large_enum_variant,
    reason = "the lifecycle holds one arbiter in place, running or retired"
)]
enum ArbiterCell<'id, D: DatapathPort<'id>> {
    Running(LifecycleArbiter<'id, D>),
    Retired(RetiredArbiter<'id, D::Caps, NoReservation, ActorGatePath<'id, D>>),
}

/// Why the lifecycle ends, while it takes its owners back.
#[derive(Clone, Copy, Debug)]
enum Ending {
    /// The application stopped it.
    Stop,
    /// The station failed.
    Restart(StationFailure),
    /// The first start was refused: the radio never ran.
    StartRefused(StartRefused),
}

/// Where the end is: the link back, then the key, then the radio's stop.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EndStage {
    /// Nothing sent yet.
    Begin,
    /// The link was asked back.
    Link,
    /// The kept key was asked back.
    Key,
    /// The radio's stop was asked.
    Radio,
}

/// The link's custody, as the lifecycle knows it.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct LinkCustody {
    /// The actor holds the station's link.
    lent: bool,
    /// The actor keeps the station's pairwise key.
    key: bool,
    /// The key the actor keeps was lent for frames before the link, and is
    /// asked back: the station steps again once it is back.
    loan: bool,
    /// The connection epoch of the link the actor holds.
    epoch: Option<u64>,
}

/// The station's pairwise installation, until its terminal returns the key.
struct PendingPairwise<'id, D: DatapathPort<'id>> {
    pending: oer_protocols_ieee80211_core_roles_sta::machine::PendingPairwiseKey<
        'id,
        ActorEpochPath<'id, D>,
    >,
    key: PendingStationPairwiseKey<'id, <Ty<'id, D> as PhaseTypes<'id>>::StationKeys>,
}

/// The station's group change, until its terminal and token come back.
struct PendingGroup<'id, D: DatapathPort<'id>> {
    slot: <<Ty<'id, D> as PhaseTypes<'id>>::StationKeys as KeyTypes<'id>>::GroupSlotIdentity,
    key_id: Option<oer_protocols_ieee80211_contracts_port::GroupKeyId>,
}

/// The lifecycle of the station on the data port of `D`: its callbacks
/// `C`, the port's installation issuer `X`, the contexts' storage `T` and
/// the events' sink `O`.
pub struct Lifecycle<
    'id,
    D: DatapathPort<'id>,
    C: StaCallbacks,
    X,
    T,
    const GROUPS: usize = 2,
    O: StationEvents = (),
> {
    station: LifecycleStation<'id, D, C, GROUPS>,
    inner: Inner<'id, D, X, T, O>,
}

/// Everything of the lifecycle but its station: what the station's actions
/// borrow beside it.
struct Inner<'id, D: DatapathPort<'id>, X, T, O> {
    arbiter: Option<ArbiterCell<'id, D>>,
    commands: CommandCredits<'id, <Ty<'id, D> as PhaseTypes<'id>>::StationCommandPath>,
    issuer: X,
    step: CcmpPacketNumberStep,
    channel: SupportedChannel<D::Caps>,
    events: O,
    terminal_access: TerminalAccess<
        'id,
        <Ty<'id, D> as PhaseTypes<'id>>::TicketKind,
        <Ty<'id, D> as PhaseTypes<'id>>::TicketPath,
    >,
    tx_contexts: T,
    admissions: Admissions,
    pairwise: Option<PendingPairwise<'id, D>>,
    group: Option<PendingGroup<'id, D>>,
    retune: Option<PendingRetune<'id, D::Caps, ActorGatePath<'id, D>>>,
    radio: Radio,
    link: LinkCustody,
    /// The receive Block Ack reply the actor offered, until the station
    /// takes it.
    reply: Option<
        RxBlockAckReplyBody<
            oer_protocols_ieee80211_core_roles_sta::epoch::LinkEpoch<'id, ActorEpochPath<'id, D>>,
        >,
    >,
    ending: Option<(Ending, Instant, EndStage)>,
    /// The first owner a broken contract left, and its cause.
    rejected: Option<Rejected<'id, D>>,
    end: Option<
        LifecycleEnd<<D::Port as oer_protocols_ieee80211_contracts_port::DataPort<'id>>::Fault>,
    >,
    /// A station frame's rate lesson, for the actor after its terminal.
    lesson: Option<(
        oer_protocols_ieee80211_core_upper_mac::rate_selection::FrameRates,
        bool,
        crate::datapath::ActorSingleOutcome<'id, D>,
    )>,
    /// The application asked for the stop.
    stop: bool,
    /// The actor's queue had no room for the next step's messages.
    blocked: bool,
    /// The alarm's deadline as armed.
    armed: Option<Instant>,
    /// The wake's instant, read once.
    now: Instant,
}

/// What a finished lifecycle keeps: the station's and the arbiter's banks,
/// the owners a broken contract left and the events' sink.
pub struct RetiredLifecycle<
    'id,
    D: DatapathPort<'id>,
    C: StaCallbacks,
    X,
    T,
    const GROUPS: usize,
    O,
> {
    /// The station's bank.
    pub station: RetiredStation<'id, LifecycleTokens<'id, D, GROUPS>, D::Caps, C>,
    /// The places the station returned.
    pub places: StationPlaceReturns<'id, ActorGatePath<'id, D>>,
    /// The arbiter's bank; `None` only when a broken contract ended the
    /// lifecycle while it moved the arbiter.
    pub arbiter: Option<RetiredArbiter<'id, D::Caps, NoReservation, ActorGatePath<'id, D>>>,
    /// The installation issuer.
    pub issuer: X,
    /// The contexts still waiting at their tickets' places.
    pub tx_contexts: T,
    /// The first owner a broken contract left.
    pub rejected: Option<Rejected<'id, D>>,
    /// The events' sink.
    pub events: O,
}

/// The lifecycle's end and what it keeps.
pub struct LifecycleCompleted<
    'id,
    D: DatapathPort<'id>,
    C: StaCallbacks,
    X,
    T,
    const GROUPS: usize,
    O,
> {
    /// How it ended.
    pub end:
        LifecycleEnd<<D::Port as oer_protocols_ieee80211_contracts_port::DataPort<'id>>::Fault>,
    /// What it keeps.
    pub retired: RetiredLifecycle<'id, D, C, X, T, GROUPS, O>,
}

impl<'id, D, C, X, T, const GROUPS: usize, O> Lifecycle<'id, D, C, X, T, GROUPS, O>
where
    D: LifecyclePort<'id>,
    Ty<'id, D>: PhaseTypes<
            'id,
            StationConfigureContext = StaConfigurationContext<'id, ActorEpochPath<'id, D>>,
        >,
    C: StaCallbacks,
    X: PortCommands<'id, Types = D::Types, Caps = D::Caps>,
    T: TxContextStorage<LifecycleTxContext<'id, D>>,
    O: StationEvents,
{
    /// The lifecycle of the station and arbiter the composition built on a
    /// fresh installation: its radio starts on `channel`, its pairwise key
    /// numbers its MPDUs by `step`, and the port's `issuer` mints its key
    /// installations.
    pub fn new(
        station: LifecycleConstructed<'id, D, C, GROUPS>,
        arbiter: LifecycleArbiter<'id, D>,
        issuer: X,
        step: CcmpPacketNumberStep,
        channel: SupportedChannel<D::Caps>,
        terminal_access: TerminalAccess<
            'id,
            <Ty<'id, D> as PhaseTypes<'id>>::TicketKind,
            <Ty<'id, D> as PhaseTypes<'id>>::TicketPath,
        >,
        events: O,
    ) -> Self {
        let (station, credits) = station;
        Self {
            station,
            inner: Inner {
                arbiter: Some(ArbiterCell::Running(arbiter)),
                commands: CommandCredits::new(credits),
                issuer,
                step,
                channel,
                events,
                terminal_access,
                tx_contexts: T::empty(),
                admissions: Admissions::default(),
                pairwise: None,
                group: None,
                retune: None,
                radio: Radio::Starting,
                link: LinkCustody::default(),
                reply: None,
                ending: None,
                rejected: None,
                end: None,
                lesson: None,
                stop: false,
                blocked: false,
                armed: None,
                now: Instant::EPOCH,
            },
        }
    }

    /// The station, for the composition's checks.
    pub const fn station(&self) -> &LifecycleStation<'id, D, C, GROUPS> {
        &self.station
    }

    /// Run the station until the application stops it, it fails or the
    /// actor ends; the clock `timer` is read once per wake.
    pub async fn run<
        const TO: usize,
        const FROM: usize,
        const RETURNS: usize,
        const COMMAND_DEPTH: usize,
        N: Notifies,
    >(
        mut self,
        actor: &mut ActorEnds<'_, 'id, D, TO, FROM, RETURNS>,
        io: &mut LifecycleIo<'_, D::Caps, impl ServiceOutputSink<D::Caps>, COMMAND_DEPTH, N>,
        timer: &impl Timer,
    ) -> LifecycleCompleted<'id, D, C, X, T, GROUPS, O> {
        if actor
            .to_actor
            .try_send(ToActor::Start(self.inner.channel))
            .is_err()
        {
            self.inner.end = Some(LifecycleEnd::Contract(LifecycleViolation::QueueFull));
        }
        let end = {
            let arm = |at: Option<Instant>| alarm_at(timer, at);
            let mut alarm = core::pin::pin!(arm(None));
            poll_fn(|cx| self.poll(cx, actor, io, timer, alarm.as_mut(), &arm)).await
        };
        self.finish(end, io)
    }

    /// One poll: register on every source, take what is ready at one clock
    /// read, and step the machines.
    fn poll<
        F: Future<Output = ()>,
        const TO: usize,
        const FROM: usize,
        const RETURNS: usize,
        const COMMAND_DEPTH: usize,
        N: Notifies,
    >(
        &mut self,
        cx: &mut Context<'_>,
        actor: &mut ActorEnds<'_, 'id, D, TO, FROM, RETURNS>,
        io: &mut LifecycleIo<'_, D::Caps, impl ServiceOutputSink<D::Caps>, COMMAND_DEPTH, N>,
        timer: &impl Timer,
        mut alarm: Pin<&mut Option<F>>,
        arm: &impl Fn(Option<Instant>) -> Option<F>,
    ) -> Poll<LifecycleEnd<<D::Port as oer_protocols_ieee80211_contracts_port::DataPort<'id>>::Fault>>
    {
        if let Some(end) = self.inner.end.take() {
            return Poll::Ready(end);
        }
        if self.accepts_commands()
            && core::pin::pin!(io.control.wait_for_stop())
                .poll(cx)
                .is_ready()
        {
            self.inner.stop = true;
        }
        // The wake's one clock read.
        self.inner.now = timer.now();
        self.wake(actor, io);
        if let Some(end) = self.inner.end.take() {
            return Poll::Ready(end);
        }
        // Register for what this wake left waiting.
        let mut again = false;
        // Messages left behind a full queue wait for its room, not for
        // another poll.
        if core::pin::pin!(actor.from_actor.ready())
            .poll(cx)
            .is_ready()
            && !self.inner.blocked
        {
            again = true;
        }
        if self.inner.blocked && core::pin::pin!(actor.to_actor.ready()).poll(cx).is_ready() {
            again = true;
        }
        if self.accepts_commands() {
            if core::pin::pin!(io.commands.ready()).poll(cx).is_ready()
                || core::pin::pin!(io.control.wait_for_stop())
                    .poll(cx)
                    .is_ready()
            {
                again = true;
            }
            io.control.policy_register(cx);
        }
        let deadline = self.deadline(io).filter(|at| *at > self.inner.now);
        if deadline != self.inner.armed {
            self.inner.armed = deadline;
            alarm.set(arm(deadline));
        }
        if let Some(waiting) = alarm.as_mut().as_pin_mut()
            && waiting.poll(cx).is_ready()
        {
            alarm.set(None);
            self.inner.armed = None;
            again = true;
        }
        if again {
            cx.waker().wake_by_ref();
        }
        Poll::Pending
    }

    /// Whether the application's commands are taken now: the radio runs, the
    /// station reclaims nothing and the lifecycle is not ending.
    fn accepts_commands(&self) -> bool {
        self.inner.radio != Radio::Starting
            && self.inner.ending.is_none()
            && !self.station.is_reclaiming()
    }

    /// The earliest instant the lifecycle must wake without a message: the
    /// station's, the arbiter's and the reconnect policy's deadlines.
    fn deadline<K, Out: ServiceOutputSink<K>, const COMMAND_DEPTH: usize, N: Notifies>(
        &self,
        io: &LifecycleIo<'_, K, Out, COMMAND_DEPTH, N>,
    ) -> Option<Instant> {
        if self.inner.ending.is_some() {
            return self.inner.ending.map(|(_, deadline, _)| deadline);
        }
        let arbiter = match &self.inner.arbiter {
            Some(ArbiterCell::Running(arbiter)) => arbiter.poll_deadline(),
            _ => None,
        };
        let policy = if self.accepts_commands() {
            io.control.policy_deadline()
        } else {
            None
        };
        [arbiter, self.station.poll_deadline(), policy]
            .into_iter()
            .flatten()
            .min()
    }

    /// One wake at `self.inner.now`: the actor's messages, the due tick, one
    /// command, the machines' actions and the end's progress.
    fn wake<
        const TO: usize,
        const FROM: usize,
        const RETURNS: usize,
        const COMMAND_DEPTH: usize,
        N: Notifies,
    >(
        &mut self,
        actor: &mut ActorEnds<'_, 'id, D, TO, FROM, RETURNS>,
        io: &mut LifecycleIo<'_, D::Caps, impl ServiceOutputSink<D::Caps>, COMMAND_DEPTH, N>,
    ) {
        self.inner.blocked = false;
        self.inbox(actor, io);
        if self.inner.end.is_some() {
            return;
        }
        if self.inner.radio != Radio::Starting && self.inner.ending.is_none() {
            let now = self.inner.now;
            let machine = [
                match &self.inner.arbiter {
                    Some(ArbiterCell::Running(arbiter)) => arbiter.poll_deadline(),
                    _ => None,
                },
                self.station.poll_deadline(),
            ]
            .into_iter()
            .flatten()
            .min();
            if machine.is_some_and(|deadline| deadline <= now) {
                self.station.input(
                    now,
                    oer_protocols_ieee80211_core_roles_sta::machine::StaInput::Tick,
                    &mut self.inner.events,
                );
            }
            self.command(io);
            if self.inner.end.is_some() {
                return;
            }
            if self.stepping() {
                self.step(actor, io);
            }
        }
        self.progress_end(actor);
    }

    /// Whether the machines step now: the radio started, the lifecycle is
    /// not ending, and no key lent for a frame before the link is out.
    fn stepping(&self) -> bool {
        self.inner.end.is_none()
            && self.inner.radio != Radio::Starting
            && self.inner.ending.is_none()
            && !self.inner.link.loan
    }

    /// Take at most one command of the application or its policy.
    fn command<const COMMAND_DEPTH: usize, N: Notifies>(
        &mut self,
        io: &mut LifecycleIo<'_, D::Caps, impl ServiceOutputSink<D::Caps>, COMMAND_DEPTH, N>,
    ) {
        if !self.accepts_commands() {
            return;
        }
        let now = self.inner.now;
        let command = if self.inner.stop {
            Some(ServiceCommand::Stop)
        } else {
            match io.commands.try_receive() {
                Some(command) => Some(command),
                None => match io.control.policy_command(now) {
                    Ok(command) => command,
                    Err(_) => {
                        self.inner
                            .begin_end(Ending::Restart(StationFailure::PolicyDeadlineOverflow), io);
                        return;
                    }
                },
            }
        };
        let Some(command) = command else {
            return;
        };
        match command {
            ServiceCommand::Station(command) => match self.inner.commands.request(command) {
                Some(request) => {
                    io.output.emit(ServiceOutput::CommandStarted(command));
                    // A profile published with this Connect names its
                    // network; a station that is not idle refuses it, and
                    // the dropped profile erases its secrets.
                    if command.command() == crate::station::contract::Command::Connect
                        && let Some(profile) = io.control.take_profile()
                    {
                        drop(self.station.set_profile(profile));
                    }
                    if let Err(refused) = self.station.command(now, request) {
                        let refused = self.inner.commands.refused(refused);
                        io.output.emit(ServiceOutput::CommandRefused(refused));
                    }
                }
                None => self.inner.begin_end(
                    Ending::Restart(StationFailure::CommandAdmission(command)),
                    io,
                ),
            },
            ServiceCommand::AccessPoint(fact) => {
                if let Some(ArbiterCell::Running(arbiter)) = self.inner.arbiter.as_mut() {
                    arbiter.access_point(now, fact.into());
                }
            }
            ServiceCommand::WantAccessPoint => {
                if let Some(ArbiterCell::Running(arbiter)) = self.inner.arbiter.as_mut() {
                    arbiter.want_access_point();
                }
            }
            ServiceCommand::Stop => self.inner.begin_end(Ending::Stop, io),
        }
    }

    /// Move the end on: the link back, the kept key back, the radio's stop.
    fn progress_end<const TO: usize, const FROM: usize, const RETURNS: usize>(
        &mut self,
        actor: &ActorEnds<'_, 'id, D, TO, FROM, RETURNS>,
    ) {
        let Some((why, deadline, stage)) = self.inner.ending else {
            return;
        };
        let next = match stage {
            EndStage::Begin if self.inner.link.lent => Some((EndStage::Link, ToActor::Disconnect)),
            EndStage::Begin | EndStage::Link if !self.inner.link.lent && self.inner.link.key => {
                Some((EndStage::Key, ToActor::ReturnPairwise))
            }
            EndStage::Begin | EndStage::Link | EndStage::Key
                if !self.inner.link.lent && !self.inner.link.key =>
            {
                Some((EndStage::Radio, ToActor::Stop { deadline }))
            }
            _ => None,
        };
        let Some((stage, message)) = next else {
            return;
        };
        if actor.to_actor.try_send(message).is_err() {
            self.inner.blocked = true;
            return;
        }
        self.inner.ending = Some((why, deadline, stage));
    }

    /// Finish the station and the arbiter after `end`, telling the
    /// application what they still owe.
    fn finish<const COMMAND_DEPTH: usize, N: Notifies>(
        self,
        end: LifecycleEnd<
            <D::Port as oer_protocols_ieee80211_contracts_port::DataPort<'id>>::Fault,
        >,
        io: &mut LifecycleIo<'_, D::Caps, impl ServiceOutputSink<D::Caps>, COMMAND_DEPTH, N>,
    ) -> LifecycleCompleted<'id, D, C, X, T, GROUPS, O> {
        let Self { station, inner } = self;
        let Inner {
            arbiter,
            mut commands,
            issuer,
            tx_contexts,
            events,
            rejected,
            ..
        } = inner;
        let (reason, arbiter_end) = match &end {
            LifecycleEnd::Stopped => (StaDisconnectReason::Requested, ArbiterEnd::Stopped),
            _ => (
                StaDisconnectReason::Arbiter(
                    oer_protocols_ieee80211_core_arbiter::LeaveReason::PortFailed,
                ),
                ArbiterEnd::PortFailed,
            ),
        };
        let (mut station, mut places) = station.finish(reason);
        let mut arbiter = match arbiter {
            Some(ArbiterCell::Running(arbiter)) => Some(arbiter.finish(arbiter_end)),
            Some(ArbiterCell::Retired(retired)) => Some(retired),
            None => None,
        };
        while let Some(notice) = arbiter.as_mut().and_then(RetiredArbiter::poll_notice) {
            match notice {
                ArbiterNotice::Station(notice) => places.returned(notice),
                ArbiterNotice::AccessPoint(command) => {
                    io.output.emit(ServiceOutput::AccessPoint(command.into()))
                }
            }
        }
        while let Some(output) = station.poll_output() {
            match output {
                StaFinalOutput::CommandCompleted(terminal) => io.output.emit(
                    ServiceOutput::CommandCompleted(commands.completed(terminal)),
                ),
                StaFinalOutput::Notify(notice) => {
                    io.output.emit(ServiceOutput::Notice(notice.into()))
                }
            }
        }
        // A restart keeps the application's connection for the next
        // installation's service.
        if !matches!(end, LifecycleEnd::Restart(_)) {
            io.control.complete();
        }
        LifecycleCompleted {
            end,
            retired: RetiredLifecycle {
                station,
                places,
                arbiter,
                issuer,
                tx_contexts,
                rejected,
                events,
            },
        }
    }
}

/// The alarm at `at`, or none: one function, so every alarm of a run has
/// the one type the run pins.
fn alarm_at<T: Timer>(timer: &T, at: Option<Instant>) -> Option<impl Future<Output = ()> + '_> {
    at.map(|at| timer.wait_until(at))
}

impl<'id, D, X, T, O> Inner<'id, D, X, T, O>
where
    D: LifecyclePort<'id>,
    Ty<'id, D>: PhaseTypes<
            'id,
            StationConfigureContext = StaConfigurationContext<'id, ActorEpochPath<'id, D>>,
        >,
    X: PortCommands<'id, Types = D::Types, Caps = D::Caps>,
    T: TxContextStorage<LifecycleTxContext<'id, D>>,
    O: StationEvents,
{
    /// Begin the end for `why`: the stop's deadline is the request's, or the
    /// policy's from now.
    fn begin_end<K, Out: ServiceOutputSink<K>, const COMMAND_DEPTH: usize, N: Notifies>(
        &mut self,
        why: Ending,
        io: &LifecycleIo<'_, K, Out, COMMAND_DEPTH, N>,
    ) {
        if self.ending.is_some() {
            return;
        }
        let deadline = match (why, io.control.stop_deadline()) {
            (Ending::Stop, Some(deadline)) => deadline,
            _ => self.now.saturating_add(io.stop.drain),
        };
        self.ending = Some((why, deadline, EndStage::Begin));
    }
}
