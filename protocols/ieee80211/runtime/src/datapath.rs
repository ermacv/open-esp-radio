//! The station's datapath actor: the one task that owns the radio.
//!
//! The actor owns the installation's [`DataPort`] by value for the
//! installation's whole life and is its only caller. It carries the data
//! both ways without the station: the network's MSDUs into the port's two
//! data arenas through the station's data plane, and the received units to
//! the network, lent in place or copied. It numbers the link's data and the
//! station's own frames from the link the station hands over
//! ([`ToActor::Connect`]), owns the receive Block Ack agreements and the
//! rate selection, and runs the lifecycle's port commands in order.
//!
//! The station's lifecycle runs in its own task on the same executor and
//! talks to the actor only in messages ([`ToActor`], [`FromActor`]): it
//! never holds the port and never runs per PPDU. Frames that are not data
//! of the link, and every frame before the link exists, go to it
//! ([`FromActor::Received`]) and come back ([`ToActor::Recycle`]); EAPOL of
//! the link goes to it as [`FromActor::Eapol`]. At most
//! [`LIFECYCLE_FRAMES`] frames are with the lifecycle at once: a frame for
//! a full lane goes back to the ring and is counted.
//!
//! # Turns
//!
//! The actor waits on the port's signals, its ends' readiness and one
//! alarm, re-armed only when its deadline changes. Each turn reads the
//! clock once and passes that instant to every call of the turn:
//!
//! 1. the transmit facts: completed station frames and port terminals go to
//!    the lifecycle, a data arena's end settles and the sealed standby
//!    publishes at once;
//! 2. the lifecycle's messages in order, at most [`MESSAGE_BUDGET`], of
//!    which at most one port command or radio change; a command the port
//!    holds at its barrier stops the messages until it ran, but a stop
//!    behind it withdraws it;
//! 3. at most [`RX_BUDGET`] received units, reordered, admitted by the link
//!    and handed on; the network at its lend limit stops the pull until it
//!    returns a unit. While the message budget left messages queued, the
//!    units wait for the next turn: a link handed over before them is in
//!    force when they are routed;
//! 4. at most `TX_DEPTH` network MSDUs into the standby arena, which
//!    publishes when nothing is on the air;
//! 5. the network's freed memory back, while its queue has room;
//! 6. the coalesced facts for the lifecycle.
//!
//! The actor wakes itself only when a budget ran out with work left, or
//! when its own poll after the turn took facts of the port's signals (a
//! unit or an event that came during the turn: taking them took the wake
//! their signal owed): one repost, then it yields. An idle actor sleeps
//! until a source wakes it.
//!
//! # Errors
//!
//! Air input never ends the actor: a frame with no reordering place or no
//! lifecycle lane goes back to the ring, counted ([`ActorEvent`]); the
//! network at its lend limit is back-pressure. A refusal returns its
//! owners to the lifecycle ([`Returned`]). Only a broken contract ends the
//! run: the port's named fault or the actor's own ([`ActorFault`]); every
//! owner then stays where it was, and the lifecycle hears it
//! ([`FromActor::Faulted`]).

mod data;
mod messages;
mod rx;
mod station_tx;
mod turn;

pub use messages::{
    ActorEnd, ActorEvent, ActorEvents, ActorFault, ActorViolation, FromActor, GroupPending,
    LifecycleEnds, NetworkEnds, Parcel, RetuneFailed, Returned, StationTx, StopObserved,
    StopOutcome, ToActor, from_actor_depth, returns_depth,
};

use core::future::{Future, poll_fn};
use core::pin::Pin;
use core::task::{Context, Poll};

