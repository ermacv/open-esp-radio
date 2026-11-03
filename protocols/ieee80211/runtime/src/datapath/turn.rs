//! The actor's turn: what it waits on, the order of its work, the
//! lifecycle's messages and the radio's start, move and stop.

use super::*;

/// What [`Actor::collect`] found.
pub(super) struct Collected {
    /// The port's signals held facts.
    pub(super) signalled: bool,
    /// Another source has work.
    pub(super) ready: bool,
}

/// The port's fault of the actor of `D`.
type PortFault<'id, D> = <<D as DatapathPort<'id>>::Port as DataPort<'id>>::Fault;

/// A turn's failure: the broken contract that ends the run.
pub(super) type Failed<'id, D> = ActorFault<PortFault<'id, D>>;

/// The turn's sink for what the port produces besides its return values: a
/// terminal goes to the lifecycle at once, unless it ends the group change
/// the actor holds a token for; that terminal, the units of a receive cut
/// and the TBTT wait in the stage for the actor.
pub(super) struct Outbox<'a, 'c, 'id, D: DatapathPort<'id>, const FROM: usize, const RETURNS: usize>
{
    pub(super) from_actor: &'a WakingSpscSender<'c, FromActor<'id, D>, FROM>,
    pub(super) returns: &'a WakingSpscSender<'c, Returned<'id, D>, RETURNS>,
    pub(super) stage: &'a mut Stage<'id, D>,
    /// A group change's terminal is owed.
    pub(super) grouped: bool,
}

/// Whether `terminal` ends a station group key change.
fn is_group<'id, T: PhaseTypes<'id>>(terminal: &LowerMacTerminal<'id, T>) -> bool {
    use oer_protocols_ieee80211_contracts_port::{InstalledGroup, Removed, RemovedKey};
    matches!(
        terminal,
        LowerMacTerminal::GroupKeyInstalled(InstalledGroup::Station { .. })
            | LowerMacTerminal::GroupKeyReplaced { .. }
            | LowerMacTerminal::GroupKeySelected { .. }
            | LowerMacTerminal::KeyRemoved(Removed::Station(RemovedKey::Group { .. }))
    )
}

/// Send `returned` and its announcement, when both queues have room.
pub(super) fn give_back<'id, D: DatapathPort<'id>, const FROM: usize, const RETURNS: usize>(
    from_actor: &WakingSpscSender<'_, FromActor<'id, D>, FROM>,
    returns: &WakingSpscSender<'_, Returned<'id, D>, RETURNS>,
    returned: Returned<'id, D>,
) -> Result<(), Returned<'id, D>> {
    if from_actor.free_capacity() == 0 {
        return Err(returned);
    }
    returns.try_send(returned).map_err(|refused| refused.0)?;
    // The announcement's room was checked; the one sender sends.
    from_actor
        .try_send(FromActor::Returned)
        .map_err(|_| ())
        .ok();
    Ok(())
}

impl<'id, D: DatapathPort<'id>, const FROM: usize, const RETURNS: usize> PortSink<'id, D::Types>
    for Outbox<'_, '_, 'id, D, FROM, RETURNS>
{
    fn terminal(&mut self, terminal: ActorTerminal<'id, D>) {
        if self.grouped && self.stage.group_terminal.is_none() && is_group(&terminal) {
            self.stage.group_terminal = Some(terminal);
            return;
        }
        let returned = Returned::Terminal {
            terminal,
            grouped: false,
        };
        if let Err(returned) = give_back(self.from_actor, self.returns, returned) {
            // The producers' bounds did not hold: the owner stays, never
            // dropped, and the run ends on the violation.
            let _quarantined = core::mem::ManuallyDrop::new(returned);
            self.stage.overflow = Some(ActorViolation::OutboxFull);
        }
    }

    fn tbtt(&mut self, event: TbttEvent) {
        self.stage.tbtt = Some(event);
    }

    fn received(&mut self, frame: ActorRxBuffer<'id, D>, meta: ActorRxMeta<'id, D>) {
        if let Err(unit) = self.stage.cut.push_back((frame, meta)) {
            let _quarantined = core::mem::ManuallyDrop::new(unit);
            self.stage.overflow = Some(ActorViolation::CutOverflow);
        }
    }
}

