//! The machines' step: every action the arbiter and the station owe,
//! each turned into its message to the actor; an arbiter's answer goes
//! back to the station at once.

use super::*;
use oer_protocols_ieee80211_contracts_port::PortCommand;
use oer_protocols_ieee80211_core_arbiter::ArbiterAction;
use oer_protocols_ieee80211_core_roles_sta::link::LinkAction;
use oer_protocols_ieee80211_core_roles_sta::machine::{
    BlockAckReplyRefusal, RxBlockAckReply, StaAction, StaInput, StaNotice, StaTransmit,
};

/// The messages one action sends at most: a lent key, its frame and the
/// key's recall.
const ACTION_MESSAGES: usize = 3;

/// A station action's transmission on the port of `D`.
type LifecycleTransmit<'id, D, const GROUPS: usize> =
    StaTransmit<'id, LifecycleTokens<'id, D, GROUPS>, <D as DatapathPort<'id>>::Caps>;

/// What follows an action once it no longer borrows the station.
enum Follow<'id, D: DatapathPort<'id>, const GROUPS: usize> {
    Nothing,
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
            while self.inner.room(actor) && self.inner.end.is_none() && self.inner.ending.is_none()
            {
                let Some(ArbiterCell::Running(arbiter)) = self.inner.arbiter.as_mut() else {
                    break;
                };
                let Some(action) = arbiter.poll_action(now) else {
                    break;
                };
                progress = true;
                match action {
                    ArbiterAction::Station(notice) => {
                        self.station
                            .input(now, StaInput::Arbiter(notice), &mut self.inner.events);
                    }
                    ArbiterAction::AccessPoint(command) => {
                        io.output.emit(ServiceOutput::AccessPoint(command.into()));
                    }
                    ArbiterAction::CloseGate { gate, reserve: () } => {
                        self.inner
                            .admissions
                            .insert(crate::station::PendingTerminal::Control(
                                crate::station::ControlTerminal::GateClosed,
                            ));
                        self.inner
                            .send(actor, ToActor::Port(PortCommand::CloseGate(gate)));
                    }
                    ArbiterAction::Retune(pending) => {
                        let channel = pending.channel();
                        self.inner.retune = Some(pending);
                        self.inner.radio = Radio::Retuning;
                        self.inner.send(actor, ToActor::Retune(channel));
                    }
                    ArbiterAction::OpenGate(closed) => {
                        self.inner.send(actor, ToActor::OpenGate(closed));
                    }
                }
            }
            let mut idle = false;
            while !self.inner.link.loan
                && self.inner.end.is_none()
                && self.inner.ending.is_none()
                && self.inner.room(actor)
            {
                let follow = {
                    let Some(action) = self.station.poll_action(now, &mut self.inner.events) else {
                        idle = true;
                        break;
                    };
                    progress = true;
                    self.inner.action(actor, io, action)
                };
                self.follow(actor, io, follow);
            }
            // The link goes back once the station owes nothing more: its
            // leave frames, numbered on the link, go under it first. The
            // last poll checked the room.
            if idle
                && self.inner.link.asked
                && self.inner.end.is_none()
                && self.inner.ending.is_none()
            {
                self.inner.link.asked = false;
                self.inner.link.returning = true;
                self.inner.send(actor, ToActor::Disconnect);
                progress = true;
            }
            if self.inner.end.is_none() && self.inner.ending.is_none() && self.inner.room(actor) {
                progress |= self.reply(actor, io);
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

    /// Offer the station the receive Block Ack reply the actor owes; whether
    /// that changed anything.
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
    ) -> bool {
        let Some(owed) = self.inner.reply.take() else {
            return false;
        };
        let kept = owed;
        let place = owed.place;
        let Some(reply) = RxBlockAckReply::from_body(owed.association, owed.bssid, owed.body)
        else {
            // Not a reply the station sends: the place is free again.
            self.inner.send(actor, ToActor::BlockAckReplied(place));
            return true;
        };
        let follow = match self
            .station
            .try_block_ack_reply(reply, &mut self.inner.events)
        {
            Ok(action) => {
                self.inner.send(actor, ToActor::BlockAckReplied(place));
                self.inner.action(actor, io, action)
            }
            Err((_, BlockAckReplyRefusal::Busy)) => {
                // The station has no token for it now: it waits for the next
                // step.
                self.inner.reply = Some(kept);
                return false;
            }
            Err((_, BlockAckReplyRefusal::Encode)) => {
                Follow::Failed(StationFailure::RxBlockAckEncode)
            }
            Err((_, _)) => {
                self.inner.send(actor, ToActor::BlockAckReplied(place));
                Follow::Nothing
            }
        };
        self.follow(actor, io, follow);
        true
    }