use oer_portable_events::{EventSink, report};
use oer_portable_memory::{SlotReceiver, SlotSender, WakingSpscReceiver, WakingSpscSender};
use oer_portable_time::{Duration, Instant, Timer};
use oer_protocols_ieee80211_contracts_port::{
    CommandProgress, CommandRefusal, CommandTypes, DataPort, Facts, GateClosed, GateOpen,
    Ieee80211Capabilities, KeyTypes, LentRxBuffer, LowerMacTerminal, PortBlockAckSlot, PortCommand,
    PortSingleOutcome, PortSink, PortTypes, RxBuffer, StartRefused, StopUnconfirmed,
    SupportedBlockAck, SupportedChannel, TbttEvent, TxBufferClass, TxBufferPool, TxFacts,
};
use oer_protocols_ieee80211_core_roles_sta::data::EthernetFrame;
use oer_protocols_ieee80211_core_roles_sta::data::plane::{DataPlane, OutboundMsdu, TxWindow};
use oer_protocols_ieee80211_core_roles_sta::data::{WmmAccessCategory, WmmUserPriority};
use oer_protocols_ieee80211_core_roles_sta::epoch::LinkEpoch;
use oer_protocols_ieee80211_core_roles_sta::link::{
    FrameNumber, GroupLane, LinkHandover, LinkReturn, SequenceNumber,
};
use oer_protocols_ieee80211_core_roles_sta::machine::{
    EapolProtection, Peer, PendingGroupKey, PendingGroupRemoval, PendingGroupReplacement,
    PendingGroupSelection, PendingRxBlockAck, StaFrame, StaFrameClass, StaRatePlan, TxRefusal,
};
use oer_protocols_ieee80211_core_upper_mac::block_ack::{
    ReplyPlace, RxBlockAckPort, RxBlockAckReplyBody,
};
use oer_protocols_ieee80211_core_upper_mac::ccmp::InstalledCcmpTxKey;
use oer_protocols_ieee80211_core_upper_mac::rate_selection::{FrameRates, RateSelection};
use oer_protocols_radio_port::{PoolPath, Slot};

use crate::station::contract::MsduRefusal;
use crate::station::{
    NetworkStop, Notifies, Outgoing, PortRxBlockAck, RX_COPIED_DEPTH, Received, RxBlockAckView,
    SameHart, StationEvents,
};

/// The frames the lifecycle holds at most: those it has not sent back.
pub const LIFECYCLE_FRAMES: usize = 4;

/// The parcels the lifecycle sends ahead at most: an association and a
/// group key change.
pub const PARCELS: usize = 2;

/// The largest message to the actor: larger ones travel as parcels.
pub const TO_ACTOR_BYTES: usize = 128;

/// The turns in a row a due port deadline reposts the actor at most.
pub const DUE_REPOSTS: u8 = 4;

/// The received units one turn pulls from the port at most.
pub const RX_BUDGET: u8 = 32;

/// The lifecycle's messages one turn takes at most.
pub const MESSAGE_BUDGET: u8 = 8;

/// The received units one group mutation's receive cut may drain: the
/// actor stages them until the command returns, then routes them before
/// the mutation's terminal.
pub const CUT_UNITS: usize = RX_BUDGET as usize;

/// How often a TID's ongoing traffic is told to the lifecycle at most: the
/// first MSDU after a quiet TID is told at once, later ones at most once
/// per this span, so the station's transmit agreements see the TID live
/// without a lifecycle wake per PPDU.
pub const TX_TRAFFIC_REFRESH: Duration = Duration::from_millis(100);

/// The network memory the data plane holds at most: both arenas' frames
/// and the one MSDU that waits for the standby.
const HELD: usize = 2 * oer_protocols_ieee80211_contracts_port::arena::ARENA_FRAMES as usize + 1;

/// The composition's mapping from its data port to the actor's types. It
/// is a mapping rather than a port supertrait, so an image names it for a
/// port of another crate.
pub trait DatapathPort<'id>: Sized + 'id {
    /// The installation's data port.
    type Port: DataPort<'id, Caps = Self::Caps, Types = Self::Types>;
    /// The capabilities shared by the port and the portable machines.
    type Caps: Ieee80211Capabilities;
    /// The port's types.
    type Types: CommandTypes<
            'id,
            TxBody = core::convert::Infallible,
            Context = (),
            DataMemory = Self::TxMemory,
            LentRx = Self::LentRx,
        > + TxBufferPool<'id, Self::Small>;
    /// The buffer class of the station's own frames.
    type Small: TxBufferClass;
    /// The network's memory a data MSDU comes in.
    type TxMemory: Send + 'static;
    /// A received buffer the port lends the network.
    type LentRx: LentRxBuffer;
    /// The received units the actor holds at most: what it asks the port
    /// for ([`DataPort::receive`]'s room), less the lifecycle's frames.
    const RX_HELD: u8;

    /// What a data arena's ACK evidence tells the rate selection of a
    /// single MPDU's response; `None` when it carries nothing it reads.
    fn arena_ack(
        response: <Self::Types as PortTypes<'id>>::BlockAckEvidence,
    ) -> Option<<Self::Types as PortTypes<'id>>::AckEvidence>;

