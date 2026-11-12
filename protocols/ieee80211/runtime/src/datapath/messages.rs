//! The messages between the station's lifecycle and the datapath actor,
//! and the ends both tasks hold.
//!
//! Both tasks run on one executor. The lifecycle sends [`ToActor`] and
//! takes [`FromActor`] in order. Owners too large for those queues travel
//! beside them: the station's link in a one-slot mailbox each way, and
//! every terminal or refused owner in the ordered return queue
//! ([`Returned`]), each announced by one [`FromActor::Returned`].

use super::*;

/// A station frame the lifecycle hands the actor: the parts of the
/// station's transmission the port takes. Its context stays with the
/// lifecycle at its ticket's place.
pub struct StationTx<'id, D: DatapathPort<'id>> {
    /// Its ticket.
    pub ticket: ActorTicket<'id, D>,
    /// The buffer holding the frame, from its header on.
    pub buffer: ActorStationBuffer<'id, D>,
    /// The frame's kind: its receiver, header and length.
    pub frame: StaFrame<D::Caps, D::Small>,
    /// What the frame is for, which picks its coexistence priority.
    pub class: StaFrameClass,
    /// Its rate and retry plan.
    pub plan: StaRatePlan,
    /// The access category it contends in.
    pub access_category: WmmAccessCategory,
    /// Who numbers it: the station, or the link as the actor submits it.
    pub number: FrameNumber,
    /// The TID whose next Sequence Number it names: an ADDBA Request, which
    /// ends the hold of the TID's data ([`ToActor::Sequence`]).
    pub names: Option<WmmUserPriority>,
}

