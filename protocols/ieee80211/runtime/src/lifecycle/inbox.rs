//! The actor's messages: frames for the station, the owners it gives back
//! and its answers, each taken in the order the actor sent it.

use super::*;
use crate::station::{ControlTerminal, PendingTerminal, RxDrop};
use oer_protocols_ieee80211_contracts_port::{
    Configured, InstalledGroup, InstalledPairwise, LowerMacTerminal, Removed, RemovedKey, RxBuffer,
    TerminalParts, TxCompleted, TxMpdu as _,
};
use oer_protocols_ieee80211_core_arbiter::RetuneRefused;
use oer_protocols_ieee80211_core_roles_sta::machine::TxRefusal;
use oer_protocols_ieee80211_core_roles_sta::machine::{StaEapol, StaInput, StaTraffic};

use crate::datapath::{RetuneFailed, StopOutcome};

/// The messages taking one of the actor's messages sends at most.
const ANSWER_MESSAGES: usize = 2;

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
    /// Take every message the actor sent, while its queue has room for
    /// the answers.
    pub(super) fn inbox<
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
        while self.inner.end.is_none() {
            if actor.to_actor.free_capacity() < ANSWER_MESSAGES {
                self.inner.blocked = true;
                return;
            }
            let Some(message) = actor.from_actor.try_receive() else {
                return;
            };
            self.message(actor, io, message);
            // The station owes its actions before its next input, as it
            // would after any input.
            if self.stepping() {
                self.step(actor, io);
            }
        }
    }

    /// Take one message of the actor.
    fn message<
        const TO: usize,
        const FROM: usize,
        const RETURNS: usize,
        const COMMAND_DEPTH: usize,
        N: Notifies,
    >(
        &mut self,
        actor: &mut ActorEnds<'_, 'id, D, TO, FROM, RETURNS>,
        io: &mut LifecycleIo<'_, D::Caps, impl ServiceOutputSink<D::Caps>, COMMAND_DEPTH, N>,
        message: FromActor<'id, D>,
    ) {
        let now = self.inner.now;
        match message {
            FromActor::Received { frame, meta } => {
                let received =
                    self.station
                        .receive(now, frame.bytes(), meta, &mut self.inner.events);
                if let Err(reason) = received {
                    report(&mut self.inner.events, || {
                        ServiceEvent::RxFrameDropped(RxDrop::Station(reason))
                    });
                }
                self.inner.send(actor, ToActor::Recycle(frame));
            }
            FromActor::Eapol {
                frame,
                payload,
                protection,
            } => {
                let bytes = frame
                    .bytes()
                    .get(usize::from(payload.start)..usize::from(payload.end));
                let taken = match bytes {
                    Some(packet) => self.station.eapol(packet, protection),
                    None => Err(
                        oer_protocols_ieee80211_core_upper_mac::receive::RxDropReason::Payload
                            .into(),
                    ),
                };
                match taken {
                    Ok(StaEapol::Taken) => {}
                    Ok(StaEapol::Faulted) => self.inner.send(actor, ToActor::EapolFaulted),
                    Err(reason) => report(&mut self.inner.events, || {
                        ServiceEvent::RxFrameDropped(RxDrop::Station(reason))
                    }),
                }
                self.inner.send(actor, ToActor::Recycle(frame));
            }
            FromActor::Returned => match actor.returns.try_receive() {
                Some(returned) => self.returned(actor, io, returned),
                None => self.inner.end = Some(LifecycleEnd::Contract(LifecycleViolation::Missing)),
            },
            // The station keeps its own schedule.
            FromActor::Tbtt(_) => {}
            FromActor::BarOwed { tid, head } => self.station.tx_block_ack_request(tid, head),
            FromActor::TxTraffic { tid, next } => {
                // The step after this message takes what the traffic owes.
                let _owed: StaTraffic = self.station.tx_traffic(tid, next, now);
            }
            FromActor::BlockAckReply(reply) => self.inner.reply = Some(reply),
            FromActor::MsduRefused { reason, count } => {
                for _ in 0..count {
                    io.output.emit(ServiceOutput::MsduRefused { reason });
                }
            }
            FromActor::LinkReturned => match actor.link_return.try_receive() {
                Some(link) => {
                    self.station.link_returned(link);
                    self.inner.link.lent = false;
                    self.inner.link.returning = false;
                    self.inner.link.epoch = None;
                }
                None => self.inner.end = Some(LifecycleEnd::Contract(LifecycleViolation::Missing)),
            },
            FromActor::TxSequence { tid, next } => self.station.tx_sequence(tid, next),
            FromActor::PairwiseReturned(key) => {
                self.station.key_returned(key);
                self.inner.link.key = false;
                self.inner.link.loan = false;
            }
            FromActor::GateOpened(open) => {
                if let Some(ArbiterCell::Running(arbiter)) = self.inner.arbiter.as_mut() {
                    arbiter.gate_opened(open);
                }
            }
            FromActor::Started(Ok(())) => self.inner.radio = Radio::Running,
            FromActor::Started(Err(refused)) => {
                // The radio never ran: nothing to take back but the stop.
                self.inner.ending =
                    Some((Ending::StartRefused(refused), now, super::EndStage::Begin));
            }
            FromActor::Retuned(result) => self.retuned(actor, io, result),
            FromActor::Stopped(outcome) => self.stopped(actor, outcome),
            FromActor::Faulted(fault) => self.inner.end = Some(LifecycleEnd::Faulted(fault)),
            FromActor::Recovering(fault) => self
                .inner
                .begin_end(Ending::Restart(StationFailure::Fault(fault)), io),
        }
    }

    /// The radio's move ended: the arbiter hears it.
    fn retuned<
        const TO: usize,
        const FROM: usize,
        const RETURNS: usize,
        const COMMAND_DEPTH: usize,
        N: Notifies,
    >(
        &mut self,
        actor: &ActorEnds<'_, 'id, D, TO, FROM, RETURNS>,
        io: &mut LifecycleIo<'_, D::Caps, impl ServiceOutputSink<D::Caps>, COMMAND_DEPTH, N>,
        result: Result<(), RetuneFailed<super::PortFault<'id, D>>>,
    ) {
        let Some(pending) = self.inner.retune.take() else {
            self.inner.end = Some(LifecycleEnd::Contract(
                LifecycleViolation::RetuneWithoutRequest,
            ));
            return;
        };
        match result {
            Ok(()) => {
                self.inner.radio = Radio::Running;
                if let Some(ArbiterCell::Running(arbiter)) = self.inner.arbiter.as_mut() {
                    arbiter.retuned(pending);
                }
            }
            Err(RetuneFailed::Stop(unconfirmed)) => {
                self.inner.retune = Some(pending);
                self.inner.end = Some(LifecycleEnd::RetuneUnconfirmed(unconfirmed));
            }
            Err(RetuneFailed::Start(refused)) => match self.inner.arbiter.take() {
                Some(ArbiterCell::Running(arbiter)) => match arbiter.retune_refused(pending) {
                    RetuneRefused::Return {
                        arbiter,
                        pending: next,
                    } => {
                        let channel = next.channel();
                        self.inner.arbiter = Some(ArbiterCell::Running(arbiter));
                        self.inner.retune = Some(next);
                        self.inner.send(actor, ToActor::Retune(channel));
                        // A stopped radio receives only notices: the return
                        // came with its own token.
                        self.arbiter_notices(io);
                    }
                    RetuneRefused::Failed(failed) => {
                        self.inner.arbiter = Some(ArbiterCell::Retired(
                            oer_protocols_ieee80211_core_arbiter::RetiredArbiter::PortFailed(
                                failed,
                            ),
                        ));
                        self.inner.radio = Radio::Running;
                        self.inner
                            .begin_end(Ending::Restart(StationFailure::RetuneRefused(refused)), io);
                    }
                },
                other => {
                    self.inner.arbiter = other;
                    self.inner.retune = Some(pending);
                    self.inner.end = Some(LifecycleEnd::Contract(
                        LifecycleViolation::RetuneWithoutRequest,
                    ));
                }
            },
        }
    }

    /// Hand the arbiter's notices to the station.
    fn arbiter_notices<const COMMAND_DEPTH: usize, N: Notifies>(
        &mut self,
        io: &mut LifecycleIo<'_, D::Caps, impl ServiceOutputSink<D::Caps>, COMMAND_DEPTH, N>,
    ) {
        let now = self.inner.now;
        let Some(ArbiterCell::Running(arbiter)) = self.inner.arbiter.as_mut() else {
            return;
        };
        while let Some(notice) = arbiter.poll_notice(now) {
            match notice {
                ArbiterNotice::Station(notice) => {
                    self.station
                        .input(now, StaInput::Arbiter(notice), &mut self.inner.events)
                }
                ArbiterNotice::AccessPoint(command) => {
                    io.output.emit(ServiceOutput::AccessPoint(command.into()))
                }
            }
        }
    }

    /// The radio's stop ended: a confirmed one ends the actor's run.
    fn stopped<const TO: usize, const FROM: usize, const RETURNS: usize>(
        &mut self,
        actor: &ActorEnds<'_, 'id, D, TO, FROM, RETURNS>,
        outcome: StopOutcome,
    ) {
        let Some((why, _, super::EndStage::Radio)) = self.inner.ending else {
            self.inner.end = Some(LifecycleEnd::Contract(LifecycleViolation::Missing));
            return;
        };
        if let StopOutcome::Unconfirmed(observed) = outcome {
            self.inner.end = Some(LifecycleEnd::StopUnconfirmed(observed));
            return;
        }
        let (message, end) = match (why, outcome) {
            // The radio stopped; the actor retires once the network's late
            // return came.
            (Ending::Stop, StopOutcome::Returning(observed)) => {
                (ToActor::Retire, LifecycleEnd::StoppedReturning(observed))
            }
            // The radio stopped and the network's owners are back; only its
            // stop was never confirmed.
            (Ending::Stop, StopOutcome::NetworkLost(observed)) => {
                (ToActor::Retire, LifecycleEnd::StoppedNetworkLost(observed))
            }
            (Ending::Stop, _) => (ToActor::Retire, LifecycleEnd::Stopped),
            (Ending::StartRefused(refused), _) => {
                (ToActor::Retire, LifecycleEnd::StartRefused(refused))
            }
            (Ending::Restart(failure), _) => (ToActor::Reinstall, LifecycleEnd::Restart(failure)),
        };
        self.inner.send(actor, message);
        if self.inner.end.is_none() {
            self.inner.end = Some(end);
        }
    }

    /// An owner the actor gave back.
    fn returned<
        const TO: usize,
        const FROM: usize,
        const RETURNS: usize,
        const COMMAND_DEPTH: usize,
        N: Notifies,
    >(
        &mut self,
        actor: &mut ActorEnds<'_, 'id, D, TO, FROM, RETURNS>,
        io: &mut LifecycleIo<'_, D::Caps, impl ServiceOutputSink<D::Caps>, COMMAND_DEPTH, N>,
        returned: Returned<'id, D>,
    ) {
        match returned {
            Returned::Terminal { terminal, grouped } => {
                let group = if grouped {
                    match actor.groups.try_receive() {
                        Some(group) => Some(group),
                        None => {
                            self.inner
                                .reject(Rejected::Terminal(terminal), LifecycleViolation::Missing);
                            return;
                        }
                    }
                } else {
                    None
                };
                self.terminal(actor, terminal, group);
            }
            Returned::Command {
                command,
                grouped,
                reason: CommandRefusal::NotEnabled,
            } if matches!(self.inner.ending, Some((_, _, super::EndStage::Radio))) => {
                // The end's stop withdrew a command held at its barrier:
                // its owners and the change's token drop with this
                // installation's retired banks.
                if grouped && actor.groups.try_receive().is_none() {
                    self.inner
                        .reject(Rejected::Command(command), LifecycleViolation::Missing);
                }
            }
            Returned::Command {
                command,
                grouped,
                reason,
            } => {
                if grouped {
                    match actor.groups.try_receive() {
                        Some(group) => {
                            // The change's token stays with the retired
                            // lifecycle beside its command.
                            let _kept = core::mem::ManuallyDrop::new(group);
                        }
                        None => {
                            self.inner
                                .reject(Rejected::Command(command), LifecycleViolation::Missing);
                            return;
                        }
                    }
                }
                let kind = command.kind();
                self.inner
                    .rejected
                    .get_or_insert(Rejected::Command(command));
                self.inner.begin_end(
                    Ending::Restart(StationFailure::CommandRefused { kind, reason }),
                    io,
                );
            }
            Returned::Station {
                ticket,
                buffer,
                reason,
            } => {
                io.output.emit(ServiceOutput::TxRefused {
                    reason: reason.into(),
                });
                self.station_returned(ticket, buffer, Err(reason));
            }
        }
    }

    /// A station frame's end: its rate lesson, when its terminal came, and
    /// the station's input, with the context waiting at its ticket's place.
    fn station_returned(
        &mut self,
        ticket: LifecycleTicket<'id, D>,
        buffer: ActorAnyBuffer<'id, D>,
        end: Result<crate::datapath::ActorSingleOutcome<'id, D>, TxRefusal>,
    ) {
        let index = ticket.index();
        let admitted = self
            .inner
            .admissions
            .remove(PendingTerminal::Transmit(index));
        let context = if admitted {
            crate::station::tx_contexts::take(&mut self.inner.tx_contexts, index)
        } else {
            None
        };
        let Some(context) = context else {
            let violation = if admitted {
                BackendContractViolation::TxContextPlace(index)
            } else {
                BackendContractViolation::UnexpectedTerminal(PendingTerminal::Transmit(index))
            };
            self.inner.reject(
                Rejected::Transmit {
                    context: None,
                    ticket,
                    buffer,
                },
                LifecycleViolation::Returned(violation),
            );
            return;
        };
        let buffer = match D::tx_buffer(buffer) {
            Ok(buffer) => buffer,
            Err(buffer) => {
                self.inner.reject(
                    Rejected::Transmit {
                        context: Some(context),
                        ticket,
                        buffer,
                    },
                    LifecycleViolation::Returned(BackendContractViolation::TxBufferClass),
                );
                return;
            }
        };
        let input = match end {
            Ok(completion) => {
                self.inner.lesson = Some((
                    context.frame_rates(),
                    self.station.is_current(&context),
                    completion,
                ));
                StaInput::TxDone {
                    context,
                    ticket,
                    buffer,
                    completion,
                }
            }
            Err(reason) => StaInput::Unsent {
                context,
                ticket,
                buffer,
                reason,
            },
        };
        self.station
            .input(self.inner.now, input, &mut self.inner.events);
    }

    /// One terminal of the port.
    fn terminal<const TO: usize, const FROM: usize, const RETURNS: usize>(
        &mut self,
        actor: &ActorEnds<'_, 'id, D, TO, FROM, RETURNS>,
        terminal: ActorTerminal<'id, D>,
        group: Option<GroupPending<'id, ActorEpochPath<'id, D>>>,
    ) {
        let now = self.inner.now;
        let control = match &terminal {
            LowerMacTerminal::GateClosed(_) => Some(ControlTerminal::GateClosed),
            LowerMacTerminal::Configured(Configured::Station(_)) => {
                Some(ControlTerminal::Configure)
            }
            LowerMacTerminal::KeyRemoved(Removed::Station(RemovedKey::Pairwise { .. })) => {
                Some(ControlTerminal::PairwiseRemoval)
            }
            LowerMacTerminal::PairwiseKeyInstalled(InstalledPairwise::Station(_)) => {
                Some(ControlTerminal::PairwiseInstall)
            }
            _ => None,
        };
        if let Some(control) = control
            && !self
                .inner
                .admissions
                .remove(PendingTerminal::Control(control))
        {
            self.inner.reject(
                Rejected::Terminal(terminal),
                LifecycleViolation::Returned(BackendContractViolation::UnexpectedTerminal(
                    PendingTerminal::Control(control),
                )),
            );
            return;
        }
        match terminal {
            LowerMacTerminal::TxCompleted(TxCompleted::Single(terminal)) => {
                let TerminalParts {
                    context: (),
                    ticket,
                    payload,
                    outcome,
                } = terminal.into_parts(&mut self.inner.terminal_access);
                let (buffer, body) = payload.into_parts();
                if let Some(body) = body {
                    match body {}
                }
                self.station_returned(ticket, buffer, Ok(outcome));
                if let Some((rates, current, outcome)) = self.inner.lesson.take() {
                    self.inner.send(
                        actor,
                        ToActor::Learn {
                            rates,
                            current,
                            outcome,
                        },
                    );
                }
            }
            LowerMacTerminal::GateClosed(closed) => {
                if let Some(ArbiterCell::Running(arbiter)) = self.inner.arbiter.as_mut() {
                    arbiter.gate_closed(closed);
                }
            }
            LowerMacTerminal::Configured(Configured::Station(permission)) => {
                self.station.input(
                    now,
                    StaInput::Configured(permission),
                    &mut self.inner.events,
                );
            }
            LowerMacTerminal::KeyRemoved(Removed::Station(RemovedKey::Pairwise {
                slot,
                association,
            })) => {
                self.station.input(
                    now,
                    StaInput::PairwiseKeyRemoved { slot, association },
                    &mut self.inner.events,
                );
            }
            terminal @ (LowerMacTerminal::KeyRemoved(Removed::Station(RemovedKey::Group {
                ..
            }))
            | LowerMacTerminal::GroupKeyInstalled(InstalledGroup::Station { .. })
            | LowerMacTerminal::GroupKeyReplaced { .. }
            | LowerMacTerminal::GroupKeySelected { .. }) => self.group_terminal(terminal, group),
            LowerMacTerminal::PairwiseKeyInstalled(InstalledPairwise::Station(key)) => {
                let Some(PendingPairwise {
                    pending,
                    key: request,
                }) = self.inner.pairwise.take()
                else {
                    self.inner.reject(
                        Rejected::Terminal(LowerMacTerminal::PairwiseKeyInstalled(
                            InstalledPairwise::Station(key),
                        )),
                        LifecycleViolation::Returned(BackendContractViolation::UnexpectedTerminal(
                            PendingTerminal::Control(ControlTerminal::PairwiseInstall),
                        )),
                    );
                    return;
                };
                match request.installed(key) {
                    Ok(key) => self.station.input(
                        now,
                        StaInput::PairwiseKeyInstalled { pending, key },
                        &mut self.inner.events,
                    ),
                    Err(foreign) => {
                        // The request keeps its association and numbering
                        // token; the foreign key stays with the fault.
                        self.inner.pairwise = Some(PendingPairwise {
                            pending,
                            key: foreign.pending,
                        });
                        self.inner.reject(
                            Rejected::Terminal(LowerMacTerminal::PairwiseKeyInstalled(
                                InstalledPairwise::Station(foreign.key),
                            )),
                            LifecycleViolation::Returned(
                                BackendContractViolation::ForeignPairwiseKey,
                            ),
                        );
                    }
                }
            }
            terminal @ (LowerMacTerminal::TxCompleted(TxCompleted::Beacon { .. })
            | LowerMacTerminal::Configured(Configured::AccessPoint(_))
            | LowerMacTerminal::PairwiseKeyInstalled(InstalledPairwise::AccessPoint(
                _,
            ))
            | LowerMacTerminal::KeyRemoved(Removed::AccessPoint(_))
            | LowerMacTerminal::GroupKeyInstalled(InstalledGroup::AccessPoint {
                ..
            })) => {
                self.inner.reject(
                    Rejected::Terminal(terminal),
                    LifecycleViolation::Returned(
                        BackendContractViolation::UnexpectedAccessPointTerminal,
                    ),
                );
            }
        }
    }

    /// A group key change's terminal, with the station's token the actor
    /// gave back after it applied the change to the link's replay check.
    fn group_terminal(
        &mut self,
        terminal: ActorTerminal<'id, D>,
        group: Option<GroupPending<'id, ActorEpochPath<'id, D>>>,
    ) {
        let (Some(pending), Some(token)) = (self.inner.group.take(), group) else {
            self.inner.reject(
                Rejected::Terminal(terminal),
                LifecycleViolation::Returned(BackendContractViolation::GroupTerminalWithoutRequest),
            );
            return;
        };
        let identity = match &terminal {
            LowerMacTerminal::KeyRemoved(Removed::Station(RemovedKey::Group { slot, .. })) => {
                <<Ty<'id, D> as PortTypes<'id>>::StationKeys as KeyTypes<'id>>::vacant_group_slot_identity(slot)
            }
            LowerMacTerminal::GroupKeyInstalled(InstalledGroup::Station { key, .. })
            | LowerMacTerminal::GroupKeyReplaced { key, .. }
            | LowerMacTerminal::GroupKeySelected { key, .. } => {
                <<Ty<'id, D> as PortTypes<'id>>::StationKeys as KeyTypes<'id>>::group_slot_identity(key)
            }
            _ => {
                self.inner.group = Some(pending);
                self.inner.rejected_group(token);
                self.inner.reject(
                    Rejected::Terminal(terminal),
                    LifecycleViolation::Returned(BackendContractViolation::GroupTerminalKindMismatch),
                );
                return;
            }
        };
        if pending.slot != identity {
            self.inner.group = Some(pending);
            self.inner.rejected_group(token);
            self.inner.reject(
                Rejected::Terminal(terminal),
                LifecycleViolation::Returned(BackendContractViolation::GroupSlotMismatch),
            );
            return;
        }
        let input = match (token, terminal) {
            (
                GroupPending::Remove(pending),
                LowerMacTerminal::KeyRemoved(Removed::Station(RemovedKey::Group {
                    slot,
                    permission,
                })),
            ) => StaInput::GroupKeyRemoved {
                pending,
                slot,
                permission,
            },
            (
                GroupPending::Install(installed),
                LowerMacTerminal::GroupKeyInstalled(InstalledGroup::Station { key, permission }),
            ) => {
                if let Some(key_id) = pending.key_id {
                    report(&mut self.inner.events, || ServiceEvent::GroupKeyInstalled {
                        key_id: key_id.get(),
                        replaced: false,
                    });
                }
                StaInput::GroupKeyInstalled {
                    pending: installed,
                    key,
                    permission,
                }
            }
            (
                GroupPending::Replace(replaced),
                LowerMacTerminal::GroupKeyReplaced { key, permission },
            ) => {
                if let Some(key_id) = pending.key_id {
                    report(&mut self.inner.events, || ServiceEvent::GroupKeyInstalled {
                        key_id: key_id.get(),
                        replaced: true,
                    });
                }
                StaInput::GroupKeyReplaced {
                    pending: replaced,
                    key,
                    permission,
                }
            }
            (
                GroupPending::Select(selected),
                LowerMacTerminal::GroupKeySelected { key, permission },
            ) => StaInput::GroupKeySelected {
                pending: selected,
                key,
                permission,
            },
            (token, terminal) => {
                self.inner.group = Some(pending);
                self.inner.rejected_group(token);
                self.inner.reject(
                    Rejected::Terminal(terminal),
                    LifecycleViolation::Returned(
                        BackendContractViolation::GroupTerminalKindMismatch,
                    ),
                );
                return;
            }
        };
        self.inner
            .admissions
            .remove(PendingTerminal::Control(ControlTerminal::GroupChange));
        self.station
            .input(self.inner.now, input, &mut self.inner.events);
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
    /// Keep the first rejected owner and end on `violation`.
    fn reject(&mut self, rejected: Rejected<'id, D>, violation: LifecycleViolation) {
        match self.rejected {
            Some(_) => {
                let _quarantined = core::mem::ManuallyDrop::new(rejected);
            }
            None => self.rejected = Some(rejected),
        }
        self.end = Some(LifecycleEnd::Contract(violation));
    }
    /// Keep a group token of a rejected terminal beside it.
    fn rejected_group(&mut self, token: GroupPending<'id, ActorEpochPath<'id, D>>) {
        match self.rejected {
            Some(_) => {
                let _quarantined = core::mem::ManuallyDrop::new(token);
            }
            None => self.rejected = Some(Rejected::Group(token)),
        }
    }
}