    /// Observe the start (`end` false) or end of a stretch of the actor's
    /// work; by default nothing observes it.
    fn span(_span: ActorSpan, _end: bool) {}

    /// Observe a unit the port gave (`frame` from its header), before the
    /// reordering and the link take it; by default nothing observes it.
    fn received(_frame: &[u8], _meta: &<Self::Types as PortTypes<'id>>::RxMeta) {}

    /// Observe a terminal the port gave, before the lifecycle takes it; by
    /// default nothing observes it.
    fn terminal(_terminal: &LowerMacTerminal<'id, Self::Types>) {}
}

/// A stretch of the actor's work, which it reports at both ends
/// ([`DatapathPort::span`]).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum ActorSpan {
    /// One whole turn.
    Turn,
    /// The transmit service and the data arena's settlement.
    Transmit,
    /// The received units.
    Receive,
    /// The network's MSDUs into the standby arena.
    Intake,
    /// The lifecycle's messages.
    Messages,
}

impl ActorSpan {
    /// Every span, each at the position of its number.
    pub const ALL: [Self; 5] = [
        Self::Turn,
        Self::Transmit,
        Self::Receive,
        Self::Intake,
        Self::Messages,
    ];

    /// The span numbered `number`.
    pub fn from_number(number: u8) -> Option<Self> {
        Self::ALL.into_iter().find(|span| *span as u8 == number)
    }
}

type Ty<'id, D> = <D as DatapathPort<'id>>::Types;

/// A received unit of the port of `D`.
pub type ActorRxBuffer<'id, D> = <Ty<'id, D> as PortTypes<'id>>::RxBuffer;
/// What the port of `D` observed of a received unit.
pub type ActorRxMeta<'id, D> = <Ty<'id, D> as PortTypes<'id>>::RxMeta;
/// The path of the station's connection epochs on the port of `D`.
pub type ActorEpochPath<'id, D> = <Ty<'id, D> as PortTypes<'id>>::EpochPath;
/// The path of the port's gate.
pub type ActorGatePath<'id, D> = <Ty<'id, D> as PortTypes<'id>>::GatePath;
/// A transmit ticket of the port of `D`.
pub type ActorTicket<'id, D> = Slot<
    'id,
    <Ty<'id, D> as PortTypes<'id>>::TicketKind,
    <Ty<'id, D> as PortTypes<'id>>::TicketPath,
>;
/// A fresh buffer of the station's class.
pub type ActorStationBuffer<'id, D> =
    <Ty<'id, D> as TxBufferPool<'id, <D as DatapathPort<'id>>::Small>>::Buffer;
/// A transmit buffer of any class, as a terminal or a refusal returns it.
pub type ActorAnyBuffer<'id, D> = <Ty<'id, D> as PortTypes<'id>>::AnyTxBuffer;
/// A command of the lifecycle to the port of `D`.
pub type ActorCommand<'id, D> = PortCommand<'id, Ty<'id, D>, <D as DatapathPort<'id>>::Caps>;
/// A terminal of the port of `D`.
pub type ActorTerminal<'id, D> = LowerMacTerminal<'id, Ty<'id, D>>;
/// A single MPDU's outcome on the port of `D`.
pub type ActorSingleOutcome<'id, D> = PortSingleOutcome<'id, Ty<'id, D>>;
/// The transmit half of the station's pairwise key on the port of `D`,
/// which the link numbers its protected MPDUs with.
pub type ActorLinkKey<'id, D> =
    InstalledCcmpTxKey<<<Ty<'id, D> as PortTypes<'id>>::StationKeys as KeyTypes<'id>>::PairwiseKey>;
/// The station's link on the port of `D`, while the actor holds it.
pub type ActorLink<'id, D> = LinkHandover<'id, ActorEpochPath<'id, D>, ActorLinkKey<'id, D>>;

/// The radio's state as the actor drives it.
#[derive(Clone, Copy)]
enum Radio<K> {
    /// Stopped, before its first start or after a refused start.
    Stopped,
    /// Starting on the channel.
    Starting(SupportedChannel<K>),
    /// Running.
    Running,
    /// Moving to the channel: stopping first, then starting there.
    Retuning {
        channel: SupportedChannel<K>,
        stopped: bool,
    },
    /// Stopping by the deadline.
    Stopping { deadline: Instant },
    /// The stop was answered; the radio takes a start or the retirement.
    Halted,
}

impl<K> Radio<K> {
    const fn running(&self) -> bool {
        matches!(self, Self::Running)
    }