/// The pending token of one group key change: it goes to the actor with the
/// change's command and comes back with its terminal, after the actor
/// applied the terminal to the link's replay epoch.
pub enum GroupPending<'id, Path: PoolPath> {
    /// An installation.
    Install(PendingGroupKey<'id, Path>),
    /// A replacement.
    Replace(PendingGroupReplacement<'id, Path>),
    /// A selection of a retained installation.
    Select(PendingGroupSelection<'id, Path>),
    /// A removal.
    Remove(PendingGroupRemoval<'id, Path>),
}

impl<'id, Path: PoolPath> GroupPending<'id, Path> {
    /// The lane change the token names, for the link.
    pub(super) fn lane(&mut self) -> GroupLane<'_, 'id, Path> {
        match self {
            Self::Install(pending) => GroupLane::Installed(pending),
            Self::Replace(pending) => GroupLane::Replaced(pending),
            Self::Select(pending) => GroupLane::Selected(pending),
            Self::Remove(pending) => GroupLane::Removed(pending),
        }
    }
}

/// What the lifecycle tells the actor.
pub enum ToActor<'id, D: DatapathPort<'id>> {
    /// Start the stopped radio on the channel.
    Start(SupportedChannel<D::Caps>),
    /// Move the running radio to the channel: it stops, then starts there.
    Retune(SupportedChannel<D::Caps>),
    /// Take the station's link from the link mailbox.
    Connect,
    /// Give the link back through the return mailbox: the arenas' unpublished
    /// frames and the waiting MSDU go back to the network, the receive
    /// agreements end.
    Disconnect,
    /// The link carries no data for now.
    Hold,
    /// The link carries data again.
    Release,
    /// The transmit agreement of `tid` started, moved or ended.
    Agreement {
        /// Its TID.
        tid: WmmUserPriority,
        /// Its window; `None` once it ended.
        window: Option<TxWindow>,
    },
    /// Tell the next Sequence Number of `tid` ([`FromActor::TxSequence`])
    /// and take no more data of `tid` until a station frame names it
    /// ([`StationTx::names`]) or the next [`Self::Release`].
    Sequence(WmmUserPriority),
    /// Give back the pairwise key kept after the link came back, or lent
    /// before the link ([`FromActor::PairwiseReturned`]).
    ReturnPairwise,
    /// Keep the station's installed pairwise key before the link exists:
    /// a protected frame the station numbered itself
    /// ([`FrameNumber::Keyed`]) draws its packet number from it, until
    /// [`Self::ReturnPairwise`].
    LendPairwise(ActorLinkKey<'id, D>),
    /// The station's RSN failed on an EAPOL frame of the link.
    EapolFaulted,
    /// Take the next parcel: a large message beside the queue.
    Parcel,
    /// The station left its peer.
    Disassociated,
    /// Send a station frame.
    StationTx(StationTx<'id, D>),
    /// Teach the rates a station frame's end, as its context names it.
    Learn {
        /// The frame's rate record.
        rates: FrameRates,
        /// Whether it belongs to the station's current connection.
        current: bool,
        /// How it ended.
        outcome: ActorSingleOutcome<'id, D>,
    },
    /// Take a receive Block Ack action the station validated.
    RxAgreement(PendingRxBlockAck<'id, ActorEpochPath<'id, D>>),
    /// The reply the actor offered at this place left, or will never be
    /// owed again.
    BlockAckReplied(ReplyPlace),
    /// Run a port command.
    Port(ActorCommand<'id, D>),
    /// Open the closed gate.
    OpenGate(GateClosed<'id, ActorGatePath<'id, D>>),
    /// A frame the actor handed the lifecycle comes back.
    Recycle(ActorRxBuffer<'id, D>),
    /// Stop the radio by `deadline`.
    Stop {
        /// When the stop proves nothing more.
        deadline: Instant,
        /// What the stop does to the network side.
        network: NetworkEnd,
    },
    /// End the actor's run: the radio is stopped and the installation
    /// retires.
    Retire,
    /// End the actor's run for the next installation: the radio is stopped,
    /// and its owner reinstalls the port under the same actor.
    Reinstall,
}

/// A message too large for the lifecycle's queue: it waits in the parcel
/// queue, and [`ToActor::Parcel`] takes it in the queue's order.
pub enum Parcel<'id, D: DatapathPort<'id>> {
    /// The station associated: the rates start from its peer.
    Associated(Peer<ActorRxMeta<'id, D>>),
    /// Run a group key change: its command and the station's token.
    Group {
        /// The change's command.
        command: ActorCommand<'id, D>,
        /// The station's pending token.
        pending: GroupPending<'id, ActorEpochPath<'id, D>>,
    },
}

/// What the actor tells the lifecycle, in its order.
pub enum FromActor<'id, D: DatapathPort<'id>> {
    /// A frame for the station: not a data frame of the link, or any frame
    /// before the link. The lifecycle sends it back ([`ToActor::Recycle`]).
    Received {
        /// The frame.
        frame: ActorRxBuffer<'id, D>,
        /// What the hardware observed of it.
        meta: ActorRxMeta<'id, D>,
    },
    /// An EAPOL frame of the link, its packet at `payload` of `frame`. The
    /// lifecycle sends the frame back.
    Eapol {
        /// The MPDU.
        frame: ActorRxBuffer<'id, D>,
        /// Where its EAPOL packet lies in the MPDU.
        payload: core::ops::Range<u16>,
        /// How the MPDU was protected.
        protection: EapolProtection,
    },
    /// The next owner of the return queue.
    Returned,
    /// A TBTT of the station's schedule; later ones replace it until the
    /// lifecycle takes it.
    Tbtt(TbttEvent),
    /// The data plane dropped a frame of `tid` under its agreement and moved
    /// its window to `head`: the access point is owed a BlockAckReq.
    BarOwed {
        /// Its TID.
        tid: WmmUserPriority,
        /// The window's new head.
        head: SequenceNumber,
    },
    /// Data of `tid` was taken; the link numbers its next MPDU `next`.
    TxTraffic {
        /// Its TID.
        tid: WmmUserPriority,
        /// The TID's next Sequence Number.
        next: SequenceNumber,
    },
    /// A reply the receive agreements owe the access point.
    BlockAckReply(RxBlockAckReplyBody<LinkEpoch<'id, ActorEpochPath<'id, D>>>),
    /// MSDUs the data plane refused, coalesced by reason.
    MsduRefused {
        /// Why.
        reason: MsduRefusal,
        /// How many, saturating.
        count: u16,
    },
    /// The link is back in the return mailbox. The actor keeps its pairwise
    /// key until [`ToActor::ReturnPairwise`].
    LinkReturned,
    /// The next Sequence Number of `tid` the lifecycle asked for; the TID's
    /// data waits until a station frame names it.
    TxSequence {
        /// Its TID.
        tid: WmmUserPriority,
        /// The TID's next Sequence Number.
        next: SequenceNumber,
    },
    /// The pairwise key the actor kept after the link came back.
    PairwiseReturned(ActorLinkKey<'id, D>),
    /// The gate is open.
    GateOpened(GateOpen<'id, ActorGatePath<'id, D>>),
    /// The start's end.
    Started(Result<(), StartRefused<<D::Port as DataPort<'id>>::Fault>>),
    /// The retune's end.
    Retuned(Result<(), RetuneFailed<<D::Port as DataPort<'id>>::Fault>>),
    /// The stop's end.
    Stopped(StopOutcome<<D::Port as DataPort<'id>>::Fault>),
    /// The actor ended on a broken contract; it keeps every owner it held.
    Faulted(ActorFault<<D::Port as DataPort<'id>>::Fault>),
    /// A broken contract the actor survives: it serves a stop, and the
    /// station restarts. Never after the lifecycle's retire or reinstall:
    /// the lifecycle ended then, so the fault ends the run
    /// ([`Self::Faulted`]).
    Recovering(ActorFault<<D::Port as DataPort<'id>>::Fault>),
}

/// An owner the actor gives back, in the return queue.
pub enum Returned<'id, D: DatapathPort<'id>> {
    /// A port terminal.
    Terminal {
        /// The terminal.
        terminal: ActorTerminal<'id, D>,
        /// It ends a group key change: the station's token waits in the
        /// group queue.
        grouped: bool,
    },
    /// A command the port refused; nothing changed.
    Command {
        /// The command, with its owners.
        command: ActorCommand<'id, D>,
        /// It was a group key change: the station's token waits in the
        /// group queue.
        grouped: bool,
        /// Why.
        reason: CommandRefusal,
    },
    /// A command the port gave back beside its fault: it did not take the
    /// command's owners. A step of it may have begun with an unknown end,
    /// so the lifecycle never issues it again; the fault's own message
    /// ends or restarts the run.
    Unrun {
        /// The command, with its owners.
        command: ActorCommand<'id, D>,
        /// It was a group key change: the station's token waits in the
        /// group queue.
        grouped: bool,
    },
    /// A station frame that did not go.
    Station {
        /// Its ticket.
        ticket: ActorTicket<'id, D>,
        /// Its buffer.
        buffer: ActorAnyBuffer<'id, D>,
        /// Why.
        reason: TxRefusal,
    },
}

/// Why a retune ended without the radio running on the new channel.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RetuneFailed<F> {
    /// The radio did not confirm its stop.
    Stop(StopUnconfirmed<F>),
    /// The radio did not start on the new channel; it is stopped.
    Start(StartRefused<F>),
}

/// How a stop ended; `F` is the port's fault.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StopOutcome<F> {
    /// The radio is stopped and every owner of the run is back.
    Confirmed,
    /// The radio is stopped and its data settled, but by the deadline the
    /// network had not returned all it holds of the station: its transmit
    /// memory, its stop or its lent receive units. The return goes on: the
    /// actor completes it whenever the network takes its turn, and a
    /// retirement waits for it. Nothing of the radio is unconfirmed.
    Returning(ReturnObserved),
    /// The radio is stopped and every owner of the run is back, the
    /// network's included: its memory went to the return queue and its lent
    /// units came back. But the network's stop participant was lost, so
    /// its stop was never confirmed: a software error for the composition
    /// to record, not a fault of the radio.
    NetworkLost(ReturnObserved),
    /// The stop proved nothing by its deadline: what was still out.
    Unconfirmed(StopObserved<F>),
}

/// What the network still owed a stopped radio at the stop's deadline.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReturnObserved {
    /// The network's transmit memory the station still holds.
    pub memory_held: u16,
    /// The network was asked to stop and has not reported it.
    pub network_running: bool,
    /// Receive units the network still holds.
    pub lent: u16,
}

/// What a stop does to the network side.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NetworkEnd {
    /// The network stays: a restart keeps its stack and its lent buffers
    /// for the next installation.
    Stays,
    /// The radio retires: the stop asks the network to stop, and confirms
    /// once it stopped. Buffers it still holds lent stay lent, counted
    /// against the lend credit, which outlives the installation.
    Stops,
}

/// What a stop still held when it ended unconfirmed; `F` is the port's
/// fault.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StopObserved<F> {
    /// The port's own report, when it ended its stop unconfirmed, with the
    /// fault the stop met.
    pub port: Option<StopUnconfirmed<F>>,
    /// The deadline passed.
    pub deadline: bool,
    /// A data arena's end is still owed.
    pub arena_on_air: bool,
    /// Frames the lifecycle has not sent back.
    pub frames_out: u8,
    /// The stop asked the network to stop and it did not report it.
    pub network_running: bool,
    /// The network's stop participant was lost, which proves no stop.
    pub network_lost: bool,
    /// Receive units the network still holds.
    pub lent: u16,
    /// The network's transmit memory the station still holds: ended
    /// frames' memory the return queue had no room for, and MSDUs left in
    /// the transmit queue.
    pub memory_held: u16,
}

/// A broken contract that ends the actor: the port's named fault, or one
/// of the actor's own. Every owner stays where it was.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActorFault<F> {
    /// The port's fault, with its observations.
    Port(F),
    /// The actor's own contract.
    Contract(ActorViolation),
    /// The port refused a received unit's recycle for its count: the unit
    /// stays with the actor ([`Actor::take_unrecycled`]), which takes no
    /// unit more.
    Receive(oer_protocols_ieee80211_contracts_port::ReceiveAccounting),
    /// A port call returned an item it could neither give the actor nor
    /// keep: the item stays with the actor ([`Actor::take_unkept`]), which
    /// makes no port call that takes owners while it keeps it.
    Unkept,
    /// The port did not accept a submission for want of its right to hand
    /// out the attempt's terminal: the attempt stays with the actor
    /// ([`Actor::take_unaccepted`]), which runs no turn while it keeps it.
    Unaccepted,
}

/// A broken contract the actor names.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActorViolation {
    /// The return queue or its announcement had no room: the producers'
    /// bounds did not hold. The owner stays with the actor.
    OutboxFull,
    /// The lifecycle closed its reception while the actor ran: nothing the
    /// actor sends reaches it, so the actor sends nothing more and does not
    /// wait for its room. The owner it did not send stays with the actor,
    /// or with the port that offered it.
    LifecycleClosed,
    /// The port did not lend its two data arenas.
    ArenaClaim,
    /// A data arena's end named an arena the plane did not publish, or
    /// another number of frames.
    ArenaCompletion,
    /// A link came while the actor held one, or none came with its message.
    LinkMailbox,
    /// A parcel's announcement came without its parcel.
    ParcelMissing,
    /// The lifecycle retired a radio it had not stopped.
    RetireRunning,
    /// The link's return found the mailbox full.
    LinkReturn,
    /// A link came back with a pairwise key while the actor kept another,
    /// the lifecycle lent one while the actor kept another, asked for a key
    /// the actor does not keep, or retired the actor while it kept one.
    PairwiseCustody,
    /// The receive reordering lost its delivery order.
    ReorderOverflow,
    /// The network's receive queue took an MSDU in a slot and lost the
    /// slot's index, which its free index excludes: the MSDU stays in its
    /// slot, unpublished.
    NetworkQueueLost,
}

/// How the actor's run ended.
#[derive(Debug, Eq, PartialEq)]
pub enum ActorEnd<F> {
    /// The lifecycle retired the stopped radio.
    Retired,
    /// The lifecycle stopped the radio for the next installation
    /// ([`Actor::reinstall`]).
    Reinstall,
    /// A broken contract ended it.
    Faulted(ActorFault<F>),
}

/// What the actor decided without telling the lifecycle.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActorEvent {
    /// A frame for the station found its lane to the lifecycle full: it
    /// went back to the ring.
    LaneFull,
    /// A received data frame found no free reordering place: it went back
    /// to the ring and leaves a hole in its order.
    ReorderFull,
    /// A receive Block Ack request found no reply place or agreement slot:
    /// the access point hears nothing and asks again.
    RepliesFull,
}

/// The sink a composition lends the actor: the station service's events
/// and the actor's own.
pub trait ActorEvents: StationEvents + EventSink<ActorEvent> {}

impl<S: StationEvents + EventSink<ActorEvent> + ?Sized> ActorEvents for S {}

/// The actor's ends to the lifecycle: its messages both ways, the return
/// queue and the link mailboxes. Every queue is the same executor's.
pub struct LifecycleEnds<
    'c,
    'id,
    D: DatapathPort<'id>,
    const TO: usize,
    const FROM: usize,
    const RETURNS: usize,
> {
    /// The lifecycle's messages.
    pub to_actor: ClosingSpscReceiver<'c, ToActor<'id, D>, TO>,
    /// The lifecycle's large messages, each announced in `to_actor`.
    pub parcels: ClosingSpscReceiver<'c, Parcel<'id, D>, PARCELS>,
    /// The actor's messages.
    pub from_actor: ClosingSpscSender<'c, FromActor<'id, D>, FROM>,
    /// The owners the actor gives back.
    pub returns: ClosingSpscSender<'c, Returned<'id, D>, RETURNS>,
    /// The station's group tokens, back with their change's end.
    pub groups: ClosingSpscSender<'c, GroupPending<'id, ActorEpochPath<'id, D>>, 1>,
    /// The link the station hands over.
    pub link: ClosingSpscReceiver<'c, ActorLink<'id, D>, 1>,
    /// The link the actor gives back.
    pub link_return: ClosingSpscSender<'c, LinkReturn, 1>,
}

/// The actor's ends to the network stack.
pub struct NetworkEnds<
    'c,
    M,
    L,
    S: NetworkStop,
    const TX_DEPTH: usize,
    const RX_DEPTH: usize,
    N: Notifies = SameHart,
> {
    /// The MSDUs the network stack sends, in the network's memory.
    pub transmit: SlotReceiver<'c, Option<Outgoing<M>>, TX_DEPTH, N::Station, N::Network>,
    /// The network's memory, back once its MSDU ended, with its cost.
    pub returned: SlotSender<'c, Option<TxEnded<M>>, TX_DEPTH, N::Network, N::Station>,
    /// The MSDUs received for the network stack, each in a slot of its
    /// own: a turn stages them and publishes them at once ([`SlotSender::flush`]).
    pub receive: SlotSender<'c, Option<Received<L>>, RX_DEPTH, N::Network, N::Station>,
    /// The copies `receive`'s marks stand for.
    pub receive_copied:
        WakingSpscSender<'c, EthernetFrame, RX_COPIED_DEPTH, N::Network, N::Station>,
    /// The network side: whether it takes frames at all, and its stop.
    pub network: &'c S,
}

/// The return queue's least depth: every terminal the port can owe, one
/// refused command and one refused station frame per ticket fit.
pub const fn returns_depth(terminals: usize) -> usize {
    terminals.saturating_add(1)
}

/// The message queue's least depth from the actor: the frame lane, one
/// announcement per return, the lifecycle's answers (start, retune, stop,
/// link, gate, reply, sequence, pairwise key) and two coalesced facts.
pub const fn from_actor_depth(returns: usize) -> usize {
    LIFECYCLE_FRAMES
        .saturating_add(returns)
        .saturating_add(8)
        .saturating_add(2)
}
