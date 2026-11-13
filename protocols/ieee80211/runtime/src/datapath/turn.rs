//! The actor's turn: what it waits on, the order of its work, the
//! lifecycle's messages and the radio's start, move and stop.

use super::*;
use oer_protocols_ieee80211_contracts_port::{Input, PortCompleted, RecycleRefused};
use oer_protocols_ieee80211_core_upper_mac::block_ack::RxBlockAckRefused;

/// What [`Actor::collect`] found.
pub(super) struct Collected {
    /// The port's signals held facts.
    pub(super) signalled: bool,
    /// Another source has work.
    pub(super) ready: bool,
    /// The port's signals gave facts in this call.
    pub(super) fresh: bool,
}

/// Where a turn left the lifecycle's messages.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Messages {
    /// Every queued message was taken.
    Taken,
    /// The next one waits for a held command or the changing radio.
    Waiting,
    /// The budget left messages queued; the next turn takes them.
    Left,
}

/// The port's fault of the actor of `D`.
type PortFault<'id, D> = <<D as DatapathPort<'id>>::Port as DataPort<'id>>::Fault;

/// A turn's failure: the broken contract that ends the run.
pub(super) type Failed<'id, D> = ActorFault<PortFault<'id, D>>;

/// The turn's sink for what the port produces besides its return values: a
/// terminal goes to the lifecycle at once, unless it ends the group change
/// the actor holds a token for; that terminal, the units of a receive cut
/// and the TBTT wait in the stage for the actor. One it has no room for it
/// refuses: the port keeps it, and the actor's next wait registers for the
/// lifecycle's room ([`Stage::refused`]) before it sleeps.
pub(super) struct Outbox<'a, 'c, 'id, D: DatapathPort<'id>, const FROM: usize, const RETURNS: usize>
{
    pub(super) from_actor: &'a mut ClosingSpscSender<'c, FromActor<'id, D>, FROM>,
    pub(super) returns: &'a mut ClosingSpscSender<'c, Returned<'id, D>, RETURNS>,
    pub(super) stage: &'a mut Stage<'id, D>,
    /// A group change's terminal is owed.
    pub(super) grouped: bool,
}

/// Whether `terminal` ends a station group key change.
fn is_group<'id, T: PortTypes<'id>>(terminal: &LowerMacTerminal<'id, T>) -> bool {
    use oer_protocols_ieee80211_contracts_port::{InstalledGroup, Removed, RemovedKey};
    matches!(
        terminal,
        LowerMacTerminal::GroupKeyInstalled(InstalledGroup::Station { .. })
            | LowerMacTerminal::GroupKeyReplaced { .. }
            | LowerMacTerminal::GroupKeySelected { .. }
            | LowerMacTerminal::KeyRemoved(Removed::Station(RemovedKey::Group { .. }))
    )
}

/// The violation a refused send to the lifecycle names: a closed reception
/// is its own, never a full queue.
pub(super) const fn refusal<T>(refused: &ClosingSpscSendError<T>) -> ActorViolation {
    match refused {
        ClosingSpscSendError::Full(_) => ActorViolation::OutboxFull,
        ClosingSpscSendError::Closed(_) => ActorViolation::LifecycleClosed,
    }
}

/// Send `returned` and its announcement, when both queues have room and
/// the lifecycle's reception is open; otherwise `returned` comes back with
/// the refusal's violation.
pub(super) fn give_back<'id, D: DatapathPort<'id>, const FROM: usize, const RETURNS: usize>(
    from_actor: &ClosingSpscSender<'_, FromActor<'id, D>, FROM>,
    returns: &ClosingSpscSender<'_, Returned<'id, D>, RETURNS>,
    returned: Returned<'id, D>,
) -> Result<(), (Returned<'id, D>, ActorViolation)> {
    if from_actor.is_closed() {
        return Err((returned, ActorViolation::LifecycleClosed));
    }
    if from_actor.free_capacity() == 0 {
        return Err((returned, ActorViolation::OutboxFull));
    }
    returns.try_send(returned).map_err(|refused| {
        let violation = refusal(&refused);
        (refused.into_inner(), violation)
    })?;
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
    fn terminal(&mut self, terminal: ActorTerminal<'id, D>) -> Result<(), ActorTerminal<'id, D>> {
        if self.grouped && self.stage.group_terminal.is_none() && is_group(&terminal) {
            D::terminal(&terminal);
            self.stage.group_terminal = Some(terminal);
            return Ok(());
        }
        // A closed reception has no lifecycle to hand it to: the port keeps
        // it, whole, and nothing waits for room that never comes.
        if self.from_actor.is_closed() || self.returns.is_closed() {
            self.stage.closed = true;
            return Err(terminal);
        }
        // Both cells first: a terminal without its announcement's room is
        // refused whole, and the port keeps it.
        let (Some(announce), Some(cell)) =
            (self.from_actor.try_reserve(), self.returns.try_reserve())
        else {
            self.stage.refused = true;
            return Err(terminal);
        };
        D::terminal(&terminal);
        // Closed after the check above: the same end.
        if let Err(ClosingSpscClosed(terminal)) =
            cell.send_with(terminal, |terminal| Returned::Terminal {
                terminal,
                grouped: false,
            })
        {
            self.stage.closed = true;
            return Err(terminal);
        }
        // The announcement carries no owner: a closed reception has no
        // reader for it, and the owner it announces stays in its queue.
        let _announced = announce.send(FromActor::Returned);
        Ok(())
    }

    fn tbtt(&mut self, event: TbttEvent) {
        self.stage.tbtt = Some(event);
    }

    fn received(
        &mut self,
        frame: ActorRxBuffer<'id, D>,
        meta: ActorRxMeta<'id, D>,
    ) -> Result<(), (ActorRxBuffer<'id, D>, ActorRxMeta<'id, D>)> {
        // A full stage is drained by this turn: the port keeps the unit and
        // offers it again at the next turn's call.
        self.stage.cut.push_back((frame, meta)).inspect_err(|_| {
            self.stage.refused = true;
        })
    }
}

/// The port carrying out the receive agreements' calls.
pub(super) struct PortBlockAcks<'r, P>(pub(super) &'r mut P);

