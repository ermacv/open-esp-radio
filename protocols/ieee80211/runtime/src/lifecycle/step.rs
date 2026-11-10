//! The machines' step: every action the arbiter and the station owe,
//! each turned into its message to the actor; an arbiter's answer goes
//! back to the station at once.

use super::outbox::Messages;
use super::*;
use oer_protocols_ieee80211_contracts_port::PortCommand;
use oer_protocols_ieee80211_core_arbiter::ArbiterAction;
use oer_protocols_ieee80211_core_roles_sta::link::LinkAction;
use oer_protocols_ieee80211_core_roles_sta::machine::{
    BlockAckReplyRefusal, RxBlockAckReply, StaAction, StaInput, StaNotice, StaTransmit,
};

/// The messages one unit of the lifecycle's work sends at most: a reply's
/// place, then its action's keyed frame (a lent key, its frame and the
/// key's recall). The actor's queue is checked for this room before each
/// unit, and a unit's outbox has as many places by type.
const ACTION_MESSAGES: usize = UNSENT;

/// A station action's transmission on the port of `D`.
type LifecycleTransmit<'id, D, const GROUPS: usize> =
    StaTransmit<'id, LifecycleTokens<'id, D, GROUPS>, <D as DatapathPort<'id>>::Caps>;

/// What follows an action once it no longer borrows the station.
#[allow(
    clippy::large_enum_variant,
    reason = "an action's messages move once, into the unit's outbox"
)]
enum Follow<'id, D: DatapathPort<'id>, const GROUPS: usize> {
    Nothing,
    /// The action's messages to the actor.
    Send(Messages<'id, D>),
    /// An arbiter's answer back to the station.
    Input(StaInput<'id, LifecycleTokens<'id, D, GROUPS>, D::Caps>),
    /// A frame the station numbered itself under its installed key: the key
    /// goes to the actor for it.
    Keyed(LifecycleTransmit<'id, D, GROUPS>),
    /// The station's notice.
    Notice(StaNotice),
    /// The station failed.
    Failed(StationFailure<super::PortFault<'id, D>>),
}

impl<'id, D: DatapathPort<'id>, const GROUPS: usize> Follow<'id, D, GROUPS> {
    /// One message to the actor.
    const fn one(message: ToActor<'id, D>) -> Self {
        Self::Send(Messages::One(Unsent::ToActor(message)))
    }
}

impl<'id, D, C, X, T, const GROUPS: usize, O> Lifecycle<'id, D, C, X, T, GROUPS, O>
where
    D: LifecyclePort<'id>,
    Ty<'id, D>: PortTypes<
            'id,
            StationConfigureContext = StaConfigurationContext<'id, ActorEpochPath<'id, D>>,
        >,
    C: StaCallbacks,
    X: PortCommands<'id, Types = D::Types, Caps = D::Caps>,
    T: TxContextStorage<LifecycleTxContext<'id, D>>,
    O: StationEvents,
{
    /// Take every action the arbiter and the station owe, while the
    /// actor's queues have room.
    pub(super) fn step<
        const TO: usize,
        const FROM: usize,
        const RETURNS: usize,
        const COMMAND_DEPTH: usize,
        N: Notifies,
    >(
        &mut self,
        actor: &ActorEnds<'_, 'id, D, TO, FROM, RETURNS>,
        io: &mut LifecycleIo<'_, D::Caps, impl ServiceOutputSink<D::Caps>, COMMAND_DEPTH, N>,
    ) {
        let now = self.inner.now;
        loop {
            let mut progress = false;
            while self.inner.end.is_none() && self.inner.ending.is_none() {
                // The unit's right comes before the action moves an owner.
                let Some(right) = self.inner.room(actor) else {
                    break;
                };
                let Some(ArbiterCell::Running(arbiter)) = self.inner.arbiter.as_mut() else {
                    self.inner.give_back(right);
                    break;
                };
                let Some(action) = arbiter.poll_action(now) else {
                    self.inner.give_back(right);
                    break;
                };
                progress = true;
                let message = match action {
                    ArbiterAction::Station(notice) => {
                        self.station
                            .input(now, StaInput::Arbiter(notice), &mut self.inner.events);
                        None
                    }
                    ArbiterAction::AccessPoint(command) => {
                        io.output.emit(ServiceOutput::AccessPoint(command.into()));
                        None
                    }
                    ArbiterAction::CloseGate { gate, reserve: () } => {
                        self.inner
                            .admissions
                            .insert(crate::station::PendingTerminal::Control(
                                crate::station::ControlTerminal::GateClosed,
                            ));
                        Some(ToActor::Port(PortCommand::CloseGate(gate)))
                    }
                    ArbiterAction::Retune(pending) => {
                        let channel = pending.channel();
                        self.inner.retune = Some(pending);
                        self.inner.radio = Radio::Retuning;
                        Some(ToActor::Retune(channel))
                    }
                    ArbiterAction::OpenGate(closed) => Some(ToActor::OpenGate(closed)),
                };
                self.inner
                    .flush(actor, right, Outbox::of(message, Messages::None));
            }
            let mut idle = false;
            while !self.inner.link.loan && self.inner.end.is_none() && self.inner.ending.is_none() {
                let Some(right) = self.inner.room(actor) else {
                    break;
                };
                let follow = {
                    let Some(action) = self.station.poll_action(now, &mut self.inner.events) else {
                        self.inner.give_back(right);
                        idle = true;
                        break;
                    };
                    progress = true;
                    self.inner.action(io, action)
                };
                let messages = self.follow(io, follow);
                self.inner.flush(actor, right, Outbox::of(None, messages));
            }
            // The link goes back once the station owes nothing more: its
            // leave frames, numbered on the link, go under it first. The
            // last poll checked the room and gave the right back.
            if idle
                && self.inner.link.asked
                && self.inner.end.is_none()
                && self.inner.ending.is_none()
                && let Some(right) = self.inner.sending.take()
            {
                self.inner.link.asked = false;
                self.inner.link.returning = true;
                self.inner.flush(
                    actor,
                    right,
                    Outbox::of(Some(ToActor::Disconnect), Messages::None),
                );
                progress = true;
            }
            if self.inner.end.is_none()
                && self.inner.ending.is_none()
                && let Some(right) = self.inner.room(actor)
            {
                progress |= self.reply(actor, io, right);
            }
            if !progress
                || self.inner.end.is_some()
                || self.inner.ending.is_some()
                || self.inner.blocked
            {
                return;
            }
        }
    }

    /// Offer the station the receive Block Ack reply the actor owes, under
    /// the unit's `right`; whether that changed anything.
    fn reply<
        const TO: usize,
        const FROM: usize,
        const RETURNS: usize,
        const COMMAND_DEPTH: usize,
        N: Notifies,
    >(
        &mut self,
        actor: &ActorEnds<'_, 'id, D, TO, FROM, RETURNS>,
        io: &mut LifecycleIo<'_, D::Caps, impl ServiceOutputSink<D::Caps>, COMMAND_DEPTH, N>,
        right: UnitRight,
    ) -> bool {
        let Some(owed) = self.inner.reply.take() else {
            self.inner.give_back(right);
            return false;
        };
        let kept = owed;
        let place = owed.place;
        let Some(reply) = RxBlockAckReply::from_body(owed.association, owed.bssid, owed.body)
        else {
            // Not a reply the station sends: the place is free again.
            self.inner.flush(
                actor,
                right,
                Outbox::of(Some(ToActor::BlockAckReplied(place)), Messages::None),
            );
            return true;
        };
        let (place, follow) = match self
            .station
            .try_block_ack_reply(reply, &mut self.inner.events)
        {
            Ok(action) => (
                Some(ToActor::BlockAckReplied(place)),
                self.inner.action(io, action),
            ),
            Err((_, BlockAckReplyRefusal::Busy)) => {
                // The station has no token for it now: it waits for the next
                // step.
                self.inner.reply = Some(kept);
                self.inner.give_back(right);
                return false;
            }
            Err((_, BlockAckReplyRefusal::Encode)) => {
                (None, Follow::Failed(StationFailure::RxBlockAckEncode))
            }
            Err((_, _)) => (Some(ToActor::BlockAckReplied(place)), Follow::Nothing),
        };
        let messages = self.follow(io, follow);
        self.inner.flush(actor, right, Outbox::of(place, messages));
        true
    }

    /// What follows an action, once it no longer borrows the station: the
    /// unit's messages to the actor.
    fn follow<const COMMAND_DEPTH: usize, N: Notifies>(
        &mut self,
        io: &mut LifecycleIo<'_, D::Caps, impl ServiceOutputSink<D::Caps>, COMMAND_DEPTH, N>,
        follow: Follow<'id, D, GROUPS>,
    ) -> Messages<'id, D> {
        match follow {
            Follow::Nothing => Messages::None,
            Follow::Send(messages) => messages,
            Follow::Input(input) => {
                self.station
                    .input(self.inner.now, input, &mut self.inner.events);
                Messages::None
            }
            Follow::Keyed(mut transmit) => match self.station.lend_key() {
                Some(key) => {
                    // The actor draws the frame's packet number from the key
                    // the station lends it, and gives the key back at once.
                    transmit.number = FrameNumber::Keyed;
                    let frame = self.inner.station_tx(transmit);
                    self.inner.link.key = true;
                    self.inner.link.loan = true;
                    let lend = ToActor::LendPairwise(key);
                    match frame {
                        Some(frame) => Messages::Three(lend, frame, ToActor::ReturnPairwise),
                        None => Messages::Two(Unsent::ToActor(lend), ToActor::ReturnPairwise),
                    }
                }
                // The station's action named its installed key.
                None => {
                    self.inner.end = Some(LifecycleEnd::Contract(LifecycleViolation::Missing));
                    Messages::None
                }
            },
            Follow::Notice(notice) => {
                if let StaNotice::Connected { .. } = notice {
                    // The network's least grants start at the slowest rate
                    // this association's plans can end at.
                    let floor = self.station.peer().map(|peer| {
                        oer_protocols_ieee80211_contracts_port::PhyRate::Legacy(
                            peer.associated.legacy_rates.slowest(),
                        )
                    });
                    io.control.state.floor(floor);
                }
                if let StaNotice::Connected { .. } = notice
                    && self.inner.link.lent
                    && let Some(epoch) = self.inner.link.epoch
                {
                    io.control.state.epoch(epoch);
                }
                io.output.emit(ServiceOutput::Notice(notice.into()));
                match notice {
                    StaNotice::Disconnected(_) => {
                        Messages::One(Unsent::ToActor(ToActor::Disassociated))
                    }
                    _ => Messages::None,
                }
            }
            Follow::Failed(failure) => {
                self.inner.begin_end(Ending::Restart(failure), io);
                Messages::None
            }
        }
    }
}

impl<'id, D, X, T, O> Inner<'id, D, X, T, O>
where
    D: LifecyclePort<'id>,
    Ty<'id, D>: PortTypes<
            'id,
            StationConfigureContext = StaConfigurationContext<'id, ActorEpochPath<'id, D>>,
        >,
    X: PortCommands<'id, Types = D::Types, Caps = D::Caps>,
    T: TxContextStorage<LifecycleTxContext<'id, D>>,
    O: StationEvents,
{
    /// The right for one unit of work, taken before the unit moves its
    /// first owner: none while the actor's queues lack one action's room
    /// (the lifecycle waits for it), nor once a reception closed (the
    /// lifecycle ends before any owner moves,
    /// [`LifecycleViolation::ActorClosed`]), nor after a refused unit.
    pub(super) fn room<const TO: usize, const FROM: usize, const RETURNS: usize>(
        &mut self,
        actor: &ActorEnds<'_, 'id, D, TO, FROM, RETURNS>,
    ) -> Option<UnitRight> {
        if actor.to_actor.is_closed() || actor.parcels.is_closed() || actor.link.is_closed() {
            self.closed();
            return None;
        }
        let room =
            actor.to_actor.free_capacity() >= ACTION_MESSAGES && actor.parcels.free_capacity() >= 1;
        if !room {
            self.blocked = true;
            return None;
        }
        self.sending.take()
    }
    /// The actor's reception closed: the lifecycle ends on it, unless a
    /// refusal named its end already.
    pub(super) fn closed(&mut self) {
        if self.end.is_none() {
            self.end = Some(LifecycleEnd::Contract(LifecycleViolation::ActorClosed));
        }
    }
    /// A unit that sends nothing gives its right back.
    pub(super) fn give_back(&mut self, right: UnitRight) {
        self.sending = Sending::Open(right);
    }
    /// Send a unit's `outbox` in order under its `right`, and give the
    /// right back once all went. A refusal keeps the refused message and
    /// every later one in the outbox, which the lifecycle keeps with the
    /// right ([`RetiredLifecycle::unsent`]); the refusal names the end
    /// unless one was named before.
    pub(super) fn flush<const TO: usize, const FROM: usize, const RETURNS: usize>(
        &mut self,
        actor: &ActorEnds<'_, 'id, D, TO, FROM, RETURNS>,
        right: UnitRight,
        mut outbox: Outbox<'id, D>,
    ) {
        match outbox.send(actor) {
            Ok(()) => self.give_back(right),
            Err(violation) => self.keep(right, outbox, violation),
        }
    }
    /// Keep a refused unit's remainder with the right: no unit follows.
    pub(super) fn keep(
        &mut self,
        right: UnitRight,
        outbox: Outbox<'id, D>,
        violation: LifecycleViolation,
    ) {
        self.sending = Sending::refused(right, outbox);
        if self.end.is_none() {
            self.end = Some(LifecycleEnd::Contract(violation));
        }
    }
    /// One station frame's message: its context waits at its ticket's
    /// place. None when that place refused the context: the frame's owners
    /// stay with the lifecycle's end.
    fn station_tx<const GROUPS: usize>(
        &mut self,
        transmit: LifecycleTransmit<'id, D, GROUPS>,
    ) -> Option<ToActor<'id, D>> {
        let StaTransmit {
            context,
            ticket,
            buffer,
            frame,
            class,
            plan,
            access_category,
            number,
        } = transmit;
        let index = ticket.index();
        let names = context.names_sequence();
        if let Err(context) =
            crate::station::tx_contexts::keep(&mut self.tx_contexts, index, context)
        {
            // The ticket's place holds another context, or lies beyond the
            // storage: the frame does not go.
            self.rejected.get_or_insert(Rejected::Transmit {
                context: Some(context),
                ticket,
                buffer: <Ty<'id, D> as oer_protocols_ieee80211_contracts_port::TxBufferPool<
                    'id,
                    D::Small,
                >>::erase(buffer),
            });
            self.end = Some(LifecycleEnd::Contract(LifecycleViolation::Returned(
                BackendContractViolation::TxContextPlace(index),
            )));
            return None;
        }
        self.admissions
            .insert(crate::station::PendingTerminal::Transmit(index));
        Some(ToActor::StationTx(StationTx {
            ticket,
            buffer,
            frame,
            class,
            plan,
            access_category,
            number,
            names,
        }))
    }
    /// Carry out one action of the station.
    fn action<const GROUPS: usize, const COMMAND_DEPTH: usize, N: Notifies>(
        &mut self,
        io: &mut LifecycleIo<'_, D::Caps, impl ServiceOutputSink<D::Caps>, COMMAND_DEPTH, N>,
        action: StaAction<'_, 'id, LifecycleTokens<'id, D, GROUPS>, D::Caps>,
    ) -> Follow<'id, D, GROUPS> {
        let now = self.now;
        let arbiter = match self.arbiter.as_mut() {
            Some(ArbiterCell::Running(arbiter)) => Some(arbiter),
            _ => None,
        };
        let input = match action {
            StaAction::Link(action) => {
                return match action {
                    LinkAction::HandOver(handover) => {
                        // Counted as the actor's when the unit's messages
                        // are made: a refused one ends the lifecycle, which
                        // reads this custody no more, with the link in its
                        // remainder.
                        self.link.lent = true;
                        self.link.key |= handover.numbers().key().is_some();
                        self.link.epoch = Some(handover.epoch().sequence());
                        Follow::Send(Messages::Two(Unsent::Link(handover), ToActor::Connect))
                    }
                    LinkAction::Hold => Follow::one(ToActor::Hold),
                    LinkAction::Release => Follow::one(ToActor::Release),
                    LinkAction::Agreement { tid, window } => {
                        Follow::one(ToActor::Agreement { tid, window })
                    }
                    // The station's frames numbered on the link go first.
                    LinkAction::Return => {
                        self.link.asked = true;
                        Follow::Nothing
                    }
                    LinkAction::Sequence(tid) => Follow::one(ToActor::Sequence(tid)),
                    LinkAction::ReturnKey => Follow::one(ToActor::ReturnPairwise),
                };
            }
            StaAction::BlockAck(request) => return Follow::one(ToActor::RxAgreement(request)),
            StaAction::RequestChannel(request, place) => match arbiter {
                Some(arbiter) => StaInput::Answer(arbiter.request(now, request, place)),
                None => return Follow::Nothing,
            },
            StaAction::RequestAbsence {
                request,
                home,
                place,
            } => match arbiter {
                Some(arbiter) => {
                    StaInput::AbsenceAnswer(arbiter.request_absence(now, request, home, place))
                }
                None => return Follow::Nothing,
            },
            StaAction::Ready(lease) => {
                if let Some(arbiter) = arbiter {
                    arbiter.ready(lease);
                }
                return Follow::Nothing;
            }
            StaAction::ExchangeLease(old) => match arbiter {
                Some(arbiter) => StaInput::Exchanged(arbiter.exchange(old)),
                None => return Follow::Nothing,
            },
            StaAction::ReleaseChannel(lease) => {
                if let Some(arbiter) = arbiter {
                    arbiter.release(lease);
                }
                return Follow::Nothing;
            }
            StaAction::Upstream(fact) => {
                if let Some(arbiter) = arbiter {
                    arbiter.upstream(now, fact);
                }
                return Follow::Nothing;
            }
            StaAction::Configure(receive, permission) => {
                self.admissions
                    .insert(crate::station::PendingTerminal::Control(
                        crate::station::ControlTerminal::Configure,
                    ));
                return Follow::one(ToActor::Port(PortCommand::Configure {
                    permission,
                    receive,
                }));
            }
            StaAction::SetEdca(edca) => {
                return Follow::one(ToActor::Port(PortCommand::SetEdca(edca)));
            }
            StaAction::SetHe(he) => {
                return Follow::one(ToActor::Port(PortCommand::SetHe(he)));
            }
            StaAction::InstallPairwiseKey {
                association,
                pending,
                slot,
                peer,
                key,
            } => {
                let (command, identity) =
                    self.issuer.install_pairwise(&association, slot, peer, key);
                let key = oer_protocols_ieee80211_core_roles_sta::port::StationPairwiseKey::issued(
                    association,
                    identity,
                    self.step,
                );
                self.pairwise = Some(PendingPairwise { pending, key });
                self.admissions
                    .insert(crate::station::PendingTerminal::Control(
                        crate::station::ControlTerminal::PairwiseInstall,
                    ));
                return Follow::one(ToActor::Port(command));
            }
            StaAction::InstallGroupKey {
                association,
                pending,
                permission,
                slot,
                key_id,
                key,
            } => {
                let identity =
                    <<Ty<'id, D> as PortTypes<'id>>::StationKeys as KeyTypes<'id>>::vacant_group_slot_identity(&slot);
                let command = self
                    .issuer
                    .install_group(association, permission, slot, key_id, key);
                return Follow::Send(self.group_change(
                    identity,
                    Some(key_id),
                    command,
                    GroupPending::Install(pending),
                ));
            }
            StaAction::ReplaceGroupKey {
                pending,
                permission,
                installed,
                key_id,
                key,
            } => {
                let identity =
                    <<Ty<'id, D> as PortTypes<'id>>::StationKeys as KeyTypes<'id>>::group_slot_identity(&installed);
                return Follow::Send(self.group_change(
                    identity,
                    Some(key_id),
                    PortCommand::ReplaceGroup {
                        permission,
                        installed,
                        key_id,
                        key,
                    },
                    GroupPending::Replace(pending),
                ));
            }
            StaAction::SelectGroupKey {
                pending,
                installed,
                permission,
            } => {
                let identity =
                    <<Ty<'id, D> as PortTypes<'id>>::StationKeys as KeyTypes<'id>>::group_slot_identity(&installed);
                return Follow::Send(self.group_change(
                    identity,
                    None,
                    PortCommand::SelectGroup {
                        permission,
                        installed,
                    },
                    GroupPending::Select(pending),
                ));
            }
            StaAction::RemoveGroupKey {
                pending,
                key,
                permission,
            } => {
                let identity =
                    <<Ty<'id, D> as PortTypes<'id>>::StationKeys as KeyTypes<'id>>::group_slot_identity(&key);
                return Follow::Send(self.group_change(
                    identity,
                    None,
                    PortCommand::RemoveGroup { permission, key },
                    GroupPending::Remove(pending),
                ));
            }
            StaAction::RemovePairwiseKey(key) => {
                self.admissions
                    .insert(crate::station::PendingTerminal::Control(
                        crate::station::ControlTerminal::PairwiseRemoval,
                    ));
                let (association, key) = key.into_parts();
                return Follow::one(ToActor::Port(PortCommand::RemovePairwise {
                    association,
                    key,
                }));
            }
            StaAction::Transmit(transmit) => {
                return Follow::Send(Messages::to_actor(self.station_tx(transmit)));
            }
            StaAction::TransmitProtected { transmit, key: _ } => {
                return Follow::Keyed(transmit);
            }
            StaAction::ContractViolation(reason) => {
                return Follow::Failed(StationFailure::StationKeyBank(reason.into()));
            }
            StaAction::Notify(notice) => return Follow::Notice(notice),
            StaAction::CommandCompleted(terminal) => {
                io.output.emit(ServiceOutput::CommandCompleted(
                    self.commands.completed(terminal),
                ));
                return Follow::Nothing;
            }
            StaAction::Associated(peer) => {
                // The rates start from what the association made known.
                return Follow::Send(Messages::Two(
                    Unsent::Parcel(Parcel::Associated(peer)),
                    ToActor::Parcel,
                ));
            }
        };
        Follow::Input(input)
    }
    /// Send a group key change with its token as a parcel; the lifecycle
    /// keeps the slot it names until its terminal.
    fn group_change(
        &mut self,
        slot: <<Ty<'id, D> as PortTypes<'id>>::StationKeys as KeyTypes<'id>>::GroupSlotIdentity,
        key_id: Option<oer_protocols_ieee80211_contracts_port::GroupKeyId>,
        command: ActorCommand<'id, D>,
        pending: GroupPending<'id, ActorEpochPath<'id, D>>,
    ) -> Messages<'id, D> {
        self.group = Some(PendingGroup { slot, key_id });
        self.admissions
            .insert(crate::station::PendingTerminal::Control(
                crate::station::ControlTerminal::GroupChange,
            ));
        Messages::Two(
            Unsent::Parcel(Parcel::Group { command, pending }),
            ToActor::Parcel,
        )
    }
}