    /// Whether a station frame or a port command waits for the radio to
    /// run: it starts or moves.
    const fn changing(&self) -> bool {
        matches!(self, Self::Starting(_) | Self::Retuning { .. })
    }
}

/// A port command the port holds at its barrier, with the group token of
/// its change.
struct HeldCommand<'id, D: DatapathPort<'id>> {
    command: ActorCommand<'id, D>,
}

/// The facts the actor owes the lifecycle, coalesced until its queue has
/// room for them.
struct Owed<'id, D: DatapathPort<'id>> {
    /// Each TID's BlockAckReq head.
    bar: [Option<SequenceNumber>; 8],
    /// Each TID's traffic: the instant it was last told and the next
    /// Sequence Number to tell.
    traffic: [(Option<Instant>, Option<SequenceNumber>); 8],
    /// The last TBTT.
    tbtt: Option<TbttEvent>,
    /// Refused MSDUs by reason.
    refused: heapless::Deque<(MsduRefusal, u16), 4>,
    /// The answers to the lifecycle's radio and link messages.
    started: Option<Result<(), StartRefused<<D::Port as DataPort<'id>>::Fault>>>,
    retuned: Option<Result<(), RetuneFailed<<D::Port as DataPort<'id>>::Fault>>>,
    stopped: Option<StopOutcome>,
    link_returned: bool,
    sequence: Option<(WmmUserPriority, SequenceNumber)>,
    pairwise: Option<ActorLinkKey<'id, D>>,
    gate: Option<GateOpen<'id, ActorGatePath<'id, D>>>,
}

impl<'id, D: DatapathPort<'id>> Owed<'id, D> {
    const fn new() -> Self {
        Self {
            bar: [None; 8],
            traffic: [(None, None); 8],
            tbtt: None,
            refused: heapless::Deque::new(),
            started: None,
            retuned: None,
            stopped: None,
            link_returned: false,
            sequence: None,
            pairwise: None,
            gate: None,
        }
    }

    fn refused(&mut self, reason: MsduRefusal) {
        if let Some((_, count)) = self.refused.iter_mut().find(|(seen, _)| *seen == reason) {
            *count = count.saturating_add(1);
        } else if self.refused.push_back((reason, 1)).is_err() {
            // Every place holds another reason: the count goes to the last.
            if let Some((_, last)) = self.refused.back_mut() {
                *last = last.saturating_add(1);
            }
        }
    }
}

/// What the port put in the turn's sink that the actor handles after the
/// call: the group change's terminal, the units a receive cut drained before
/// it, the last TBTT.
struct Stage<'id, D: DatapathPort<'id>> {
    cut: heapless::Deque<(ActorRxBuffer<'id, D>, ActorRxMeta<'id, D>), CUT_UNITS>,
    group_terminal: Option<ActorTerminal<'id, D>>,
    tbtt: Option<TbttEvent>,
    /// The sink found no room for an owner: the run ends on this broken
    /// contract.
    overflow: Option<ActorViolation>,
}

/// The data transmission's state: the plane over the port's arenas, the
/// MSDU that waits for the standby and the network's freed memory.
struct DataTx<M> {
    plane: Option<DataPlane>,
    deferred: Option<OutboundMsdu<M>>,
    freed: heapless::Deque<M, HELD>,
}