    /// What follows an action, once it no longer borrows the station.
    fn follow<
        const TO: usize,
        const FROM: usize,
        const RETURNS: usize,
        const COMMAND_DEPTH: usize,
        N: Notifies,
    >(
        &mut self,
        actor: &ActorEnds<'_, 'id, D, TO, FROM, RETURNS>,
        io: &mut LifecycleIo<'_, D::Caps, impl ServiceOutputSink<D::Caps>, COMMAND_DEPTH, N>,
        follow: Follow<'id, D, GROUPS>,
    ) {
        match follow {
            Follow::Nothing => {}
            Follow::Input(input) => {
                self.station
                    .input(self.inner.now, input, &mut self.inner.events)
            }
            Follow::Keyed(mut transmit) => match self.station.lend_key() {
                Some(key) => {
                    // The actor draws the frame's packet number from the key
                    // the station lends it, and gives the key back at once.
                    self.inner.send(actor, ToActor::LendPairwise(key));
                    transmit.number = FrameNumber::Keyed;
                    self.inner.station_tx(actor, transmit);
                    self.inner.send(actor, ToActor::ReturnPairwise);
                    self.inner.link.key = true;
                    self.inner.link.loan = true;
                }
                // The station's action named its installed key.
                None => self.inner.end = Some(LifecycleEnd::Contract(LifecycleViolation::Missing)),
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
                if let StaNotice::Disconnected(_) = notice {
                    self.inner.send(actor, ToActor::Disassociated);
                }
            }
            Follow::Failed(failure) => self.inner.begin_end(Ending::Restart(failure), io),
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
    /// Whether the actor's queues take one more action's messages.
    fn room<const TO: usize, const FROM: usize, const RETURNS: usize>(
        &mut self,
        actor: &ActorEnds<'_, 'id, D, TO, FROM, RETURNS>,
    ) -> bool {
        let room =
            actor.to_actor.free_capacity() >= ACTION_MESSAGES && actor.parcels.free_capacity() >= 1;
        if !room {
            self.blocked = true;
        }
        room
    }
    /// Send `message`; a full queue, which [`Self::room`] checked, is a
    /// broken contract.
    pub(super) fn send<const TO: usize, const FROM: usize, const RETURNS: usize>(
        &mut self,
        actor: &ActorEnds<'_, 'id, D, TO, FROM, RETURNS>,
        message: ToActor<'id, D>,
    ) {
        if let Err(refused) = actor.to_actor.try_send(message) {
            let _quarantined = core::mem::ManuallyDrop::new(refused.0);
            self.end = Some(LifecycleEnd::Contract(LifecycleViolation::QueueFull));
        }
    }
    /// Send one station frame: its context waits at its ticket's place.
    fn station_tx<const GROUPS: usize, const TO: usize, const FROM: usize, const RETURNS: usize>(
        &mut self,
        actor: &ActorEnds<'_, 'id, D, TO, FROM, RETURNS>,
        transmit: LifecycleTransmit<'id, D, GROUPS>,
    ) {
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
            return;
        }
        self.admissions
            .insert(crate::station::PendingTerminal::Transmit(index));
        self.send(
            actor,
            ToActor::StationTx(StationTx {
                ticket,
                buffer,
                frame,
                class,
                plan,
                access_category,
                number,
                names,
            }),
        );
    }
    /// Carry out one action of the station.
    fn action<
        const GROUPS: usize,
        const TO: usize,
        const FROM: usize,
        const RETURNS: usize,
        const COMMAND_DEPTH: usize,
        N: Notifies,
    >(
        &mut self,
        actor: &ActorEnds<'_, 'id, D, TO, FROM, RETURNS>,
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
                match action {
                    LinkAction::HandOver(handover) => {
                        let keyed = handover.numbers().key().is_some();
                        let epoch = handover.epoch().sequence();
                        if let Err(refused) = actor.link.try_send(handover) {
                            let _quarantined = core::mem::ManuallyDrop::new(refused.0);
                            self.end = Some(LifecycleEnd::Contract(LifecycleViolation::QueueFull));
                            return Follow::Nothing;
                        }
                        self.link.lent = true;
                        self.link.key |= keyed;
                        self.link.epoch = Some(epoch);
                        self.send(actor, ToActor::Connect);
                    }
                    LinkAction::Hold => self.send(actor, ToActor::Hold),
                    LinkAction::Release => self.send(actor, ToActor::Release),
                    LinkAction::Agreement { tid, window } => {
                        self.send(actor, ToActor::Agreement { tid, window })
                    }
                    // The station's frames numbered on the link go first.
                    LinkAction::Return => self.link.asked = true,
                    LinkAction::Sequence(tid) => self.send(actor, ToActor::Sequence(tid)),
                    LinkAction::ReturnKey => self.send(actor, ToActor::ReturnPairwise),
                }
                return Follow::Nothing;
            }
            StaAction::BlockAck(request) => {
                self.send(actor, ToActor::RxAgreement(request));
                return Follow::Nothing;
            }
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
                self.send(
                    actor,
                    ToActor::Port(PortCommand::Configure {
                        permission,
                        receive,
                    }),
                );
                return Follow::Nothing;
            }
            StaAction::SetEdca(edca) => {
                self.send(actor, ToActor::Port(PortCommand::SetEdca(edca)));
                return Follow::Nothing;
            }
            StaAction::SetHe(he) => {
                self.send(actor, ToActor::Port(PortCommand::SetHe(he)));
                return Follow::Nothing;
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
                self.send(actor, ToActor::Port(command));
                return Follow::Nothing;
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
                self.group_change(
                    actor,
                    identity,
                    Some(key_id),
                    command,
                    GroupPending::Install(pending),
                );
                return Follow::Nothing;
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
                self.group_change(
                    actor,
                    identity,
                    Some(key_id),
                    PortCommand::ReplaceGroup {
                        permission,
                        installed,
                        key_id,
                        key,
                    },
                    GroupPending::Replace(pending),
                );
                return Follow::Nothing;
            }
            StaAction::SelectGroupKey {
                pending,
                installed,
                permission,
            } => {
                let identity =
                    <<Ty<'id, D> as PortTypes<'id>>::StationKeys as KeyTypes<'id>>::group_slot_identity(&installed);
                self.group_change(
                    actor,
                    identity,
                    None,
                    PortCommand::SelectGroup {
                        permission,
                        installed,
                    },
                    GroupPending::Select(pending),
                );
                return Follow::Nothing;
            }
            StaAction::RemoveGroupKey {
                pending,
                key,
                permission,
            } => {
                let identity =
                    <<Ty<'id, D> as PortTypes<'id>>::StationKeys as KeyTypes<'id>>::group_slot_identity(&key);
                self.group_change(
                    actor,
                    identity,
                    None,
                    PortCommand::RemoveGroup { permission, key },
                    GroupPending::Remove(pending),
                );
                return Follow::Nothing;
            }
            StaAction::RemovePairwiseKey(key) => {
                self.admissions
                    .insert(crate::station::PendingTerminal::Control(
                        crate::station::ControlTerminal::PairwiseRemoval,
                    ));
                let (association, key) = key.into_parts();
                self.send(
                    actor,
                    ToActor::Port(PortCommand::RemovePairwise { association, key }),
                );
                return Follow::Nothing;
            }
            StaAction::Transmit(transmit) => {
                self.station_tx(actor, transmit);
                return Follow::Nothing;
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
                if let Err(refused) = actor.parcels.try_send(Parcel::Associated(peer)) {
                    let _quarantined = core::mem::ManuallyDrop::new(refused.0);
                    self.end = Some(LifecycleEnd::Contract(LifecycleViolation::QueueFull));
                    return Follow::Nothing;
                }
                self.send(actor, ToActor::Parcel);
                return Follow::Nothing;
            }
        };
        Follow::Input(input)
    }
    /// Send a group key change with its token as a parcel; the lifecycle
    /// keeps the slot it names until its terminal.
    fn group_change<const TO: usize, const FROM: usize, const RETURNS: usize>(
        &mut self,
        actor: &ActorEnds<'_, 'id, D, TO, FROM, RETURNS>,
        slot: <<Ty<'id, D> as PortTypes<'id>>::StationKeys as KeyTypes<'id>>::GroupSlotIdentity,
        key_id: Option<oer_protocols_ieee80211_contracts_port::GroupKeyId>,
        command: ActorCommand<'id, D>,
        pending: GroupPending<'id, ActorEpochPath<'id, D>>,
    ) {
        self.group = Some(PendingGroup { slot, key_id });
        self.admissions
            .insert(crate::station::PendingTerminal::Control(
                crate::station::ControlTerminal::GroupChange,
            ));
        if let Err(refused) = actor.parcels.try_send(Parcel::Group { command, pending }) {
            let _quarantined = core::mem::ManuallyDrop::new(refused.0);
            self.end = Some(LifecycleEnd::Contract(LifecycleViolation::QueueFull));
            return;
        }
        self.send(actor, ToActor::Parcel);
    }
}