/// The port carrying out the receive agreements' calls.
pub(super) struct PortBlockAcks<'r, P>(pub(super) &'r mut P);

impl<'id, P: DataPort<'id>> RxBlockAckPort<PortRxBlockAck<'id, P::Types>> for PortBlockAcks<'_, P> {
    type Caps = P::Caps;

    fn add(
        &mut self,
        slot: PhaseBlockAckSlot<'id, P::Types>,
        agreement: SupportedBlockAck<P::Caps>,
    ) -> <P::Types as PhaseTypes<'id>>::BlockAck {
        self.0.add_block_ack(slot, agreement)
    }

    fn remove(
        &mut self,
        running: <P::Types as PhaseTypes<'id>>::BlockAck,
    ) -> PhaseBlockAckSlot<'id, P::Types> {
        self.0.remove_block_ack(running)
    }

    fn recycle(&mut self, frame: <P::Types as PhaseTypes<'id>>::RxBuffer) {
        self.0.recycle(frame);
    }
}

impl<'id, D: DatapathPort<'id>, S, O> Actor<'id, D, S, O>
where
    S: RateSelection<
            ActorRxMeta<'id, D>,
            <Ty<'id, D> as PhaseTypes<'id>>::AckEvidence,
            <Ty<'id, D> as PhaseTypes<'id>>::BlockAckEvidence,
        >,
    O: ActorEvents,
{
    /// Whether the lifecycle's messages are taken now: no command waits at
    /// its barrier, and a message taken while the radio changes waits for
    /// it no longer.
    fn takes_messages(&self) -> bool {
        self.held.is_none() && !(self.stash.is_some() && self.radio.changing())
    }

    /// The earliest instant the actor must run without a signal: the port's
    /// deadline and its data arenas', the receive agreements', the ring's
    /// continuation and the stop's deadline.
    fn deadline(&mut self) -> Option<Instant> {
        let stop = match self.radio {
            Radio::Stopping { deadline } => Some(deadline),
            _ => None,
        };
        let arena = if self.data.plane.as_ref().is_some_and(DataPlane::on_air) {
            oer_protocols_ieee80211_contracts_port::arena::TxArena::deadline(&self.port.arena())
        } else {
            None
        };
        [
            self.port.deadline(),
            arena,
            self.receive.deadline(),
            self.rx_later,
            stop,
        ]
        .into_iter()
        .flatten()
        .min()
    }

    /// Whether the port's deadline asks for its service now: work the turn
    /// left, which the next turn takes after one repost. A deadline that is
    /// still due after the service is the port's "call again": a station
    /// MPDU queued in this turn publishes at the next service, and a
    /// published one ends there. After [`DUE_REPOSTS`] turns in a row the
    /// actor sleeps instead: the alarm never arms a deadline already passed,
    /// so only the port's next signal or a later deadline wakes it, and a
    /// port that never moves its deadline meets its own named bounds (a held
    /// command's, a publication's), which end the run as faults.
    pub(super) fn due_again(&mut self) -> bool {
        let due = self.port.deadline().is_some_and(|at| at <= self.now);
        if !due {
            self.due_streak = 0;
            return false;
        }
        self.due_streak = self.due_streak.saturating_add(1);
        self.due_streak <= DUE_REPOSTS
    }

    /// Register on every source the next turn serves and say whether one
    /// has work now: the port's facts (which wait here for the turn), and
    /// the other sources. A fact the port signalled during a turn already
    /// woke the actor; another source made ready by the turn itself did not.
    pub(super) fn collect<
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
        mut alarm: Pin<&mut Option<F>>,
        arm: &impl Fn(Option<Instant>) -> Option<F>,
    ) -> Collected {
        let facts = self.port.poll_facts(cx);
        self.facts = Facts {
            tx: match (self.facts.tx, facts.tx) {
                (Some(held), Some(new)) => Some(held.union(new)),
                (held, new) => held.or(new),
            },
            tbtt: self.facts.tbtt || facts.tbtt,
            rx: self.facts.rx || facts.rx,
        };
        let signalled = !self.facts.is_empty();
        let mut ready = false;
        if self.takes_messages()
            && (self.stash.is_some()
                || core::pin::pin!(lifecycle.to_actor.ready())
                    .poll(cx)
                    .is_ready())
        {
            ready = true;
        }
        if self.accepts()
            && core::pin::pin!(network.transmit.ready())
                .poll(cx)
                .is_ready()
        {
            ready = true;
        }
        if !self.data.freed.is_empty()
            && core::pin::pin!(network.returned.ready())
                .poll(cx)
                .is_ready()
        {
            ready = true;
        }
        if self.receive.has_ready()
            && (!network.network.takes_frames() || self.port.poll_lend_room(cx).is_ready())
        {
            ready = true;
        }
        // One alarm, re-armed only when its deadline changes; a deadline the
        // last turn left behind is not armed again.
        let deadline = self.deadline().filter(|at| *at > self.now);
        if deadline != self.armed {
            self.armed = deadline;
            alarm.set(arm(deadline));
        }
        if let Some(waiting) = alarm.as_mut().as_pin_mut()
            && waiting.poll(cx).is_ready()
        {
            alarm.set(None);
            self.armed = None;
            ready = true;
        }
        Collected { signalled, ready }
    }

    /// One turn at `self.now`.
    pub(super) fn turn<
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
    ) -> Result<(), Failed<'id, D>> {
        let facts = core::mem::take(&mut self.facts);
        D::span(ActorSpan::Transmit, false);
        let transmitted = self.transmit(facts, lifecycle, network);
        D::span(ActorSpan::Transmit, true);
        transmitted?;
        D::span(ActorSpan::Receive, false);
        let received = self.rx(facts.rx, lifecycle, network);
        D::span(ActorSpan::Receive, true);
        received?;
        D::span(ActorSpan::Intake, false);
        let taken = self.intake(lifecycle, network);
        D::span(ActorSpan::Intake, true);
        taken?;
        self.return_freed(network);
        D::span(ActorSpan::Messages, false);
        let served = self.messages(cx, lifecycle, network);
        D::span(ActorSpan::Messages, true);
        served?;
        self.drive_radio(cx)?;
        // A message may have released the link's data: the plane takes it
        // now, so the network's next MSDU wakes this actor.
        self.connect_plane()?;
        self.flush(lifecycle);
        Ok(())
    }

    /// Serve the port's transmit facts, or its due deadline: terminals to
    /// the lifecycle, a data arena's end settled, then the next arena
    /// published.
    fn transmit<
        const TO: usize,
        const FROM: usize,
        const RETURNS: usize,
        NS: NetworkStop,
        const TX_DEPTH: usize,
        const RX_DEPTH: usize,
        N: Notifies,
    >(
        &mut self,
        facts: Facts,
        lifecycle: &mut LifecycleEnds<'_, 'id, D, TO, FROM, RETURNS>,
        network: &mut NetworkEnds<'_, D::TxMemory, D::LentRx, NS, TX_DEPTH, RX_DEPTH, N>,
    ) -> Result<(), Failed<'id, D>> {
        let due = self.port.deadline().is_some_and(|at| at <= self.now);
        if facts.tx.is_some() || facts.tbtt || due {
            let mut outbox = Outbox {
                from_actor: &lifecycle.from_actor,
                returns: &lifecycle.returns,
                stage: &mut self.stage,
                grouped: self.group.is_some(),
            };
            let done = self
                .port
                .service_tx(
                    facts.tx.unwrap_or(TxFacts::DUE),
                    facts.tbtt,
                    self.now,
                    &mut outbox,
                )
                .map_err(ActorFault::Port)?;
            self.staged(lifecycle, network)?;
            if let Some(done) = done {
                self.settle(done)?;
            }
        }
        self.publish()
    }

    /// Hand on what the port staged in the last call: the units of a
    /// receive cut through the receive path, then the group change's
    /// terminal, applied to the link's replay epoch first, with its token.
    pub(super) fn staged<
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
    ) -> Result<(), Failed<'id, D>> {
        if let Some(violation) = self.stage.overflow.take() {
            return Err(ActorFault::Contract(violation));
        }
        if let Some(event) = self.stage.tbtt.take() {
            self.owed.tbtt = Some(event);
        }
        while let Some((frame, meta)) = self.stage.cut.pop_front() {
            self.received(frame, meta, lifecycle, network)?;
        }
        if let Some(terminal) = self.stage.group_terminal.take() {
            let mut group = self.group.take();
            if let (Some(link), Some(pending)) = (self.link.as_mut(), group.as_mut()) {
                link.group_lane(pending.lane());
            }
            let grouped = self.give_group(lifecycle, group)?;
            self.give(lifecycle, Returned::Terminal { terminal, grouped })?;
        }
        Ok(())
    }

    /// Put the station's group token in the group queue, ahead of the
    /// return that announces it; whether there was one.
    fn give_group<const TO: usize, const FROM: usize, const RETURNS: usize>(
        &mut self,
        lifecycle: &LifecycleEnds<'_, 'id, D, TO, FROM, RETURNS>,
        group: Option<GroupPending<'id, ActorEpochPath<'id, D>>>,
    ) -> Result<bool, Failed<'id, D>> {
        let Some(group) = group else {
            return Ok(false);
        };
        lifecycle.groups.try_send(group).map_err(|refused| {
            self.retained.group = Some(refused.0);
            ActorFault::Contract(ActorViolation::OutboxFull)
        })?;
        Ok(true)
    }

    /// Give `returned` back to the lifecycle; without room the actor keeps it
    /// and the run ends.
    pub(super) fn give<const TO: usize, const FROM: usize, const RETURNS: usize>(
        &mut self,
        lifecycle: &LifecycleEnds<'_, 'id, D, TO, FROM, RETURNS>,
        returned: Returned<'id, D>,
    ) -> Result<(), Failed<'id, D>> {
        give_back(&lifecycle.from_actor, &lifecycle.returns, returned).map_err(|returned| {
            self.retained.returned = Some(returned);
            ActorFault::Contract(ActorViolation::OutboxFull)
        })
    }

    /// Take the lifecycle's messages in order: at most
    /// [`MESSAGE_BUDGET`], of which at most one command or radio change. A
    /// command held at its barrier runs first; while it waits, or while the
    /// radio starts or moves, the next message waits.
    fn messages<
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
    ) -> Result<(), Failed<'id, D>> {
        let mut heavy = false;
        if let Some(HeldCommand { command }) = self.held.take() {
            heavy = true;
            self.command(command, lifecycle, network)?;
            if self.held.is_some() {
                return Ok(());
            }
        }
        for _ in 0..MESSAGE_BUDGET {
            if self.radio.changing() && self.stash.is_some() {
                return Ok(());
            }
            let Some(message) = self
                .stash
                .take()
                .or_else(|| lifecycle.to_actor.try_receive())
            else {
                return Ok(());
            };
            let weighs = !matches!(
                message,
                ToActor::Recycle(_)
                    | ToActor::StationTx(_)
                    | ToActor::Learn { .. }
                    | ToActor::BlockAckReplied(_)
                    | ToActor::Hold
                    | ToActor::Release
                    | ToActor::Agreement { .. }
            );
            let waits = self.radio.changing()
                && matches!(
                    message,
                    ToActor::StationTx(_) | ToActor::Port(_) | ToActor::Parcel
                );
            if (weighs && heavy) || waits {
                self.stash = Some(message);
                self.again |= !waits;
                return Ok(());
            }
            heavy |= weighs;
            self.message(cx, message, lifecycle, network)?;
            if self.held.is_some() || self.end.is_some() {
                return Ok(());
            }
        }
        self.again |= !lifecycle.to_actor.is_empty();
        Ok(())
    }

    /// Take one message of the lifecycle.
    fn message<
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
        message: ToActor<'id, D>,
        lifecycle: &mut LifecycleEnds<'_, 'id, D, TO, FROM, RETURNS>,
        network: &mut NetworkEnds<'_, D::TxMemory, D::LentRx, NS, TX_DEPTH, RX_DEPTH, N>,
    ) -> Result<(), Failed<'id, D>> {
        match message {
            ToActor::Start(channel) => match self.radio {
                Radio::Stopped | Radio::Halted => {
                    self.radio = Radio::Starting(channel);
                    self.drive_radio(cx)?;
                }
                _ => self.owed.started = Some(Err(StartRefused::Running)),
            },
            ToActor::Retune(channel) => {
                let stopped = matches!(self.radio, Radio::Stopped | Radio::Halted);
                self.radio = Radio::Retuning { channel, stopped };
                self.drive_radio(cx)?;
            }
            ToActor::Connect => {
                let Some(link) = lifecycle.link.try_receive() else {
                    return Err(ActorFault::Contract(ActorViolation::LinkMailbox));
                };
                if self.link.is_some() {
                    self.retained.link = Some(link);
                    return Err(ActorFault::Contract(ActorViolation::LinkMailbox));
                }
                self.link = Some(link);
            }
            ToActor::Disconnect => self.disconnect(lifecycle)?,
            ToActor::Hold | ToActor::Release => {
                if let Some(link) = self.link.as_mut() {
                    link.hold(matches!(message, ToActor::Hold));
                }
            }
            ToActor::Agreement { tid, window } => {
                if !self
                    .link
                    .as_mut()
                    .is_some_and(|link| link.agreement(tid, window))
                    && let Some(plane) = self.data.plane.as_mut()
                {
                    plane.agreement(tid, window);
                }
            }
            ToActor::EapolFaulted => {
                if let Some(link) = self.link.as_mut() {
                    link.eapol_faulted();
                }
            }
            ToActor::Parcel => match lifecycle.parcels.try_receive() {
                Some(Parcel::Associated(peer)) => {
                    self.rates
                        .associated(&peer.associated.rate_peer(), &peer.response);
                }
                Some(Parcel::Group { command, pending }) => {
                    if self.group.is_some() {
                        // One group change at a time: the second comes back.
                        let grouped = self.give_group(lifecycle, Some(pending))?;
                        self.give(
                            lifecycle,
                            Returned::Command {
                                command,
                                grouped,
                                reason: CommandRefusal::Busy,
                            },
                        )?;
                    } else {
                        self.group = Some(pending);
                        self.command(command, lifecycle, network)?;
                    }
                }
                None => return Err(ActorFault::Contract(ActorViolation::ParcelMissing)),
            },
            ToActor::Disassociated => self.rates.disconnected(),
            ToActor::StationTx(transmit) => self.station_tx(transmit, lifecycle)?,
            ToActor::Learn {
                rates,
                current,
                outcome,
            } => self.rates.learn(self.now, rates, current, &outcome),
            ToActor::RxAgreement(request) => {
                if self
                    .receive
                    .offer(
                        self.now,
                        request.epoch(),
                        request.bssid(),
                        request.action(),
                        request.supported(),
                        &mut PortBlockAcks(&mut self.port),
                        &mut self.events,
                    )
                    .is_err()
                {
                    // No reply place or bank is left for the request: the
                    // access point hears nothing and asks again.
                    report(&mut self.events, || ActorEvent::RepliesFull);
                }
            }
            ToActor::BlockAckReplied(place) => {
                if self.reply_out == Some(place) {
                    self.receive.sent(place);
                    self.reply_out = None;
                }
            }
            ToActor::Port(command) => self.command(command, lifecycle, network)?,
            ToActor::OpenGate(closed) => {
                self.owed.gate = Some(self.port.open_gate(closed));
            }
            ToActor::Recycle(frame) => {
                self.port.recycle(frame);
                self.frames_out = self.frames_out.saturating_sub(1);
                // The room it frees may let the ring give units it held
                // back: the next turn pulls again.
                self.facts.rx = true;
                self.again = true;
            }
            ToActor::Stop { deadline } => self.stop(deadline, network)?,
            ToActor::Retire => {
                if !matches!(self.radio, Radio::Halted | Radio::Stopped) {
                    // Only a stopped radio retires: the lifecycle stops it
                    // first.
                    return Err(ActorFault::Contract(ActorViolation::RetireRunning));
                }
                self.end = Some(ActorEnd::Retired);
            }
        }
        Ok(())
    }

    /// Run one port command, or keep it held at its barrier; a refusal goes
    /// back with its owners and the group token.
    fn command<
        const TO: usize,
        const FROM: usize,
        const RETURNS: usize,
        NS: NetworkStop,
        const TX_DEPTH: usize,
        const RX_DEPTH: usize,
        N: Notifies,
    >(
        &mut self,
        command: ActorCommand<'id, D>,
        lifecycle: &mut LifecycleEnds<'_, 'id, D, TO, FROM, RETURNS>,
        network: &mut NetworkEnds<'_, D::TxMemory, D::LentRx, NS, TX_DEPTH, RX_DEPTH, N>,
    ) -> Result<(), Failed<'id, D>> {
        // Frames the arenas wrote under the pairwise key carry its packet
        // numbers: a new key or the key's removal withdraws them first.
        if matches!(
            command,
            PortCommand::InstallPairwise(_) | PortCommand::RemovePairwise { .. }
        ) {
            self.withdraw_unpublished()?;
        }
        let mut outbox = Outbox {
            from_actor: &lifecycle.from_actor,
            returns: &lifecycle.returns,
            stage: &mut self.stage,
            grouped: self.group.is_some(),
        };
        let progress = self
            .port
            .command(command, self.now, &mut outbox)
            .map_err(ActorFault::Port)?;
        match progress {
            CommandProgress::Done => {}
            CommandProgress::Held(command) => self.held = Some(HeldCommand { command }),
            CommandProgress::Refused(command, reason) => {
                // A refused group change ends without a terminal.
                let group = if self.stage.group_terminal.is_none() {
                    self.group.take()
                } else {
                    None
                };
                let grouped = self.give_group(lifecycle, group)?;
                self.give(
                    lifecycle,
                    Returned::Command {
                        command,
                        grouped,
                        reason,
                    },
                )?;
            }
        }
        self.staged(lifecycle, network)
    }

    /// Give the link back: the arenas' unpublished frames and the waiting
    /// MSDU go back to the network, the receive agreements end, and the
    /// link goes to the return mailbox.
    fn disconnect<const TO: usize, const FROM: usize, const RETURNS: usize>(
        &mut self,
        lifecycle: &mut LifecycleEnds<'_, 'id, D, TO, FROM, RETURNS>,
    ) -> Result<(), Failed<'id, D>> {
        self.stop_data()?;
        self.receive
            .clear(&mut PortBlockAcks(&mut self.port), &mut self.events);
        self.reply_out = None;
        let Some(link) = self.link.take() else {
            return Err(ActorFault::Contract(ActorViolation::LinkMailbox));
        };
        if let Err(refused) = lifecycle.link_return.try_send(link.into_return()) {
            self.retained.link_return = Some(refused.0);
            return Err(ActorFault::Contract(ActorViolation::LinkReturn));
        }
        self.owed.link_returned = true;
        Ok(())
    }

    /// Begin the stop by `deadline`: the data's unpublished frames go back
    /// to the network, the receive agreements end and their units return,
    /// and the radio stops.
    fn stop<NS: NetworkStop, const TX_DEPTH: usize, const RX_DEPTH: usize, N: Notifies>(
        &mut self,
        deadline: Instant,
        network: &mut NetworkEnds<'_, D::TxMemory, D::LentRx, NS, TX_DEPTH, RX_DEPTH, N>,
    ) -> Result<(), Failed<'id, D>> {
        self.stop_data()?;
        self.receive
            .clear(&mut PortBlockAcks(&mut self.port), &mut self.events);
        self.reply_out = None;
        self.return_freed(network);
        self.radio = match self.radio {
            Radio::Stopped | Radio::Halted => {
                self.owed.stopped = Some(StopOutcome::Confirmed);
                Radio::Halted
            }
            _ => Radio::Stopping { deadline },
        };
        Ok(())
    }

    /// What a stop still holds.
    fn observed(&self, port: Option<StopUnconfirmed>, deadline: bool) -> StopObserved {
        StopObserved {
            port,
            deadline,
            arena_on_air: self.data.plane.as_ref().is_some_and(DataPlane::on_air),
            frames_out: self.frames_out,
        }
    }

    /// Move the radio on: its start, its move and its stop run within the
    /// port's bounds, each answered once.
    fn drive_radio(&mut self, cx: &mut Context<'_>) -> Result<(), Failed<'id, D>> {
        let now = self.now;
        match self.radio {
            Radio::Starting(channel) => match self.port.poll_start(cx, channel, now) {
                Poll::Ready(Ok(())) => {
                    self.radio = Radio::Running;
                    self.running()?;
                    self.owed.started = Some(Ok(()));
                }
                Poll::Ready(Err(refused)) => {
                    self.radio = Radio::Stopped;
                    self.owed.started = Some(Err(refused));
                }
                Poll::Pending => {}
            },
            Radio::Retuning {
                channel,
                stopped: false,
            } => match self.port.poll_stop(cx, now) {
                Poll::Ready(Ok(())) => {
                    self.radio = Radio::Retuning {
                        channel,
                        stopped: true,
                    };
                    return self.drive_radio(cx);
                }
                Poll::Ready(Err(unconfirmed)) => {
                    self.radio = Radio::Halted;
                    self.owed.retuned = Some(Err(RetuneFailed::Stop(unconfirmed)));
                }
                Poll::Pending => {}
            },
            Radio::Retuning {
                channel,
                stopped: true,
            } => match self.port.poll_start(cx, channel, now) {
                Poll::Ready(Ok(())) => {
                    self.radio = Radio::Running;
                    self.running()?;
                    self.owed.retuned = Some(Ok(()));
                }
                Poll::Ready(Err(refused)) => {
                    self.radio = Radio::Stopped;
                    self.owed.retuned = Some(Err(RetuneFailed::Start(refused)));
                }
                Poll::Pending => {}
            },
            Radio::Stopping { deadline } => {
                let stopped = self.port.poll_stop(cx, now);
                let late = now >= deadline;
                let outcome = match stopped {
                    Poll::Ready(Err(unconfirmed)) => Some(StopOutcome::Unconfirmed(
                        self.observed(Some(unconfirmed), late),
                    )),
                    // The radio stopped: the run's owners are back once the
                    // data arena's end settled and the lifecycle sent its
                    // frames back.
                    Poll::Ready(Ok(())) => {
                        let observed = self.observed(None, late);
                        if !observed.arena_on_air && observed.frames_out == 0 {
                            Some(StopOutcome::Confirmed)
                        } else if late {
                            Some(StopOutcome::Unconfirmed(observed))
                        } else {
                            None
                        }
                    }
                    Poll::Pending if late => {
                        Some(StopOutcome::Unconfirmed(self.observed(None, true)))
                    }
                    Poll::Pending => None,
                };
                if let Some(outcome) = outcome {
                    self.radio = Radio::Halted;
                    self.owed.stopped = Some(outcome);
                }
            }
            Radio::Stopped | Radio::Running | Radio::Halted => {}
        }
        Ok(())
    }

    /// The radio runs: the data plane takes the port's arenas once.
    fn running(&mut self) -> Result<(), Failed<'id, D>> {
        if self.data.plane.is_some() {
            return Ok(());
        }
        let rights =
            oer_protocols_ieee80211_contracts_port::arena::TxArena::arenas(&mut self.port.arena())
                .ok_or(ActorFault::Contract(ActorViolation::ArenaClaim))?;
        let plane =
            DataPlane::new(rights).map_err(|_| ActorFault::Contract(ActorViolation::ArenaClaim))?;
        self.data.plane = Some(plane);
        self.aggregate_rate();
        Ok(())
    }

    /// Tell the lifecycle what it is owed, while its queue has room beyond
    /// the places the bounded producers keep: the answers first, then the
    /// coalesced facts.
    fn flush<const TO: usize, const FROM: usize, const RETURNS: usize>(
        &mut self,
        lifecycle: &LifecycleEnds<'_, 'id, D, TO, FROM, RETURNS>,
    ) {
        let from = &lifecycle.from_actor;
        // Answers have their own places in the queue's depth.
        if let Some(started) = self.owed.started.take()
            && let Err(refused) = from.try_send(FromActor::Started(started))
        {
            self.owed.started = match refused.0 {
                FromActor::Started(started) => Some(started),
                _ => None,
            };
        }
        if let Some(retuned) = self.owed.retuned.take()
            && let Err(refused) = from.try_send(FromActor::Retuned(retuned))
        {
            self.owed.retuned = match refused.0 {
                FromActor::Retuned(retuned) => Some(retuned),
                _ => None,
            };
        }
        if self.owed.link_returned && from.try_send(FromActor::LinkReturned).is_ok() {
            self.owed.link_returned = false;
        }
        if let Some(gate) = self.owed.gate.take()
            && let Err(refused) = from.try_send(FromActor::GateOpened(gate))
        {
            self.owed.gate = match refused.0 {
                FromActor::GateOpened(gate) => Some(gate),
                _ => None,
            };
        }
        if let Some(stopped) = self.owed.stopped.take()
            && let Err(refused) = from.try_send(FromActor::Stopped(stopped))
        {
            self.owed.stopped = match refused.0 {
                FromActor::Stopped(stopped) => Some(stopped),
                _ => None,
            };
        }
        if self.reply_out.is_none()
            && let Some(reply) = self.receive.next_reply()
            && from.try_send(FromActor::BlockAckReply(reply)).is_ok()
        {
            self.reply_out = Some(reply.place);
        }
        // Facts only take the places the bounded producers do not keep.
        let reserve = LIFECYCLE_FRAMES.saturating_add(RETURNS);
        let room =
            |from: &WakingSpscSender<'_, FromActor<'id, D>, FROM>| from.free_capacity() > reserve;
        if let Some(event) = self.owed.tbtt
            && room(from)
            && from.try_send(FromActor::Tbtt(event)).is_ok()
        {
            self.owed.tbtt = None;
        }
        for tid in WmmUserPriority::ALL {
            if let Some(head) = *tid.select(&self.owed.bar)
                && room(from)
                && from.try_send(FromActor::BarOwed { tid, head }).is_ok()
            {
                *tid.select_mut(&mut self.owed.bar) = None;
            }
            let (told, next) = tid.select_mut(&mut self.owed.traffic);
            if let Some(sequence) = *next
                && room(from)
                && from
                    .try_send(FromActor::TxTraffic {
                        tid,
                        next: sequence,
                    })
                    .is_ok()
            {
                *next = None;
                *told = Some(self.now);
            }
        }
        while let Some(&(reason, count)) = self.owed.refused.front() {
            if !room(from)
                || from
                    .try_send(FromActor::MsduRefused { reason, count })
                    .is_err()
            {
                break;
            }
            let _told = self.owed.refused.pop_front();
        }
    }

    /// End the run on `fault`: the lifecycle hears it when its queue has
    /// room; every owner stays where it is.
    pub(super) fn fault<const TO: usize, const FROM: usize, const RETURNS: usize>(
        &mut self,
        lifecycle: &LifecycleEnds<'_, 'id, D, TO, FROM, RETURNS>,
        fault: Failed<'id, D>,
    ) {
        let _told = lifecycle.from_actor.try_send(FromActor::Faulted(fault));
        self.end = Some(ActorEnd::Faulted(fault));
    }
}