/// The station's datapath actor over the data port of `D`: its rates `S`
/// and its events' sink `O`.
pub struct Actor<'id, D: DatapathPort<'id>, S, O> {
    port: D::Port,
    rates: S,
    events: O,
    receive: RxBlockAckView<'id, D::Types>,
    link: Option<ActorLink<'id, D>>,
    /// The link's pairwise key, kept after the link came back until the
    /// lifecycle asks for it: the one owner of its packet numbers.
    pairwise: Option<ActorLinkKey<'id, D>>,
    /// The TID whose data waits for the station's ADDBA Request, which
    /// names the TID's next Sequence Number.
    sequence_hold: Option<WmmUserPriority>,
    data: DataTx<D::TxMemory>,
    radio: Radio<D::Caps>,
    held: Option<HeldCommand<'id, D>>,
    /// The next message, taken while a held command or a changing radio
    /// stopped the messages.
    stash: Option<ToActor<'id, D>>,
    /// The station's token of the group change whose terminal is owed.
    group: Option<GroupPending<'id, ActorEpochPath<'id, D>>>,
    stage: Stage<'id, D>,
    /// Frames the lifecycle holds.
    frames_out: u8,
    /// The reply offered to the lifecycle, until it answers.
    reply_out: Option<ReplyPlace>,
    owed: Owed<'id, D>,
    /// The facts of the port taken and not served yet.
    facts: Facts,
    /// The receive ring has work from this instant on.
    rx_later: Option<Instant>,
    /// A budget ran out with work left.
    again: bool,
    /// The alarm's deadline as armed.
    armed: Option<Instant>,
    /// The turns in a row whose port deadline stayed due.
    due_streak: u8,
    /// The turns in a row after which the port's signals gave facts again
    /// although the turn served none of its work.
    fact_streak: u8,
    /// The turn served the port's work: a received unit, transmit events
    /// or a TBTT.
    progressed: bool,
    /// The turn's instant, read once.
    now: Instant,
    /// Owners the actor keeps after a broken contract.
    retained: Retained<'id, D>,
    /// How the run ended, once it did.
    end: Option<ActorEnd<<D::Port as DataPort<'id>>::Fault>>,
}

/// Owners a broken contract leaves with the actor.
struct Retained<'id, D: DatapathPort<'id>> {
    link: Option<ActorLink<'id, D>>,
    returned: Option<Returned<'id, D>>,
    completed: Option<oer_protocols_ieee80211_contracts_port::PortCompleted<'id, D::Port>>,
    link_return: Option<LinkReturn>,
    pairwise: Option<ActorLinkKey<'id, D>>,
    group: Option<GroupPending<'id, ActorEpochPath<'id, D>>>,
}