impl<'id, P: DataPort<'id>> RxBlockAckPort<PortRxBlockAck<'id, P::Types>> for PortBlockAcks<'_, P> {
    type Caps = P::Caps;

    fn add(
        &mut self,
        slot: PortBlockAckSlot<'id, P::Types>,
        agreement: SupportedBlockAck<P::Caps>,
    ) -> <P::Types as PortTypes<'id>>::BlockAck {
        self.0.add_block_ack(slot, agreement)
    }

    fn remove(
        &mut self,
        running: <P::Types as PortTypes<'id>>::BlockAck,
    ) -> PortBlockAckSlot<'id, P::Types> {
        self.0.remove_block_ack(running)
    }

    fn recycle(
        &mut self,
        frame: <P::Types as PortTypes<'id>>::RxBuffer,
    ) -> Result<(), RecycleRefused<<P::Types as PortTypes<'id>>::RxBuffer>> {
        self.0.recycle(frame)
    }
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
    /// Whether the lifecycle's messages are taken now: a command waiting at
    /// its barrier takes one more, which a stop withdraws it for, and a
    /// message taken while the radio changes waits for it no longer.
    fn takes_messages(&self) -> bool {
        if self.held.is_some() {
            return self.stash.is_none();
        }
        !(self.stash.is_some() && self.radio.changing())
    }

    /// The earliest instant after the turn's at which the actor must run
    /// without a signal: the port's deadline and its data arenas', the
    /// receive agreements', the ring's continuation and the stop's deadline.
    /// A deadline the turn left passed is not one: the turn served it, or
    /// it takes its bounded reposts ([`Self::due_again`]); it never hides
    /// a later one.
    fn deadline(&mut self) -> Option<Instant> {
        let stop = match self.radio {
            Radio::Stopping { deadline } | Radio::Draining { deadline } => Some(deadline),
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
            self.held_back
                .map(|since| since.saturating_add(NETWORK_LATE)),
            stop,
        ]
        .into_iter()
        .flatten()
        .filter(|at| *at > self.now)
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

    /// Whether the facts the port's signals gave after the turn take a
    /// repost: taking them took the wake their signal owed (a unit or a
    /// transmit event that came during the turn), so the next turn serves
    /// them after one repost. A transmit event or a TBTT among them always
    /// takes it: the port gives each once, the actor holds it, and the next
    /// turn serves it as the port's work (its relay, its settlement), so no
    /// streak may leave it to an unrelated wake. A port that gives standing
    /// work rather than new facts (a ring waiting for its continuation, a
    /// ring the caller has no room for) gives it again after a turn that
    /// served none of the port's work: after [`DUE_REPOSTS`] such turns in a
    /// row the actor sleeps, and that work's own source (its alarm, the
    /// network's return, the lifecycle's frame) wakes it.
    pub(super) fn facts_again(&mut self, fresh: bool) -> bool {
        let progressed = core::mem::take(&mut self.progressed);
        // What the next turn's transmit service counts as progress.
        let events = self.facts.tx.is_some_and(|tx| tx.bits() != 0) || self.facts.tbtt;
        if !fresh || progressed || events {
            self.fact_streak = 0;
        }
        if !fresh {
            return false;
        }
        self.fact_streak = self.fact_streak.saturating_add(1);
        self.fact_streak <= DUE_REPOSTS
    }

    /// Register on every source the next turn serves and say whether one
    /// has work now: the port's facts (which wait here for the turn), and
    /// the other sources. Taking a fact the port signalled during a turn
    /// takes the wake its signal owed, so the facts this call gave say so
    /// ([`Collected::fresh`], [`Self::facts_again`]); another source made
    /// ready by the turn itself did not wake the actor either.
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
        let fresh = self.collect_facts(cx);
        self.collect_sources(cx, fresh, lifecycle, network, alarm.as_mut(), arm)
    }

    /// Take the port's facts into the held ones; whether it gave any.
    pub(super) fn collect_facts(&mut self, cx: &mut Context<'_>) -> bool {
        let facts = self.port.poll_facts(cx);
        let fresh = !facts.is_empty();
        self.facts = Facts {
            tx: match (self.facts.tx, facts.tx) {
                (Some(held), Some(new)) => Some(held.union(new)),
                (held, new) => held.or(new),
            },
            tbtt: self.facts.tbtt || facts.tbtt,
            rx: self.facts.rx || facts.rx,
        };
        fresh
    }

    /// The [`Self::collect`] of every source but the port's facts, `fresh`
    /// when the last call of [`Self::collect_facts`] took some.
    pub(super) fn collect_sources<
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
        fresh: bool,
        lifecycle: &mut LifecycleEnds<'_, 'id, D, TO, FROM, RETURNS>,
        network: &mut NetworkEnds<'_, D::TxMemory, D::LentRx, NS, TX_DEPTH, RX_DEPTH, N>,
        mut alarm: Pin<&mut Option<F>>,
        arm: &impl Fn(Option<Instant>) -> Option<F>,
    ) -> Collected {
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
        if self.accepts() {
            let want = self.intake_want(TX_DEPTH);
            if want != 0
                && core::pin::pin!(network.transmit.ready_at_least(want))
                    .poll(cx)
                    .is_ready()
            {
                ready = true;
            }
        }
        let owes_queued = self.data.returns.owes_queued(network);
        if (!self.data.returns.freed.is_empty() || owes_queued)
            && core::pin::pin!(network.returned.ready())
                .poll(cx)
                .is_ready()
        {
            ready = true;
        }
        if self.poll_network_stop(cx, network) {
            ready = true;
        }
        // What the outbox refused waits in the port, which signals nothing
        // for it: the lifecycle's room wakes the actor, each queue's wait
        // armed before its room is checked again.
        // A closed reception ends the wait at once: the turn ends the run.
        if self.stage.refused {
            let announce = lifecycle.from_actor.poll_room(cx);
            let cell = lifecycle.returns.poll_room(cx);
            if matches!(announce, Poll::Ready(Err(_))) || matches!(cell, Poll::Ready(Err(_))) {
                self.stage.closed = true;
                ready = true;
            } else if announce.is_ready() && cell.is_ready() {
                ready = true;
            }
        }
        if self.receive.has_ready()
            && (!network.network.takes_frames()
                || self.network_late()
                || self.port.poll_lend_room(cx).is_ready())
        {
            ready = true;
        }
        // One alarm, re-armed only when its deadline changes; a deadline the
        // last turn left behind is not armed again.
        let deadline = self.deadline();
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
        Collected {
            signalled,
            ready,
            fresh,
        }
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
        // The lifecycle's messages go before the received units: a link
        // handed over is in force for every unit this turn routes.
        D::span(ActorSpan::Messages, false);
        let served = self.messages(cx, lifecycle, network);
        D::span(ActorSpan::Messages, true);
        if served? == Messages::Left {
            // The budget left messages queued, a link among them: the
            // units wait for the turn that took them, which follows at once.
            self.facts.rx |= facts.rx;
            self.again = true;
        } else {
            D::span(ActorSpan::Receive, false);
            let received = self.rx(facts.rx, lifecycle, network);
            D::span(ActorSpan::Receive, true);
            received?;
        }
        network.receive.flush();
        D::span(ActorSpan::Intake, false);
        let taken = self.intake(lifecycle, network);
        D::span(ActorSpan::Intake, true);
        taken?;
        self.return_freed(network);
        self.return_network_memory(network);
        // Before the radio's deadline judges it: a stop that asked this turn
        // reads the answer the network already has.
        let _moved: bool = self.poll_network_stop(cx, network);
        self.drive_radio(cx)?;
        // A message may have released the link's data: the plane takes it
        // now, so the network's next MSDU wakes this actor.
        self.connect_plane()?;
        self.flush(lifecycle);
        self.lifecycle_open(lifecycle)
    }

    /// The run ends once the lifecycle's reception closed: nothing the
    /// actor owes it is sent any more, and no wait for its room follows.
    /// What it did not send stays where it is: the owed answers with the
    /// actor, a refused terminal with the port.
    pub(super) fn lifecycle_open<const TO: usize, const FROM: usize, const RETURNS: usize>(
        &mut self,
        lifecycle: &LifecycleEnds<'_, 'id, D, TO, FROM, RETURNS>,
    ) -> Result<(), Failed<'id, D>> {
        if self.stage.closed
            || lifecycle.from_actor.is_closed()
            || lifecycle.returns.is_closed()
            || lifecycle.groups.is_closed()
            || lifecycle.link_return.is_closed()
        {
            self.stage.closed = true;
            return Err(ActorFault::Contract(ActorViolation::LifecycleClosed));
        }
        Ok(())
    }

    /// Put the next arena on the air the moment the published one ends,
    /// ahead of everything else the actor owes: the air waits for nothing
    /// but this. The standby first takes what the network queued. The held
    /// transmit facts stay for the turn, which serves the rest of them.
    pub(super) fn relay<
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
        let Some(tx) = self.facts.tx.filter(|tx| tx.bits() != 0) else {
            return Ok(());
        };
        if !self.data.plane.as_ref().is_some_and(DataPlane::on_air) {
            return Ok(());
        }
        D::span(ActorSpan::Intake, false);
        let taken = self.intake(lifecycle, network);
        D::span(ActorSpan::Intake, true);
        taken?;
        D::span(ActorSpan::Transmit, false);
        let relayed = self.relay_arena(tx);
        D::span(ActorSpan::Transmit, true);
        relayed
    }

    /// The arena's end on `tx`, the next arena ahead of its settlement.
    fn relay_arena(&mut self, tx: TxFacts) -> Result<(), Failed<'id, D>> {
        let done = self
            .port
            .service_arena(tx, self.now)
            .map_err(ActorFault::Port)?;
        let Some(done) = done else {
            return Ok(());
        };
        self.facts.tx = Some(tx.arena_ended());
        self.finish_arena(done)?;
        self.publish()
    }

    /// Publish the next arena ahead of `done` when the plane may, then
    /// settle `done`.
    fn finish_arena(&mut self, done: PortCompleted<'id, D::Port>) -> Result<(), Failed<'id, D>> {
        if let Err(failed) = self.publish_ahead(&done) {
            self.retained.completed = Some(done);
            return Err(failed);
        }
        self.settle(done)
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
        if facts.tx.is_some_and(|tx| tx.bits() != 0) || facts.tbtt {
            self.progressed = true;
        }
        // What the outbox refused waits in the port: after a confirmed stop
        // only `take_unconsumed` offers it, before one the service does.
        let refused = core::mem::take(&mut self.stage.refused);
        if refused {
            let mut outbox = Outbox {
                from_actor: &mut lifecycle.from_actor,
                returns: &mut lifecycle.returns,
                stage: &mut self.stage,
                grouped: self.group.is_some(),
            };
            if self.port.take_unconsumed(&mut outbox).is_ok() {
                self.staged(lifecycle, network)?;
            }
        }
        if facts.tx.is_some() || facts.tbtt || due || refused {
            let mut outbox = Outbox {
                from_actor: &mut lifecycle.from_actor,
                returns: &mut lifecycle.returns,
                stage: &mut self.stage,
                grouped: self.group.is_some(),
            };
            // No turn runs while an unkept item is kept: its place is
            // empty here.
            let served = match self.port.service_tx(
                facts.tx.unwrap_or(TxFacts::DUE),
                facts.tbtt,
                self.now,
                &mut outbox,
            ) {
                Ok(served) => served,
                Err(oer_protocols_ieee80211_contracts_port::Unkeeping { unkept, input }) => {
                    // The run ends on it; the arena an accepted call ended
                    // stays with the actor, and the port keeps its own
                    // fault. A call refused before it served
                    // (`NotAccepted(())`) ran nothing and answered nothing.
                    self.retained.unkept = Some(unkept);
                    if let Input::Accepted(Ok(Some(done))) = input {
                        self.retained.completed = Some(done);
                    }
                    return Err(ActorFault::Unkept);
                }
            };
            let done = served.map_err(ActorFault::Port)?;
            self.staged(lifecycle, network)?;
            if let Some(done) = done {
                self.finish_arena(done)?;
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
        if let Some(event) = self.stage.tbtt.take() {
            self.owed.tbtt = Some(event);
        }
        while let Some((frame, meta)) = self.stage.cut.pop_front() {
            D::received(frame.bytes(), &meta);
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
            let violation = refusal(&refused);
            self.retained.group = Some(refused.into_inner());
            ActorFault::Contract(violation)
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
        give_back(&lifecycle.from_actor, &lifecycle.returns, returned).map_err(
            |(returned, violation)| {
                self.retained.returned = Some(returned);
                ActorFault::Contract(violation)
            },
        )
    }

    /// Take the lifecycle's messages in order: at most
    /// [`MESSAGE_BUDGET`], of which at most one command or radio change. A
    /// command held at its barrier runs first; while it waits, or while the
    /// radio starts or moves, the next message waits, but a stop behind a
    /// held command withdraws it and runs.
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
    ) -> Result<Messages, Failed<'id, D>> {
        let mut heavy = false;
        if let Some(HeldCommand { command }) = self.held.take() {
            heavy = true;
            self.command(command, lifecycle, network)?;
            if self.held.is_some() {
                let next = self
                    .stash
                    .take()
                    .or_else(|| lifecycle.to_actor.try_receive());
                match next {
                    Some(stop @ ToActor::Stop { .. }) => {
                        self.message(cx, stop, lifecycle, network)?;
                    }
                    next => self.stash = next,
                }
                return Ok(Messages::Waiting);
            }
        }
        for _ in 0..MESSAGE_BUDGET {
            if self.radio.changing() && self.stash.is_some() {
                return Ok(Messages::Waiting);
            }
            let Some(message) = self
                .stash
                .take()
                .or_else(|| lifecycle.to_actor.try_receive())
            else {
                return Ok(Messages::Taken);
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
                    | ToActor::Sequence(_)
                    | ToActor::ReturnPairwise
                    | ToActor::LendPairwise(_)
            );
            let waits = self.radio.changing()
                && matches!(
                    message,
                    ToActor::StationTx(_) | ToActor::Port(_) | ToActor::Parcel
                );
            if waits {
                self.stash = Some(message);
                return Ok(Messages::Waiting);
            }
            if weighs && heavy {
                self.stash = Some(message);
                self.again = true;
                return Ok(Messages::Left);
            }
            heavy |= weighs;
            self.message(cx, message, lifecycle, network)?;
            if self.held.is_some() || self.end.is_some() {
                return Ok(Messages::Waiting);
            }
        }
        if lifecycle.to_actor.is_empty() {
            return Ok(Messages::Taken);
        }
        self.again = true;
        Ok(Messages::Left)
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
                    self.stop_unconfirmed = false;
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
                if matches!(message, ToActor::Release) {
                    self.sequence_hold = None;
                }
            }
            ToActor::Sequence(tid) => {
                // The number stays exact: the TID takes no data until the
                // request that names it.
                if let Some(link) = self.link.as_ref() {
                    self.owed.sequence = Some((tid, link.numbers().next(tid)));
                    self.sequence_hold = Some(tid);
                }
            }
            ToActor::ReturnPairwise => match self.pairwise.take() {
                Some(key) => self.owed.pairwise = Some(key),
                None => return Err(ActorFault::Contract(ActorViolation::PairwiseCustody)),
            },
            ToActor::LendPairwise(key) => {
                if self.pairwise.is_some() || self.link.is_some() {
                    self.retained.pairwise = Some(key);
                    return Err(ActorFault::Contract(ActorViolation::PairwiseCustody));
                }
                self.pairwise = Some(key);
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
                // The window moved or ended: its TID may go on.
                self.data.queues.unblock();
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
                match self.receive.offer(
                    self.now,
                    request.epoch(),
                    request.bssid(),
                    request.action(),
                    request.supported(),
                    &mut PortBlockAcks(&mut self.port),
                    &mut self.events,
                ) {
                    Ok(()) => {}
                    // No reply place or bank is left for the request: the
                    // access point hears nothing and asks again.
                    Err(RxBlockAckRefused::Capacity(_)) => {
                        report(&mut self.events, || ActorEvent::RepliesFull);
                    }
                    // The previous association's frames stay in their
                    // places; the run ends on the count.
                    Err(RxBlockAckRefused::Clear(fault)) => {
                        return Err(ActorFault::Receive(fault));
                    }
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
                self.recycle(frame)?;
                self.frames_out = self.frames_out.saturating_sub(1);
                // The room it frees may let the ring give units it held
                // back: the next turn pulls again.
                self.facts.rx = true;
                self.again = true;
            }
            ToActor::Stop {
                deadline,
                network: end,
            } => self.stop(deadline, end, lifecycle, network)?,
            ToActor::Retire if matches!(self.radio, Radio::Returning) => {
                self.finishing = true;
                if self.pairwise.is_some() {
                    return Err(ActorFault::Contract(ActorViolation::PairwiseCustody));
                }
                // The radio stopped; the actor ends once the network's
                // return came.
                self.retiring = true;
            }
            ToActor::Retire | ToActor::Reinstall => {
                self.finishing = true;
                if !matches!(self.radio, Radio::Halted | Radio::Stopped) {
                    // Only a stopped radio retires: the lifecycle stops it
                    // first.
                    return Err(ActorFault::Contract(ActorViolation::RetireRunning));
                }
                if self.pairwise.is_some() {
                    // The key goes back before the actor ends.
                    return Err(ActorFault::Contract(ActorViolation::PairwiseCustody));
                }
                self.end = Some(if matches!(message, ToActor::Reinstall) {
                    ActorEnd::Reinstall
                } else {
                    ActorEnd::Retired
                });
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
            from_actor: &mut lifecycle.from_actor,
            returns: &mut lifecycle.returns,
            stage: &mut self.stage,
            grouped: self.group.is_some(),
        };
        // No turn runs while an unkept item is kept: its place is empty
        // here.
        let (answer, unkept) = crate::answered(self.port.command(command, self.now, &mut outbox));
        let answer = match answer {
            Input::Accepted(answer) => answer,
            Input::NotAccepted(command) => {
                // Found before the port accepted it: it goes back to the
                // lifecycle as one the port did not take, with the group
                // token, and the run ends on the unkept item.
                self.retained.unkept = unkept;
                let group = if self.stage.group_terminal.is_none() {
                    self.group.take()
                } else {
                    None
                };
                let grouped = self.give_group(lifecycle, group)?;
                self.give(lifecycle, Returned::Unrun { command, grouped })?;
                return Err(ActorFault::Unkept);
            }
        };
        if unkept.is_some() {
            // The command's own answer is served below; the run then ends
            // on the unkept item.
            self.retained.unkept = unkept;
        }
        let progress = match answer {
            Ok(progress) => progress,
            Err(CommandFailed::Returned { command, fault }) => {
                // The port did not take it: it goes back to the lifecycle
                // with the group token, never issued again, and the fault
                // ends the run.
                let group = if self.stage.group_terminal.is_none() {
                    self.group.take()
                } else {
                    None
                };
                let grouped = self.give_group(lifecycle, group)?;
                self.give(lifecycle, Returned::Unrun { command, grouped })?;
                return Err(ActorFault::Port(fault));
            }
            Err(CommandFailed::Taken(fault)) => return Err(ActorFault::Port(fault)),
        };
        match progress {
            CommandProgress::Done => {}
            CommandProgress::Held(command) => self.held = Some(HeldCommand { command }),
            CommandProgress::Refused(command, reason) => self.refuse(command, reason, lifecycle)?,
        }
        if self.retained.unkept.is_some() {
            return Err(ActorFault::Unkept);
        }
        self.staged(lifecycle, network)
    }

    /// Give a command that did not run back with its owners and the group
    /// token: a refused group change ends without a terminal.
    fn refuse<const TO: usize, const FROM: usize, const RETURNS: usize>(
        &mut self,
        command: ActorCommand<'id, D>,
        reason: CommandRefusal,
        lifecycle: &LifecycleEnds<'_, 'id, D, TO, FROM, RETURNS>,
    ) -> Result<(), Failed<'id, D>> {
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
        )
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
            .clear(&mut PortBlockAcks(&mut self.port), &mut self.events)
            .map_err(ActorFault::Receive)?;
        self.reply_out = None;
        let Some(link) = self.link.take() else {
            return Err(ActorFault::Contract(ActorViolation::LinkMailbox));
        };
        self.sequence_hold = None;
        // The actor keeps the pairwise key, the one owner of its packet
        // numbers, until the lifecycle asks for it.
        let (link, key) = link.into_return();
        if let Some(key) = key {
            if self.pairwise.is_some() {
                self.retained.pairwise = Some(key);
                self.retained.link_return = Some(link);
                return Err(ActorFault::Contract(ActorViolation::PairwiseCustody));
            }
            self.pairwise = Some(key);
        }
        if let Err(refused) = lifecycle.link_return.try_send(link) {
            let violation = match refused {
                ClosingSpscSendError::Full(_) => ActorViolation::LinkReturn,
                ClosingSpscSendError::Closed(_) => ActorViolation::LifecycleClosed,
            };
            self.retained.link_return = Some(refused.into_inner());
            return Err(ActorFault::Contract(violation));
        }
        self.owed.link_returned = true;
        Ok(())
    }

    /// Begin the stop by `deadline`: the data's unpublished frames go back
    /// to the network, the receive agreements end and their units return,
    /// and the radio stops.
    fn stop<
        const TO: usize,
        const FROM: usize,
        const RETURNS: usize,
        NS: NetworkStop,
        const TX_DEPTH: usize,
        const RX_DEPTH: usize,
        N: Notifies,
    >(
        &mut self,
        deadline: Instant,
        end: NetworkEnd,
        lifecycle: &LifecycleEnds<'_, 'id, D, TO, FROM, RETURNS>,
        network: &mut NetworkEnds<'_, D::TxMemory, D::LentRx, NS, TX_DEPTH, RX_DEPTH, N>,
    ) -> Result<(), Failed<'id, D>> {
        // The stop withdraws the command held at its barrier: it goes back
        // with its owners, refused as the stopping radio refuses it, and
        // the stopping port forgets its barrier.
        if let Some(HeldCommand { command }) = self.held.take() {
            self.refuse(command, CommandRefusal::NotEnabled, lifecycle)?;
        }
        self.stop_data()?;
        self.receive
            .clear(&mut PortBlockAcks(&mut self.port), &mut self.events)
            .map_err(ActorFault::Receive)?;
        self.reply_out = None;
        self.return_freed(network);
        // The network stops once its memory is home: the stopped radio's
        // ended frames and its MSDUs still queued go back first
        // ([`Self::return_network_memory`]).
        if end == NetworkEnd::Stops {
            self.data.returns.owe_stop();
        }
        self.radio = match self.radio {
            Radio::Stopped | Radio::Halted => {
                if self.data.returns.stopping() == NetworkStopping::NotAsked {
                    self.owed.stopped = Some(StopOutcome::Confirmed);
                    Radio::Halted
                } else {
                    Radio::Draining { deadline }
                }
            }
            _ => Radio::Stopping { deadline },
        };
        Ok(())
    }

    /// Follow the network side's stop and its report; whether it moved,
    /// which the stop's turn then serves.
    fn poll_network_stop<
        NS: NetworkStop,
        const TX_DEPTH: usize,
        const RX_DEPTH: usize,
        N: Notifies,
    >(
        &mut self,
        cx: &mut Context<'_>,
        network: &NetworkEnds<'_, D::TxMemory, D::LentRx, NS, TX_DEPTH, RX_DEPTH, N>,
    ) -> bool {
        self.data.returns.poll_stop(cx, network)
    }

    /// Whether the network side owes the stop nothing: no stop asked it to
    /// stop, or it stopped. A unit it still holds does not hold the stop:
    /// the lend count outlives the installation, and the unit rejoins
    /// whichever ring runs when the network drops it. A stop it is owed
    /// settles nothing yet.
    fn network_settled(&self) -> bool {
        self.data.returns.settled(self.runner_home())
    }

    /// Whether nothing of the station's own owes the network's memory an
    /// end: the data settled and its queues are empty.
    fn runner_home(&self) -> bool {
        self.data_settled() && self.data.queues.is_empty()
    }

    /// Whether no arena owes its end: none on the air and none holding a
    /// frame whose terminal has not come, so no later settlement adds the
    /// network's memory to the return.
    pub(super) fn data_settled(&self) -> bool {
        self.data
            .plane
            .as_ref()
            .is_none_or(|plane| !plane.on_air() && !plane.holds_frames())
    }

    /// The answer of a stop whose network settled: confirmed, or the
    /// radio's confirmed stop with the network's participant lost.
    fn network_outcome(
        &self,
        observed: &StopObserved<PortFault<'id, D>>,
    ) -> StopOutcome<PortFault<'id, D>> {
        if self.data.returns.lost() {
            StopOutcome::NetworkLost(ReturnObserved {
                memory_held: observed.memory_held,
                network_running: false,
                lent: observed.lent,
            })
        } else {
            StopOutcome::Confirmed
        }
    }

    /// What a stop still holds.
    fn observed(
        &self,
        port: Option<StopUnconfirmed<PortFault<'id, D>>>,
        deadline: bool,
    ) -> StopObserved<PortFault<'id, D>> {
        StopObserved {
            port,
            deadline,
            // An arena whose end is owed: on the air, or holding frames the
            // port returned its right to before their terminal came.
            arena_on_air: self
                .data
                .plane
                .as_ref()
                .is_some_and(|plane| plane.on_air() || plane.holds_frames()),
            frames_out: self.frames_out,
            network_running: self.data.returns.asked(),
            network_lost: self.network_lost(),
            memory_held: u16::try_from(self.data.returns.held()).unwrap_or(u16::MAX),
            lent: u16::try_from(self.port.lent()).unwrap_or(u16::MAX),
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
                    self.radio = after_refused_start(&refused);
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
                    self.stop_unconfirmed = true;
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
                    self.radio = after_refused_start(&refused);
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
                        let data_home = !observed.arena_on_air && observed.frames_out == 0;
                        if data_home && self.network_settled() {
                            Some(self.network_outcome(&observed))
                        } else if data_home {
                            // The radio and its data are done: what is left
                            // is the network's, whose memory goes home
                            // before it stops. A lost participant changes
                            // nothing of the radio's stop.
                            self.radio = Radio::Draining { deadline };
                            self.again = true;
                            None
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
                    self.stop_unconfirmed = matches!(outcome, StopOutcome::Unconfirmed(_));
                    self.owed.stopped = Some(outcome);
                }
            }
            Radio::Draining { deadline } => {
                let late = now >= deadline;
                let observed = self.observed(None, late);
                if self.network_settled() {
                    self.radio = Radio::Halted;
                    self.owed.stopped = Some(self.network_outcome(&observed));
                } else if late {
                    // The radio stopped: only the network's return is late.
                    // The wait ends here; the return does not.
                    self.radio = Radio::Returning;
                    self.owed.stopped = Some(StopOutcome::Returning(ReturnObserved {
                        memory_held: observed.memory_held,
                        network_running: observed.network_running,
                        lent: observed.lent,
                    }));
                }
            }
            Radio::Returning => {
                // A lost participant settles once the station's own share
                // came home ([`Self::network_home`]); the composition reads
                // the loss ([`Actor::network_lost`]).
                if self.network_settled() {
                    self.radio = Radio::Halted;
                    if self.retiring {
                        self.end = Some(ActorEnd::Retired);
                    }
                }
            }
            Radio::Stopped | Radio::Unstopped | Radio::Running | Radio::Halted => {}
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
            self.owed.started = match refused.into_inner() {
                FromActor::Started(started) => Some(started),
                _ => None,
            };
        }
        if let Some(retuned) = self.owed.retuned.take()
            && let Err(refused) = from.try_send(FromActor::Retuned(retuned))
        {
            self.owed.retuned = match refused.into_inner() {
                FromActor::Retuned(retuned) => Some(retuned),
                _ => None,
            };
        }
        if self.owed.link_returned && from.try_send(FromActor::LinkReturned).is_ok() {
            self.owed.link_returned = false;
        }
        if let Some((tid, next)) = self.owed.sequence
            && from.try_send(FromActor::TxSequence { tid, next }).is_ok()
        {
            self.owed.sequence = None;
        }
        if let Some(key) = self.owed.pairwise.take()
            && let Err(refused) = from.try_send(FromActor::PairwiseReturned(key))
        {
            self.owed.pairwise = match refused.into_inner() {
                FromActor::PairwiseReturned(key) => Some(key),
                _ => None,
            };
        }
        if let Some(gate) = self.owed.gate.take()
            && let Err(refused) = from.try_send(FromActor::GateOpened(gate))
        {
            self.owed.gate = match refused.into_inner() {
                FromActor::GateOpened(gate) => Some(gate),
                _ => None,
            };
        }
        if let Some(stopped) = self.owed.stopped.take()
            && let Err(refused) = from.try_send(FromActor::Stopped(stopped))
        {
            self.owed.stopped = match refused.into_inner() {
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
            |from: &ClosingSpscSender<'_, FromActor<'id, D>, FROM>| from.free_capacity() > reserve;
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

    /// Return `frame` to the port. A refusal keeps the unit with the actor
    /// and ends the run: the caller returns the fault at once, and no turn
    /// runs while the unit is kept ([`Actor::take_unrecycled`]), so the
    /// one place is empty here.
    pub(super) fn recycle(&mut self, frame: ActorRxBuffer<'id, D>) -> Result<(), Failed<'id, D>> {
        self.port.recycle(frame).map_err(|refused| {
            let fault = refused.fault;
            self.retained.unrecycled = Some(refused);
            ActorFault::Receive(fault)
        })
    }

    /// Serve `fault`: the first one the port can stop past asks the
    /// lifecycle for a restart. Any other ends the run; the lifecycle hears
    /// it when its queue has room, and every owner stays where it is.
    pub(super) fn fault<const TO: usize, const FROM: usize, const RETURNS: usize>(
        &mut self,
        lifecycle: &LifecycleEnds<'_, 'id, D, TO, FROM, RETURNS>,
        fault: Failed<'id, D>,
    ) {
        // A refused recycle is final: its unit stays kept, so no restart.
        // So is an unkept item: it stays kept, and no port call follows.
        // And any fault after the lifecycle's last message: the lifecycle
        // ended and hears no recovery, so nothing would restart.
        if !self.recovering
            && !self.finishing
            && !matches!(
                fault,
                ActorFault::Receive(_)
                    | ActorFault::Unkept
                    | ActorFault::Unaccepted
                    | ActorFault::Contract(ActorViolation::LifecycleClosed)
            )
            && self.retained.unkept.is_none()
            && self.retained.unaccepted.is_none()
            && self.port.recover_for_stop()
            && lifecycle
                .from_actor
                .try_send(FromActor::Recovering(fault))
                .is_ok()
        {
            self.recovering = true;
            return;
        }
        let _told = lifecycle.from_actor.try_send(FromActor::Faulted(fault));
        self.end = Some(ActorEnd::Faulted(fault));
    }
}

/// The radio after a refused start: stopped, or stopping still when the
/// port's stop of the begun start has not confirmed.
const fn after_refused_start<K, F>(refused: &StartRefused<F>) -> Radio<K> {
    match refused {
        StartRefused::Unstopped { .. } => Radio::Unstopped,
        StartRefused::Running
        | StartRefused::Retune(_)
        | StartRefused::Deadline
        | StartRefused::Device => Radio::Stopped,
    }
}