impl<'id, D: DatapathPort<'id>, S, O> Actor<'id, D, S, O>
where
    S: RateSelection<
            ActorRxMeta<'id, D>,
            <Ty<'id, D> as PortTypes<'id>>::AckEvidence,
            <Ty<'id, D> as PortTypes<'id>>::BlockAckEvidence,
        >,
    O: ActorEvents,
{
    /// The actor holds more received units than the lifecycle's lane:
    /// checked at monomorphization by every constructor.
    const HOLDS_THE_LANE: () = {
        if D::RX_HELD as usize <= LIFECYCLE_FRAMES {
            panic!("the actor holds more received units than the lifecycle's lane");
        }
    };

    /// The actor of a fresh installation's `port`, its frames at the rates
    /// `rates` selects, its receive agreements in `receive`'s storage.
    pub fn new(port: D::Port, rates: S, receive: RxBlockAckView<'id, D::Types>, events: O) -> Self {
        let () = Self::HOLDS_THE_LANE;
        Self {
            port,
            rates,
            events,
            receive,
            link: None,
            pairwise: None,
            sequence_hold: None,
            data: DataTx {
                plane: None,
                deferred: None,
                freed: heapless::Deque::new(),
            },
            radio: Radio::Stopped,
            held: None,
            stash: None,
            group: None,
            stage: Stage {
                cut: heapless::Deque::new(),
                group_terminal: None,
                tbtt: None,
                overflow: None,
            },
            frames_out: 0,
            reply_out: None,
            owed: Owed::new(),
            facts: Facts::default(),
            rx_later: None,
            again: false,
            armed: None,
            due_streak: 0,
            fact_streak: 0,
            progressed: false,
            now: Instant::EPOCH,
            retained: Retained {
                link: None,
                returned: None,
                completed: None,
                link_return: None,
                pairwise: None,
                group: None,
            },
            end: None,
        }
    }

    /// The actor of [`Self::new`], constructed in the caller's backing
    /// `storage` field by field: no complete actor is built on a stack.
    #[inline(never)]
    pub fn init_in_place<'s>(
        storage: &'s mut core::mem::MaybeUninit<Self>,
        port: D::Port,
        rates: S,
        receive: RxBlockAckView<'id, D::Types>,
        events: O,
    ) -> oer_portable_memory::Placed<'s, Self> {
        let () = Self::HOLDS_THE_LANE;
        oer_portable_memory::Placed::init(storage, |place| {
            oer_portable_memory::init_in_place!(place, Self {
                port: port,
                rates: rates,
                events: events,
                receive: receive,
                link: None,
                pairwise: None,
                sequence_hold: None,
                data => |place: &mut core::mem::MaybeUninit<DataTx<D::TxMemory>>| {
                    oer_portable_memory::init_in_place!(place, DataTx {
                        plane: None,
                        deferred: None,
                        freed: heapless::Deque::new(),
                    })
                },
                radio: Radio::Stopped,
                held: None,
                stash: None,
                group: None,
                stage => |place: &mut core::mem::MaybeUninit<Stage<'id, D>>| {
                    oer_portable_memory::init_in_place!(place, Stage {
                        cut: heapless::Deque::new(),
                        group_terminal: None,
                        tbtt: None,
                        overflow: None,
                    })
                },
                frames_out: 0,
                reply_out: None,
                owed: Owed::new(),
                facts: Facts::default(),
                rx_later: None,
                again: false,
                armed: None,
                due_streak: 0,
                fact_streak: 0,
                progressed: false,
                now: Instant::EPOCH,
                retained: Retained {
                    link: None,
                    returned: None,
                    completed: None,
                    link_return: None,
                    pairwise: None,
                    group: None,
                },
                end: None,
            })
        })
    }

    /// The port, for its owner's checks between runs.
    pub const fn port(&self) -> &D::Port {
        &self.port
    }

    /// The port, for its owner once the run retired it: the actor no
    /// longer calls it.
    pub fn port_mut(&mut self) -> &mut D::Port {
        &mut self.port
    }

    /// The rate selection.
    pub const fn rates(&self) -> &S {
        &self.rates
    }

    /// The events' sink.
    pub const fn events(&self) -> &O {
        &self.events
    }

    /// Replace the port with the next installation's, keeping the data
    /// plane's state and the network's memory: `reinstall` takes the
    /// stopped port and gives the new one, or the old one back.
    ///
    /// # Errors
    ///
    /// The actor, unchanged but for the port `reinstall` gave back, with
    /// its refusal.
    #[expect(
        clippy::result_large_err,
        reason = "the actor comes back by value: it lives in its owner's placed storage"
    )]
    pub fn reinstall<E>(
        mut self,
        reinstall: impl FnOnce(D::Port) -> Result<D::Port, (D::Port, E)>,
    ) -> Result<Self, (Self, E)> {
        let Self { port, .. } = self;
        let port = match reinstall(port) {
            Ok(port) => port,
            Err((port, error)) => {
                self = Self { port, ..self };
                return Err((self, error));
            }
        };
        let mut next = Self { port, ..self };
        next.reinstalled();
        Ok(next)
    }

    /// Replace the port in place with the next installation's, keeping the
    /// data plane's state and the network's memory: `reinstall` turns the
    /// stopped port into the new one, or refuses and leaves it.
    ///
    /// # Errors
    ///
    /// `reinstall`'s refusal; the actor stays as it was.
    pub fn reinstall_in_place<R, E>(
        &mut self,
        reinstall: impl FnOnce(&mut D::Port) -> Result<R, E>,
    ) -> Result<R, E> {
        let installed = reinstall(&mut self.port)?;
        self.reinstalled();
        Ok(installed)
    }

    /// The state a new installation starts from: it lends new arenas, so
    /// the plane over the old ones ended with them, and its radio stopped.
    fn reinstalled(&mut self) {
        self.data.plane = None;
        self.radio = Radio::Stopped;
        self.end = None;
    }

    /// Serve the port until the lifecycle retires it or a broken contract
    /// ends the run. The clock `timer` is read once per turn.
    pub async fn run<
        const TO: usize,
        const FROM: usize,
        const RETURNS: usize,
        NS: NetworkStop,
        const TX_DEPTH: usize,
        const RX_DEPTH: usize,
        N: Notifies,
    >(
        &mut self,
        lifecycle: &mut LifecycleEnds<'_, 'id, D, TO, FROM, RETURNS>,
        network: &mut NetworkEnds<'_, D::TxMemory, D::LentRx, NS, TX_DEPTH, RX_DEPTH, N>,
        timer: &impl Timer,
    ) -> ActorEnd<<D::Port as DataPort<'id>>::Fault> {
        const {
            if RETURNS < returns_depth(<Ty<'id, D> as PortTypes<'id>>::TERMINAL_CAPACITY) {
                panic!("the return queue holds every terminal the port can owe");
            }
            if FROM < from_actor_depth(RETURNS) {
                panic!("the actor's queue holds the lane, the returns and the answers");
            }
            if core::mem::size_of::<FromActor<'id, D>>() > FROM_ACTOR_BYTES {
                panic!("a message from the actor stays small: owners travel beside it");
            }
            if core::mem::size_of::<ToActor<'id, D>>() > TO_ACTOR_BYTES {
                panic!("a message to the actor stays small: large ones travel as parcels");
            }
        };
        let arm = |at: Option<Instant>| alarm_at(timer, at);
        let mut alarm = core::pin::pin!(arm(None));
        poll_fn(|cx| self.poll(cx, lifecycle, network, timer, alarm.as_mut(), &arm)).await
    }

    /// One poll: take the port's facts, put the next arena on the air when
    /// they ended the published one ([`Self::relay`]), collect what else
    /// is ready, run one turn, and sleep unless a budget ran out with work
    /// left.
    #[allow(
        clippy::too_many_arguments,
        reason = "the poll's ends, clock and alarm are distinct borrows"
    )]
    fn poll<
        F: Future<Output = ()>,
        const TO: usize,
        const FROM: usize,
        const RETURNS: usize,
        NS: NetworkStop,
        const TX_DEPTH: usize,
        const RX_DEPTH: usize,
        N: Notifies,
    >(
        &mut self,
        cx: &mut Context<'_>,
        lifecycle: &mut LifecycleEnds<'_, 'id, D, TO, FROM, RETURNS>,
        network: &mut NetworkEnds<'_, D::TxMemory, D::LentRx, NS, TX_DEPTH, RX_DEPTH, N>,
        timer: &impl Timer,
        mut alarm: Pin<&mut Option<F>>,
        arm: &impl Fn(Option<Instant>) -> Option<F>,
    ) -> Poll<ActorEnd<<D::Port as DataPort<'id>>::Fault>> {
        if let Some(end) = self.end.take() {
            return Poll::Ready(end);
        }
        let reposted = core::mem::take(&mut self.again);
        let fresh = self.collect_facts(cx);
        // The turn's one clock read: a transmit event's relay needs it first.
        let relaying = self.facts.tx.is_some_and(|tx| tx.bits() != 0);
        if relaying {
            self.now = timer.now();
            if let Err(fault) = self.relay(lifecycle, network) {
                self.fault(lifecycle, fault);
            }
            if let Some(end) = self.end.take() {
                return Poll::Ready(end);
            }
        }
        let collected = self.collect_sources(cx, fresh, lifecycle, network, alarm.as_mut(), arm);
        if !collected.signalled && !collected.ready && !reposted {
            return Poll::Pending;
        }
        if !relaying {
            self.now = timer.now();
        }
        D::span(ActorSpan::Turn, false);
        let turned = self.turn(cx, lifecycle, network);
        D::span(ActorSpan::Turn, true);
        if let Err(fault) = turned {
            self.fault(lifecycle, fault);
        }
        if let Some(end) = self.end.take() {
            return Poll::Ready(end);
        }
        // Register for what the turn left waiting. Work a budget left, a
        // source the turn made ready, facts the port's signals gave after
        // the turn or a port service the turn made due takes one repost:
        // the other tasks run first.
        let after = self.collect(cx, lifecycle, network, alarm.as_mut(), arm);
        let facts_again = self.facts_again(after.fresh);
        if after.ready || facts_again || self.due_again() {
            self.again = true;
        }
        if self.again {
            cx.waker().wake_by_ref();
        }
        Poll::Pending
    }
}

/// The alarm at `at`, or none: one function, so every alarm of a run has
/// the one type the run pins.
fn alarm_at<T: Timer>(timer: &T, at: Option<Instant>) -> Option<impl Future<Output = ()> + '_> {
    at.map(|at| timer.wait_until(at))
}

#[cfg(test)]
mod tests;

/// The largest message from the actor: a received frame with its metadata
/// (the S31 receive buffer and its metadata take up to 128 bytes); owners
/// larger than a frame's travel in the return queue.
pub const FROM_ACTOR_BYTES: usize = 128;
