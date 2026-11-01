use super::*;
use super::{
    keys::{GroupCommand, Installation, Installed, RawKeyIdentity, Removal},
    lifecycle::PortCoreOwner,
};
use oer_ieee80211_wire::qos::WmmAccessCategory;

mod aggregate;
pub(super) use aggregate::PendingAggregate;
use aggregate::{Queued, Running, admitted_key};
mod core_views;
#[cfg(oer_debug_control)]
mod key_replacement;
// A single phase projection lends the shared command set. The operation
// itself has one body; invalid P1 commands remain typed capacity refusals.
macro_rules! idle_command {
    ($owner:expr, |$view:ident| $command:expr) => {{
        let mut $view = $owner.view().commands().ok_or(Fault::Capacity)?;
        $command
    }};
}

pub(super) struct Pending<'id, Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> {
    pub operation: Single<'id, Id, D, B, O>,
    pub plan: RetryPlan<PhyRate>,
    pub category: WmmAccessCategory,
    pub protection: Protection,
    pub power: TxPower,
    pub coex: TxOperation,
    pub stage: usize,
    pub stage_tries: u8,
    pub transmissions: u16,
    pub publications: u16,
    pub air_rate: Option<PhyRate>,
}
impl<Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> Pending<'_, Id, D, B, O> {
    fn role(&self) -> VifRole {
        self.operation.payload().role
    }
    fn key(&self) -> Option<RawKeyIdentity> {
        self.operation.payload().key
    }
    fn air(&self) -> Option<Air> {
        Some(Air {
            transmissions: core::num::NonZeroU16::new(self.transmissions)?,
            rate: self.air_rate?,
        })
    }
}
pub(super) enum Command<'id, Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> {
    Submit(Pending<'id, Id, D, B, O>),
    SubmitAggregate(PendingAggregate<'id, Id, D, B, O>),
    Configure {
        permission: owned::OwnedConfigured<'id, Id, D, B>,
        config: CoreVif,
    },
    CloseGate(GateOpen<'id, Part<Id, 1>>),
    OpenGate,
    Install {
        identity: RawKeyIdentity,
        installation: Installation<'id, Id>,
        material: Ccmp128Key,
    },
    Remove {
        identity: RawKeyIdentity,
        removal: Removal<'id, Id>,
    },
    Group {
        at: Instant,
        command: GroupCommand<'id, Id>,
    },
}
struct Completion<'id, Id: SlotIdentity, const B: usize> {
    queue: FreeQueue<'id, CorePath<Id>>,
    ticket: TxTicket<'id, CorePath<Id>>,
    slot: Pin<&'static mut TxSlot<B>>,
    end: PublicationEnd,
    rate: PhyRate,
}
pub(super) struct Sink<'id, Id: SlotIdentity, const B: usize> {
    completed: heapless::Vec<Completion<'id, Id, B>, 4>,
    /// The one aggregate a completion of the core returned.
    aggregates: heapless::Vec<
        (
            FreeQueue<'id, CorePath<Id>>,
            TxTicket<'id, CorePath<Id>>,
            PublicationEnd,
        ),
        1,
    >,
    removed: heapless::Vec<KeyRemoved<'id, CorePath<Id>>, 20>,
    tbtt: Option<TbttEvent>,
    /// The first callback the port's core contract forbids: a second
    /// completion of a queue in one service, a second aggregate completion,
    /// or more removals than installed keys. Its owners are retained here,
    /// never returned to a pool, and the service poisons.
    refused: Option<(Fault, Refused<'id, Id, B>)>,
}
/// Owners of a refused callback, kept until the port's quarantine ends.
#[allow(dead_code, reason = "retained owners are never read, only kept")]
enum Refused<'id, Id: SlotIdentity, const B: usize> {
    Completion(Completion<'id, Id, B>),
    Aggregate(FreeQueue<'id, CorePath<Id>>, TxTicket<'id, CorePath<Id>>),
    Removal(KeyRemoved<'id, CorePath<Id>>),
}
impl<'id, Id: SlotIdentity, const B: usize> Sink<'id, Id, B> {
    fn refuse(&mut self, fault: Fault, owners: Refused<'id, Id, B>) {
        if self.refused.is_none() {
            self.refused = Some((fault, owners));
        } else {
            // A later refusal adds no cause; its tokens stay unusable.
            let _quarantined = core::mem::ManuallyDrop::new(owners);
        }
    }
    /// The recorded contract fault, retaining its owners in quarantine.
    fn fault(&mut self) -> Option<Fault> {
        let (fault, owners) = self.refused.take()?;
        let _quarantined = core::mem::ManuallyDrop::new(owners);
        Some(fault)
    }
}
impl<Id: SlotIdentity, const B: usize> Default for Sink<'_, Id, B> {
    fn default() -> Self {
        Self {
            completed: heapless::Vec::new(),
            aggregates: heapless::Vec::new(),
            removed: heapless::Vec::new(),
            tbtt: None,
            refused: None,
        }
    }
}
impl<'id, Id: SlotIdentity, const B: usize> LowerMacSink<'id, 'static, B, CorePath<Id>>
    for Sink<'id, Id, B>
{
    fn tx_completed(
        &mut self,
        queue: FreeQueue<'id, CorePath<Id>>,
        ticket: TxTicket<'id, CorePath<Id>>,
        slot: Pin<&'static mut TxSlot<B>>,
        end: PublicationEnd,
        rate: PhyRate,
    ) {
        // One completion per hardware queue per service.
        if let Err(completion) = self.completed.push(Completion {
            queue,
            ticket,
            slot,
            end,
            rate,
        }) {
            self.refuse(Fault::Capacity, Refused::Completion(completion));
        }
    }
    fn ampdu_completed(
        &mut self,
        queue: FreeQueue<'id, CorePath<Id>>,
        ticket: TxTicket<'id, CorePath<Id>>,
        end: PublicationEnd,
    ) {
        // One aggregate completes per call of the core.
        if let Err((queue, ticket, _)) = self.aggregates.push((queue, ticket, end)) {
            self.refuse(Fault::Capacity, Refused::Aggregate(queue, ticket));
        }
    }
    fn key_removed(&mut self, removed: KeyRemoved<'id, CorePath<Id>>) {
        // One removal per installed hardware key.
        if let Err(removed) = self.removed.push(removed) {
            self.refuse(Fault::Capacity, Refused::Removal(removed));
        }
    }
    fn disabled(&mut self) {}
    fn tbtt(&mut self, event: TbttEvent) {
        self.tbtt = Some(event);
    }
}

pub(super) struct Driver<'id, Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> {
    core: PortCoreOwner<'id, Id, D, B>,
    tickets: [Option<TxTicket<'id, CorePath<Id>>>; TX_TICKETS],
    free: [Option<FreeQueue<'id, CorePath<Id>>>; 4],
    queues: [heapless::Deque<Queued<'id, Id, D, B, O>, TX_TICKETS>; 4],
    running: [Option<Running<'id, Id, D, B, O>>; 4],
    /// Each queue's ordinary descriptor, while no MPDU of it is published.
    descriptors: [Option<Pin<&'static mut TxSlot<B>>>; TX_DESCRIPTORS],
    /// The aggregate chains no aggregate holds.
    chains: [Option<AmpduChain<B>>; AMPDU_CHAINS],
    pending_command: Option<Command<'id, Id, D, B, O>>,
    configuring: Option<owned::OwnedConfigured<'id, Id, D, B>>,
    closing: Option<GateOpen<'id, Part<Id, 1>>>,
    closed: bool,
    keys: [Option<(RawKeyIdentity, Installed<'id, Id>)>; 20],
    applied_ba: [Option<RxBlockAckValues>; 8],
    transition: Option<Instant>,
    clock_deadline: Instant,
    /// The one receiver of this driver's publication observations, present
    /// from preparation until retirement.
    observations: Option<O>,
}
impl<'id, Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations>
    Driver<'id, Id, D, B, O>
{
    /// The transmitter of a disabled core; any other core keeps it, and the
    /// stopped port then refuses reinstallation.
    pub(super) fn take_transmitter(
        &mut self,
    ) -> Option<OrdinaryTxIdleOwner<'static, D::Power, D::Entropy, D::Timer, B>> {
        self.core
            .dismantle()
            .ok()
            .map(LowerMacCoreParts::into_transmitter)
    }
    /// Every descriptor, when each is back and idle; a missing or busy one
    /// keeps them all, and the stopped port then refuses reinstallation.
    fn take_descriptors(&mut self) -> Option<TxDescriptors<B>> {
        let idle = self
            .descriptors
            .iter()
            .flatten()
            .all(|slot| slot.state() == crate::mac::tx::TxDmaState::Free && !slot.attached())
            && self
                .chains
                .iter()
                .flatten()
                .all(|chain| chain.is_fully_free());
        if !idle
            || self.descriptors.iter().any(Option::is_none)
            || self.chains.iter().any(Option::is_none)
        {
            return None;
        }
        let [Some(a), Some(b), Some(c), Some(d)] = self.descriptors.each_mut().map(Option::take)
        else {
            return None;
        };
        let [Some(first), Some(second)] = self.chains.each_mut().map(Option::take) else {
            return None;
        };
        Some(TxDescriptors {
            single: [a, b, c, d],
            chains: [first, second],
        })
    }
    /// A retired driver holds no admitted work for a new installation.
    pub(super) fn retired(&self) -> bool {
        self.queues.iter().all(|q| q.is_empty()) && self.running.iter().all(Option::is_none)
    }
    pub(super) const fn vacant() -> Self {
        Self {
            core: PortCoreOwner::<Id, D, B>::vacant(),
            tickets: [const { None }; TX_TICKETS],
            free: [const { None }; 4],
            queues: [const { heapless::Deque::new() }; 4],
            running: [const { None }; 4],
            descriptors: [const { None }; TX_DESCRIPTORS],
            chains: [const { None }; AMPDU_CHAINS],
            pending_command: None,
            configuring: None,
            closing: None,
            closed: false,
            keys: [const { None }; 20],
            applied_ba: [None; 8],
            transition: None,
            clock_deadline: Instant::from_micros(0),
            observations: None,
        }
    }
    #[inline(never)]
    pub(super) fn prepare(
        &mut self,
        core: PortCoreOwner<'id, Id, D, B>,
        tickets: [TxTicket<'id, CorePath<Id>>; TX_TICKETS],
        queues: [FreeQueue<'id, CorePath<Id>>; 4],
        descriptors: TxDescriptors<B>,
        observations: O,
        now: Instant,
    ) {
        // The one caller checked `retired` before taking any resource.
        self.core = core;
        self.tickets = tickets.map(Some);
        self.free = queues.map(Some);
        let TxDescriptors { single, chains } = descriptors;
        self.descriptors = single.map(Some);
        self.chains = chains.map(Some);
        self.observations = Some(observations);
        self.clock_deadline = now.saturating_add(Duration::from_secs(60));
    }
    pub(super) fn retire(&mut self) -> Option<Reusable<D, B, O>> {
        let transmitter = self.take_transmitter();
        let observations = self.observations.take();
        for queue in &mut self.queues {
            queue.clear();
        }
        for owner in &mut self.running {
            drop(owner.take());
        }
        let descriptors = self.take_descriptors();
        for ticket in &mut self.tickets {
            drop(ticket.take());
        }
        for queue in &mut self.free {
            drop(queue.take());
        }
        for key in &mut self.keys {
            drop(key.take());
        }
        drop(self.pending_command.take());
        drop(self.configuring.take());
        drop(self.closing.take());
        self.closed = false;
        self.applied_ba = [None; 8];
        self.transition = None;
        let (transmitter, descriptors) = transmitter.zip(descriptors)?;
        Some((transmitter, descriptors, observations?))
    }
    pub(super) fn deadline(&mut self, held: &Held<'_, 'id, Id, D, B, O>) -> Result<Instant, Fault> {
        // The clock sample waits for a start in flight; its own bound wakes
        // the driver meanwhile.
        let clock = if starting(held) {
            Instant::from_micros(u64::MAX)
        } else {
            self.clock_deadline
        };
        let mut deadline = core_views::deadline::<Id, D, B>(self.core.view())
            .unwrap_or(clock)
            .min(clock);
        if let Some(device) = held.storage.device()?.next_deadline() {
            deadline = deadline.min(device);
        }
        if let Some(start) = self.transition {
            deadline = deadline.min(start.saturating_add(COMMAND_BOUND));
        }
        if let Some(Command::Group { at, .. }) = self.pending_command.as_ref() {
            deadline = deadline.min(at.saturating_add(COMMAND_BOUND));
        }
        if !core_views::configuring::<Id, D, B>(self.core.view()) {
            for pending in self.queues.iter().flat_map(|q| q.iter()) {
                if let Some(at) = pending.admission() {
                    let lifetime = held.edca.lifetime(vif(pending.role()), pending.category());
                    deadline = deadline.min(at.saturating_add(lifetime));
                }
            }
        }
        for at in self.running.iter().flatten().filter_map(Running::deadline) {
            deadline = deadline.min(at);
        }
        Ok(deadline)
    }
    fn finish(
        &mut self,
        held: &Held<'_, 'id, Id, D, B, O>,
        pending: Pending<'id, Id, D, B, O>,
        outcome: SingleOutcome<Ack, Infallible>,
    ) {
        if pending.publications != 0 {
            core_views::reset_contention::<Id, D, B>(
                self.core.view(),
                pending.role(),
                pending.category,
            );
        }
        if let TxOutcome::Expired { msdus } = outcome {
            held.storage.expired.set(
                held.storage
                    .expired
                    .get()
                    .saturating_add(msdus.count_ones()),
            );
        }
        let place = pending.operation.ticket().index();
        held.tx_completed(
            place,
            owned::OwnedTxCompleted::Single(pending.operation.complete(outcome)),
        );
    }
    // The retained barrier is also the withdrawal marker; no owner or result
    // is moved out of its AC while an earlier owner can still complete.
    fn withdrawal(&self, role: VifRole, key: Option<RawKeyIdentity>) -> Option<WithdrawCause> {
        if self.configuring.as_ref().is_some_and(|p| p.role() == role) {
            return Some(WithdrawCause::Disabled);
        }
        match &self.pending_command {
            Some(Command::Remove { identity, .. })
            | Some(Command::Group {
                command: GroupCommand::Remove { identity, .. },
                ..
            }) if key == Some(*identity) => Some(WithdrawCause::KeyRemoved),
            _ => None,
        }
    }
    /// End a queued entry, which no publication holds, as withdrawn.
    fn withdraw_queued(
        &mut self,
        held: &Held<'_, 'id, Id, D, B, O>,
        queued: Queued<'id, Id, D, B, O>,
        cause: WithdrawCause,
    ) {
        match queued {
            Queued::Single(pending) => self.finish(held, pending, TxOutcome::Withdrawn(cause)),
            Queued::Aggregate(pending) => {
                self.finish_aggregate(held, pending, TxOutcome::Withdrawn(cause), false)
            }
        }
    }
    #[inline(never)]
    fn withdraw(&mut self, held: &Held<'_, 'id, Id, D, B, O>) -> bool {
        let global = if !held.want_enabled.get() {
            Some(WithdrawCause::Disabled)
        } else if self.closing.is_some() {
            Some(WithdrawCause::GateClosed)
        } else {
            None
        };
        // Without a global cause, a configuration or a key removal, no
        // owner is withdrawn (`withdrawal` names none): the common turn.
        let selective = self.configuring.is_some()
            || matches!(
                self.pending_command,
                Some(Command::Remove { .. })
                    | Some(Command::Group {
                        command: GroupCommand::Remove { .. },
                        ..
                    })
            );
        if global.is_none() && !selective {
            return true;
        }
        for index in 0..4 {
            if self.running[index].is_some() {
                continue;
            }
            // A selective barrier cannot return an owner past either a
            // published head or an earlier unaffected queued owner.
            while let Some(cause) = self.queues[index].front().and_then(|pending| {
                global.or_else(|| self.withdrawal(pending.role(), pending.key()))
            }) {
                let Some(queued) = self.queues[index].pop_front() else {
                    break;
                };
                self.withdraw_queued(held, queued, cause);
            }
        }
        !self
            .queues
            .iter()
            .flat_map(|queue| queue.iter())
            .map(|queued| (queued.role(), queued.key()))
            .chain(
                self.running
                    .iter()
                    .flatten()
                    .map(|running| (running.role(), running.key())),
            )
            .any(|(role, key)| self.withdrawal(role, key).is_some())
    }
    #[inline(always)]
    fn blocked(&self, pending: &Queued<'id, Id, D, B, O>) -> bool {
        self.withdrawal(pending.role(), pending.key()).is_some()
            || match &self.pending_command {
                Some(Command::Remove { .. })
                | Some(Command::Group {
                    command: GroupCommand::Remove { .. },
                    ..
                }) => false,
                Some(_) => true,
                None => false,
            }
    }
    fn expired(
        held: &Held<'_, 'id, Id, D, B, O>,
        pending: &Queued<'id, Id, D, B, O>,
        now: Instant,
    ) -> bool {
        pending.admission().is_some_and(|at| {
            now.saturating_duration_since(at)
                >= held.edca.lifetime(vif(pending.role()), pending.category())
        })
    }
    fn collect(
        &mut self,
        held: &Held<'_, 'id, Id, D, B, O>,
        sink: &mut Sink<'id, Id, B>,
        stopping: bool,
    ) -> Result<(), Fault> {
        if let Some(fault) = sink.fault() {
            return Err(fault);
        }
        if !sink.aggregates.is_empty() {
            // Only an aggregate's own completion returns one.
            return Err(Fault::Capacity);
        }
        for completion in sink.completed.drain(..) {
            let index = queue(completion.queue.category());
            let Some(Running::Single(mut pending)) = self.running[index].take() else {
                return Err(Fault::Capacity);
            };
            self.free[index] = Some(completion.queue);
            let ticket_index = completion.ticket.index();
            self.tickets[ticket_index] = Some(completion.ticket);
            let mut slot = completion.slot;
            // The buffer leaves the queue's descriptor and rejoins the right
            // its publication kept.
            let memory = slot.as_mut().detach().ok_or(Fault::Capacity)?;
            *self.descriptors.get_mut(index).ok_or(Fault::Capacity)? = Some(slot);
            if pending
                .operation
                .execute()
                .mpdu
                .buffer
                .dma
                .complete(memory)
                .is_err()
            {
                return Err(Fault::Capacity);
            }
            let sent = matches!(
                completion.end,
                PublicationEnd::Completed {
                    status: TxStatus::Success | TxStatus::AckTimeout,
                    ..
                }
            );
            if sent {
                pending.transmissions = pending
                    .transmissions
                    .checked_add(1)
                    .ok_or(Fault::Capacity)?;
                pending.air_rate = Some(completion.rate);
            }
            let outcome = match completion.end {
                PublicationEnd::Expired { msdus } => Some(TxOutcome::Expired { msdus }),
                PublicationEnd::Withdrawn => Some(TxOutcome::Withdrawn(WithdrawCause::Disabled)),
                PublicationEnd::TimedOut => Some(TxOutcome::Indeterminate(
                    IndeterminateCause::HardwareTimeout,
                )),
                PublicationEnd::Completed {
                    status: TxStatus::Success,
                    ack_snr_db,
                    block_ack,
                } => {
                    let response = match pending.operation.payload().layout.kind() {
                        MpduKind::Group | MpduKind::IndividualNoAck => SingleResponse::Unsolicited,
                        MpduKind::Individual => SingleResponse::Ack(Ack::from_sample(ack_snr_db)?),
                        MpduKind::BlockAckRequest => SingleResponse::BlockAck {
                            report: block_ack.ok_or(Fault::Capacity)?,
                            ack: Ack::from_sample(ack_snr_db)?,
                        },
                    };
                    Some(TxOutcome::Ended(SingleEnd::Delivered {
                        air: pending.air().ok_or(Fault::Capacity)?,
                        response,
                    }))
                }
                end => {
                    pending.stage_tries =
                        pending.stage_tries.checked_add(1).ok_or(Fault::Capacity)?;
                    let stage = pending
                        .plan
                        .stages()
                        .nth(pending.stage)
                        .ok_or(Fault::Capacity)?;
                    if pending.stage_tries >= stage.transmissions.get() {
                        pending.stage += 1;
                        pending.stage_tries = 0;
                    }
                    if pending.plan.stages().nth(pending.stage).is_none() {
                        Some(TxOutcome::Ended(match end {
                            PublicationEnd::Collision
                            | PublicationEnd::Completed {
                                status: TxStatus::Collision,
                                ..
                            } => SingleEnd::ContentionLimit(pending.air()),
                            PublicationEnd::Completed {
                                status: TxStatus::CtsTimeout,
                                ..
                            } => SingleEnd::CtsTimeout(pending.air()),
                            _ => SingleEnd::AckTimeout(pending.air().ok_or(Fault::Capacity)?),
                        }))
                    } else if stopping {
                        Some(TxOutcome::Withdrawn(WithdrawCause::Disabled))
                    } else if self.closing.is_some() || self.closed {
                        Some(TxOutcome::Withdrawn(WithdrawCause::GateClosed))
                    } else {
                        None
                    }
                }
            };
            if let Some(outcome) = outcome {
                self.finish(held, pending, outcome);
            } else if self.queues[index]
                .push_front(Queued::Single(pending))
                .is_err()
            {
                return Err(Fault::Capacity);
            }
        }
        if let Some(tbtt) = sink.tbtt.take() {
            if held.events.push(owned::OwnedEvent::Tbtt(tbtt)).is_err() {
                held.loss.set(true);
            }
            held.storage.shared.wake_body();
        }
        if !sink.removed.is_empty() {
            return Err(Fault::Capacity);
        }
        self.withdraw(held);
        Ok(())
    }
    /// Offer one TX edge to the core's MPDUs and to the published
    /// aggregates. Their timeout aborts force CCA one at a time: when one
    /// side's settle ends, the other side's latched timeouts are offered
    /// again, as the core does among its queues.
    fn service_tx(
        &mut self,
        held: &Held<'_, 'id, Id, D, B, O>,
        device: &mut D,
        wake: WifiTxWake,
        stopping: bool,
    ) -> Result<(), Fault> {
        if !self.aggregate_running(None) {
            // No aggregate: the MPDUs' settles are the core's own.
            let mut sink = Sink::default();
            core_views::service::<Id, D, B>(self.core.view(), device.hardware()?, wake, &mut sink)?;
            return self.collect(held, &mut sink, stopping);
        }
        let settling = |driver: &mut Self| {
            driver
                .core
                .view()
                .aggregates()
                .is_some_and(|view| view.abort_settling())
        };
        let timeout = WifiTxWake::Interrupt {
            events: crate::mac::irq::EVENT_TX_TIMEOUT,
        };
        let before = settling(self);
        let mut sink = Sink::default();
        core_views::service::<Id, D, B>(self.core.view(), device.hardware()?, wake, &mut sink)?;
        self.collect(held, &mut sink, stopping)?;
        let settled = before && !settling(self);
        if self.service_aggregates(held, device, if settled { timeout } else { wake })? {
            let mut sink = Sink::default();
            core_views::service::<Id, D, B>(
                self.core.view(),
                device.hardware()?,
                timeout,
                &mut sink,
            )?;
            self.collect(held, &mut sink, stopping)?;
        }
        Ok(())
    }
    fn receive(
        &mut self,
        held: &Held<'_, 'id, Id, D, B, O>,
        device: &mut D,
        unit: D::Receive,
    ) -> Result<(), Fault> {
        if held.rx_out.get() >= RX_HELD {
            core::mem::forget(unit);
            return Err(Fault::Capacity);
        }
        let accepted = {
            match unit.normalized() {
                Ok(frame) => {
                    core_views::received::<Id, D, B>(self.core.view(), device.hardware()?, &frame)
                        .map(|(bytes, meta)| {
                            (
                                bytes
                                    .as_ptr()
                                    .addr()
                                    .checked_sub(unit.bytes().as_ptr().addr()),
                                bytes.len(),
                                meta,
                            )
                        })
                }
                Err(_) => None,
            }
        };
        if let Some((Some(start), len, meta)) = accepted {
            let mut unit = unit;
            // The buffer lends the MPDU from here on: its view is narrowed
            // once, checked against the received unit.
            unit.narrow(start, len).map_err(|_| Fault::Capacity)?;
            held.rx_out.set(held.rx_out.get() + 1);
            if let Err(event) = held.events.push(owned::OwnedEvent::Received {
                frame: ReceiveBuffer::new(
                    unit,
                    super::LossGuard::new(
                        &held.storage.shared.poisoned,
                        &held.storage.shared.driver,
                    ),
                ),
                meta,
            }) {
                core::mem::forget(event);
                return Err(Fault::Capacity);
            }
            held.storage.shared.received.queue();
            held.storage.shared.wake_body();
        } else {
            device.recycle(unit);
        }
        Ok(())
    }
}
/// Publication reports to the driver's observation receiver.
impl<'id, Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations>
    Driver<'id, Id, D, B, O>
{
    pub(super) fn poll(
        &mut self,
        cx: &mut Context<'_>,
        held: &Held<'_, 'id, Id, D, B, O>,
        device: &mut D,
    ) -> Result<(), Fault> {
        while let Some(frame) = held.recycled.pop() {
            device.recycle(frame.into_unit().0);
            held.rx_out
                .set(held.rx_out.get().checked_sub(1).ok_or(Fault::Capacity)?);
        }
        let network = &held.storage.shared.network;
        let transferred = network.take_transferred();
        held.rx_out.set(
            held.rx_out
                .get()
                .checked_sub(transferred)
                .ok_or(Fault::Capacity)?,
        );
        if network.take_returned() {
            device.returned();
        }
        let now = device.timer().now();
        // A start in flight may lend the device's registers to the radio
        // actor (a retune): the clock is sampled once the start ended.
        if !starting(held) && now >= self.clock_deadline {
            device.radio_now().map_err(|_| Fault::Clock)?;
            self.clock_deadline = now.saturating_add(Duration::from_secs(60));
        }
        if self
            .transition
            .is_some_and(|start| now.saturating_duration_since(start) >= COMMAND_BOUND)
        {
            return Err(Fault::Deadline);
        }
        if held.want_enabled.get() && !held.enabled.get() {
            self.transition.get_or_insert(now);
            let channel = held
                .channel
                .get()
                .map_err(Fault::Setting)?
                .wifi_channel()
                .ok_or(Fault::Capacity)?;
            match device.poll_start(cx, channel) {
                Poll::Pending => return Ok(()),
                Poll::Ready(Err(cause)) => return Err(cause),
                Poll::Ready(Ok(())) => {
                    core_views::tune::<Id, D, B>(
                        self.core.view(),
                        channel,
                        device.take_power_profile(),
                    )?;
                    held.enabled.set(true);
                    self.transition = None;
                    held.storage.shared.wake_body();
                }
            }
        }
        if !held.enabled.get() {
            return Ok(());
        }
        let stopping = !held.want_enabled.get();
        if stopping {
            self.transition.get_or_insert(now);
        }
        #[cfg(oer_debug_control)]
        let completion_held = held.completion_hold.get();
        #[cfg(not(oer_debug_control))]
        let completion_held = false;
        if !completion_held {
            self.service_tx(held, device, WifiTxWake::Deadline, stopping)?;
        }
        // One turn serves up to `EVENTS_PER_POLL` events. A turn that ends
        // on `Pending` is registered for the next one; a turn that used its
        // budget, or served a TX wake whose completions freed queues, polls
        // again. Received units wait in the ring while P1 holds
        // `RX_HELD` or has not taken `RX_IN_FLIGHT` queued ones.
        let mut again = true;
        let mut served_tx = false;
        let received = &held.storage.shared.received;
        for _ in 0..EVENTS_PER_POLL {
            let room = RX_HELD
                .saturating_sub(held.rx_out.get())
                .min(received.room(RX_IN_FLIGHT));
            match device.poll_receive(room, cx) {
                Poll::Ready(Ok(DeviceEvent::Tx(wake))) => {
                    self.service_tx(held, device, wake, stopping)?;
                    served_tx = true;
                }
                Poll::Ready(Ok(DeviceEvent::Receive(unit))) => {
                    self.receive(held, device, unit)?;
                }
                Poll::Ready(Ok(DeviceEvent::Tbtt)) => {
                    let mut sink = Sink::default();
                    if !core_views::configuring::<Id, D, B>(self.core.view()) {
                        self.core
                            .view()
                            .commands()
                            .ok_or(Fault::Capacity)?
                            .station_tbtt(device.hardware()?, &mut sink);
                    }
                    self.collect(held, &mut sink, stopping)?;
                }
                Poll::Ready(Err(cause)) => return Err(cause),
                Poll::Pending => {
                    again = false;
                    break;
                }
            }
        }
        // Lends and returns wake the driver only when it asks: out of
        // receive room, or with many units in the network, where the radio
        // may run short. Otherwise the next interrupt's poll counts them.
        let network = &held.storage.shared.network;
        let starved = held.rx_out.get() >= RX_HELD && network.want_room();
        let short = network.held() >= D::LENT_RX / 2 && network.want_units(cx);
        // P1's next take wakes a driver its queued units hold back; one it
        // took meanwhile leaves room now.
        let mut waiting = false;
        if received.room(RX_IN_FLIGHT) == 0 && held.rx_out.get() < RX_HELD {
            received.want();
            waiting = received.room(RX_IN_FLIGHT) != 0;
        }
        if again || served_tx || starved || short || waiting {
            cx.waker().wake_by_ref();
        }
        // A configuration barrier waits for its role's published aggregates
        // as the core waits for its MPDUs.
        let configuring_aggregate = self
            .configuring
            .as_ref()
            .is_some_and(|permission| self.aggregate_running(Some(permission.role())));
        if self.configuring.is_some() && !configuring_aggregate {
            if core_views::configuring::<Id, D, B>(self.core.view()) {
                let mut sink = Sink::default();
                let done = core_views::advance_configuration::<Id, D, B>(
                    self.core.view(),
                    device.hardware()?,
                    &mut sink,
                );
                self.collect(held, &mut sink, stopping)?;
                if !done {
                    return Ok(());
                }
            }
            if self.withdraw(held) {
                let permission = self
                    .configuring
                    .take()
                    .expect("configuration retains its permission");
                held.terminal(owned::OwnedTerminal::Configured(permission));
            }
        }
        if held.dirty_edca.replace(false) {
            for role in [VifRole::Station, VifRole::AccessPoint] {
                let Some(table) = held.edca.get(vif(role)) else {
                    // P1 is publishing it: its commit wake serves it again.
                    held.dirty_edca.set(true);
                    break;
                };
                let _installed = idle_command!(self.core, |view| view.set_checked_edca(
                    role,
                    SupportedEdca::<Esp32s31Caps<B>>::new(table).ok_or(Fault::Capacity)?,
                ))
                .map_err(|_| Fault::Capacity)?;
                #[cfg(test)]
                held.storage.installed_edca[vif(role)].set(Some(_installed.get()));
            }
        }
        if let Some(color) = held.color.take() {
            idle_command!(self.core, |view| view.set_checked_he_bss_color(color));
        }
        if held.dirty_ba.replace(false) {
            for (index, old) in self.applied_ba.iter_mut().enumerate() {
                let wanted = match held.block_acks.get(index) {
                    Ok(wanted) => wanted,
                    // P1 is publishing it: its commit wake serves it again.
                    Err(None) => {
                        held.dirty_ba.set(true);
                        break;
                    }
                    Err(Some(fault)) => return Err(Fault::Setting(fault)),
                };
                if *old == wanted {
                    continue;
                }
                if let Some(old) = old.take() {
                    idle_command!(self.core, |view| view.end_checked_rx_block_ack(
                        device.hardware()?,
                        old.role,
                        old.peer,
                        old.tid,
                    ))?;
                }
                if let Some(agreement) = wanted {
                    idle_command!(self.core, |view| view
                        .start_checked_rx_block_ack(device.hardware()?, agreement))?;
                }
                *old = wanted;
            }
        }
        // Every command is bounded by the affine capacities; a barrier retains
        // its FIFO position and prevents later publication.
        for _ in 0..COMMANDS {
            if self.configuring.is_some() {
                break;
            }
            // Without a held or an incoming command no command moves: the
            // common turn checks the queue alone.
            if self.pending_command.is_none() {
                if held.commands.empty() {
                    self.withdraw(held);
                    break;
                }
                self.pending_command = held.commands.pop();
            }
            let drained = self.withdraw(held);
            let Some(command) = self.pending_command.take() else {
                break;
            };
            match command {
                Command::Submit(pending) => {
                    let index = queue(pending.category);
                    if self.queues[index]
                        .push_back(Queued::Single(pending))
                        .is_err()
                    {
                        return Err(Fault::Capacity);
                    }
                }
                Command::SubmitAggregate(pending) => {
                    let index = queue(pending.category);
                    if self.queues[index]
                        .push_back(Queued::Aggregate(pending))
                        .is_err()
                    {
                        return Err(Fault::Capacity);
                    }
                }
                Command::Configure { permission, config } => {
                    core_views::configure::<Id, D, B>(self.core.view(), config)?;
                    self.configuring = Some(permission);
                    cx.waker().wake_by_ref();
                    break;
                }
                Command::CloseGate(gate) => {
                    core_views::hold::<Id, D, B>(self.core.view());
                    self.closing = Some(gate);
                }
                Command::OpenGate => {
                    let mut sink = Sink::default();
                    core_views::open::<Id, D, B>(self.core.view(), device.hardware()?, &mut sink)?;
                    self.closed = false;
                    self.collect(held, &mut sink, stopping)?;
                }
                Command::Install {
                    identity,
                    installation,
                    material,
                } => {
                    // FIFO behind a bounded group command, this command
                    // completes within that bound: it waits for nothing.
                    self.install(device, identity, installation, &material)?;
                    identity.activity.set(identity.serial);
                    held.terminal(owned::OwnedTerminal::PairwiseKeyInstalled(
                        owned::OwnedInstalledPairwise::new(identity.role, identity),
                    ));
                }
                Command::Remove { identity, removal } => {
                    if !drained {
                        self.pending_command = Some(Command::Remove { identity, removal });
                        break;
                    }
                    let removed = self.remove(device, identity)?;
                    held.terminal(owned::OwnedTerminal::KeyRemoved(
                        self.removed_terminal(removal, removed)?,
                    ));
                }
                Command::Group { at, command } => {
                    if now.saturating_duration_since(at) >= COMMAND_BOUND {
                        // The expired command stays in quarantine here.
                        #[cfg(test)]
                        let _first = held.storage.shared.retained_group.set(command.owners());
                        self.pending_command = Some(Command::Group { at, command });
                        return Err(Fault::Deadline);
                    }
                    if self
                        .running
                        .iter()
                        .flatten()
                        .any(|running| command.uses(running.key()))
                        || !drained
                    {
                        self.pending_command = Some(Command::Group { at, command });
                        break;
                    }
                    match device.poll_rx_cut(RX_HELD - held.rx_out.get(), cx) {
                        Poll::Ready(Ok(Some(unit))) => {
                            self.receive(held, device, unit)?;
                            self.pending_command = Some(Command::Group { at, command });
                            cx.waker().wake_by_ref();
                            break;
                        }
                        Poll::Pending => {
                            self.pending_command = Some(Command::Group { at, command });
                            break;
                        }
                        Poll::Ready(Err(cause)) => {
                            self.pending_command = Some(Command::Group { at, command });
                            return Err(cause);
                        }
                        Poll::Ready(Ok(None)) => {
                            let terminal = self.group(held, device, at, command)?;
                            if let Err(cause) = device.finish_rx_cut() {
                                handoff::retain_terminal(held.failed_terminal, terminal.into());
                                return Err(cause);
                            }
                            held.terminal(terminal);
                        }
                    }
                }
            }
        }
        self.withdraw(held);
        // A gate closes after the published aggregates end, as after the
        // MPDUs the core holds.
        if self.closing.is_some()
            && !self.aggregate_running(None)
            && !core_views::configuring::<Id, D, B>(self.core.view())
            && core_views::close::<Id, D, B>(self.core.view(), device.hardware()?)
        {
            // The core has returned every published head. Existing AC queues
            // retain their tails in order until those heads are terminal.
            let gate = held
                .gate
                .close(self.closing.take().expect("closing owns gate"));
            self.closed = true;
            held.terminal(owned::OwnedTerminal::GateClosed(gate));
        }
        for index in 0..4 {
            if self.running[index].is_some() {
                continue;
            }
            // Expiry continues behind a gate or a command barrier.
            while self.queues[index]
                .front()
                .is_some_and(|p| Self::expired(held, p, now))
            {
                match self.queues[index].pop_front() {
                    Some(Queued::Single(pending)) => {
                        self.finish(held, pending, TxOutcome::Expired { msdus: 1 });
                    }
                    Some(Queued::Aggregate(pending)) => {
                        if let Err(pending) = self.expire_aggregate(held, pending, now) {
                            // Its oldest admission aged and no subframe did:
                            // the record lost its admission.
                            drop(pending);
                            return Err(Fault::Capacity);
                        }
                    }
                    None => break,
                }
            }
            if stopping
                || self.closed
                || self.closing.is_some()
                || core_views::configuring::<Id, D, B>(self.core.view())
                || self.queues[index].front().is_some_and(|p| self.blocked(p))
            {
                continue;
            }
            #[cfg(oer_debug_control)]
            if held.publication_hold.get() & (1 << index) != 0 {
                continue;
            }
            // An aggregate waits at the head until a chain is free.
            if matches!(
                self.queues.get(index).and_then(|queue| queue.front()),
                Some(Queued::Aggregate(_))
            ) && !self.chain_free()
            {
                continue;
            }
            match self.queues[index].pop_front() {
                Some(Queued::Single(pending)) => self.publish(held, device, index, pending)?,
                Some(Queued::Aggregate(pending)) => {
                    self.publish_aggregate(held, device, index, pending)?
                }
                None => {}
            }
        }
        if stopping
            && !self.aggregate_running(None)
            && self.pending_command.is_none()
            && held.commands.empty()
            && !core_views::configuring::<Id, D, B>(self.core.view())
            && self.configuring.is_none()
            && self.closing.is_none()
        {
            let mut sink = Sink::default();
            core_views::disable::<Id, D, B>(self.core.view(), device.hardware()?, &mut sink);
            self.collect(held, &mut sink, true)?;
            let mut sink = Sink::default();
            let disabled = core_views::finish_disable::<Id, D, B>(self.core.view(), &mut sink);
            self.collect(held, &mut sink, true)?;
            if disabled {
                match device.poll_stop(cx) {
                    Poll::Ready(Ok(())) => {
                        held.enabled.set(false);
                        self.transition = None;
                        held.storage.shared.wake_body();
                    }
                    Poll::Ready(Err(cause)) => return Err(cause),
                    Poll::Pending => {}
                }
            }
        }
        Ok(())
    }
    fn publish(
        &mut self,
        held: &Held<'_, 'id, Id, D, B, O>,
        device: &mut D,
        index: usize,
        mut pending: Pending<'id, Id, D, B, O>,
    ) -> Result<(), Fault> {
        let stage = pending
            .plan
            .stages()
            .nth(pending.stage)
            .ok_or(Fault::Capacity)?;
        let (rate, protection) = (
            stage.rate,
            if pending.publications == 0 {
                pending.protection
            } else {
                stage.protection
            },
        );
        let role = pending.role();
        let CoreView::Open(mut core) = self.core.view() else {
            return Err(Fault::Capacity);
        };
        let observations = self.observations.as_mut().ok_or(Fault::Capacity)?;
        let backoff_slots = core.draw_backoff(role, pending.category, observations);
        let core_ticket = self
            .tickets
            .iter_mut()
            .find_map(Option::take)
            .ok_or(Fault::Capacity)?;
        let key_handle = admitted_key(&self.keys, pending.key())?;
        let admission = match key_handle {
            Some(Installed::Transmit(key)) => core
                .admit_key(key, core_ticket)
                .map(TxAdmission::Protected)
                .map_err(|_| Fault::Capacity)?,
            Some(Installed::Receive(_)) => return Err(Fault::Capacity),
            None => TxAdmission::Open(core_ticket),
        };
        let mpdu = pending.operation.execute().mpdu;
        // The hardware takes the memory through the queue's descriptor; the
        // buffer keeps its return right.
        let lease = mpdu.buffer.dma.publish().ok_or(Fault::Capacity)?;
        let mut slot = self
            .descriptors
            .get_mut(index)
            .and_then(Option::take)
            .ok_or(Fault::Capacity)?;
        if slot.as_mut().attach(lease).is_err() {
            return Err(Fault::Capacity);
        }
        let TxSlotAccess::Free(mut memory) = slot.access() else {
            return Err(Fault::Capacity);
        };
        if !mpdu.published {
            // A new MSDU: the descriptor's exchange record starts over.
            memory.reset_work();
            if let (Some(Installed::Transmit(key)), Some(pn)) = (key_handle, mpdu.pn) {
                let buffer = memory.buffer_mut().map_err(|_| Fault::Capacity)?;
                core.write_ccmp_header(key, buffer, mpdu.layout.header_len(), pn)
                    .map_err(|_| Fault::Capacity)?;
            }
        }
        let values = MpduValues::checked(
            memory,
            core_mac::MpduParameters {
                role,
                admitted_at: mpdu.buffer.admission.ok_or(Fault::Clock)?,
                len: mpdu.layout.mpdu_len(),
                rate,
                protection,
                power_ceiling_dbm: match pending.power {
                    TxPower::Calibrated => None,
                    TxPower::MaxDbm(dbm) => Some(dbm),
                },
                backoff_slots,
                no_ack: mpdu.layout.kind() == MpduKind::IndividualNoAck,
                operation: pending.coex,
                retry: mpdu.published,
            },
        )
        .map_err(|_| Fault::Capacity)?;
        mpdu.published = true;
        pending.publications = pending.publications.checked_add(1).ok_or(Fault::Capacity)?;
        #[cfg(all(oer_trace, oer_debug_control))]
        let publication = super::trace::Publication {
            ticket: pending.operation.ticket().index(),
            buffer: pending.operation.execute().mpdu.buffer.index(),
        };
        let queue = self.free[index].take().ok_or(Fault::Capacity)?;
        // Durable backing owns the indivisible operation before publication.
        self.running[index] = Some(Running::Single(pending));
        if let Err(refused) = core.submit_checked(device.hardware()?, queue, admission, values) {
            core::mem::forget(refused);
            return Err(Fault::Capacity);
        }
        #[cfg(all(oer_trace, oer_debug_control))]
        oer_espressif_staged_runtime::trace::emit(&publication);
        let _ = held;
        Ok(())
    }
    fn install(
        &mut self,
        device: &mut D,
        identity: RawKeyIdentity,
        installation: Installation<'id, Id>,
        material: &Ccmp128Key,
    ) -> Result<(), Fault> {
        let installed = match installation {
            Installation::Transmit { spp, scope, slot } => Installed::Transmit(
                idle_command!(self.core, |view| view.install_key_parts(
                    device.hardware()?,
                    spp,
                    scope,
                    slot,
                    material.as_bytes()
                ))
                .map_err(|refused| {
                    let fault = refused.fault;
                    core::mem::forget(refused);
                    Fault::Core(fault)
                })?,
            ),
            Installation::Receive { spp, slot, key_id } => Installed::Receive(
                idle_command!(self.core, |view| view.install_station_group_parts(
                    device.hardware()?,
                    spp,
                    slot,
                    key_id,
                    material.as_bytes(),
                ))
                .map_err(|refused| {
                    let fault = refused.fault;
                    core::mem::forget(refused);
                    Fault::Core(fault)
                })?,
            ),
        };
        let place = self
            .keys
            .iter_mut()
            .find(|k| k.is_none())
            .ok_or(Fault::Capacity)?;
        *place = Some((identity, installed));
        Ok(())
    }
    fn remove(
        &mut self,
        device: &mut D,
        identity: RawKeyIdentity,
    ) -> Result<KeyRemoved<'id, CorePath<Id>>, Fault> {
        let place = self
            .keys
            .iter_mut()
            .find(|entry| entry.as_ref().is_some_and(|(key, _)| *key == identity))
            .ok_or(Fault::Capacity)?;
        let (_, installed) = place.take().ok_or(Fault::Capacity)?;
        let mut sink = Sink::default();
        match installed {
            Installed::Transmit(key) => {
                idle_command!(self.core, |view| view.remove_checked_key(
                    device.hardware()?,
                    key,
                    &mut sink
                ))
            }
            Installed::Receive(key) => {
                idle_command!(self.core, |view| view.remove_checked_key(
                    device.hardware()?,
                    key,
                    &mut sink
                ))
            }
        }
        if let Some(fault) = sink.fault() {
            return Err(fault);
        }
        sink.removed.pop().ok_or(Fault::Capacity)
    }
    fn removed_terminal(
        &self,
        removal: Removal<'id, Id>,
        removed: KeyRemoved<'id, CorePath<Id>>,
    ) -> Result<owned::OwnedRemoved<'id, Id>, Fault> {
        use core_mac::association::VacantKeySlot as Slot;
        let removed = match (removal, removed.into_slot()) {
            (Removal::StationPairwise(association), Slot::StationPairwise(slot)) => {
                owned::OwnedRemoved::Station(owned::RemovedOwners::Pairwise { slot, association })
            }
            (Removal::AccessPointPairwise(association), Slot::AccessPointPairwise(slot)) => {
                owned::OwnedRemoved::AccessPoint(owned::RemovedOwners::Pairwise {
                    slot,
                    association,
                })
            }
            (Removal::StationGroup(permission), Slot::StationGroup(slot)) => {
                owned::OwnedRemoved::Station(owned::RemovedOwners::Group { slot, permission })
            }
            (Removal::AccessPointGroup(permission), Slot::AccessPointGroup(slot)) => {
                owned::OwnedRemoved::AccessPoint(owned::RemovedOwners::Group { slot, permission })
            }
            _ => return Err(Fault::Capacity),
        };
        Ok(removed)
    }
    fn group(
        &mut self,
        held: &Held<'_, 'id, Id, D, B, O>,
        device: &mut D,
        at: Instant,
        command: GroupCommand<'id, Id>,
    ) -> Result<owned::OwnedTerminal<'id, Id, D, B>, Fault> {
        match command {
            GroupCommand::InstallStation {
                spp,
                slot,
                permission,
                key_id,
                material,
            } => {
                let identity = RawKeyIdentity {
                    role: VifRole::Station,
                    slot: slot.index(),
                    serial: held.serial(),
                    key_id,
                    activity: &held.storage.shared.key_live[1],
                };
                self.install(
                    device,
                    identity,
                    Installation::Receive { spp, slot, key_id },
                    &material,
                )?;
                identity.activity.set(identity.serial);
                return Ok(owned::OwnedTerminal::GroupKeyInstalled(
                    owned::OwnedInstalledGroup::Station {
                        key: GroupKey {
                            identity,
                            brand: PhantomData,
                        },
                        permission,
                    },
                ));
            }
            GroupCommand::InstallAccessPoint {
                spp,
                slot,
                permission,
                key_id,
                material,
            } => {
                let identity = RawKeyIdentity {
                    role: VifRole::AccessPoint,
                    slot: slot.index(),
                    serial: held.serial(),
                    key_id,
                    activity: &held.storage.shared.key_live[17 + usize::from(slot.index())],
                };
                let Some(key_id) = crate::mac::crypto::ApGroupKeyId::new(key_id.get()) else {
                    return Err(Fault::Capacity);
                };
                self.install(
                    device,
                    identity,
                    Installation::Transmit {
                        spp,
                        scope: TransmitKeyScope::AccessPointGroup { key_id },
                        slot: core_mac::association::VacantKeySlot::AccessPointGroup(slot),
                    },
                    &material,
                )?;
                identity.activity.set(identity.serial);
                return Ok(owned::OwnedTerminal::GroupKeyInstalled(
                    owned::OwnedInstalledGroup::AccessPoint {
                        key: GroupKey {
                            identity,
                            brand: PhantomData,
                        },
                        permission,
                    },
                ));
            }
            GroupCommand::Replace {
                permission,
                key,
                key_id,
                material,
            } => {
                let entry = self
                    .keys
                    .iter_mut()
                    .flatten()
                    .find(|(identity, _)| *identity == key.identity)
                    .ok_or(Fault::Capacity)?;
                let Installed::Receive(installed) = &mut entry.1 else {
                    return Err(Fault::Capacity);
                };
                #[cfg(oer_debug_control)]
                let mut hardware = key_replacement::RejectOnce {
                    hardware: device.hardware()?,
                    reject: held.reject_group_replacement.replace(false),
                };
                #[cfg(oer_debug_control)]
                let hardware = &mut hardware;
                #[cfg(not(oer_debug_control))]
                let hardware = device.hardware()?;
                let result = idle_command!(self.core, |view| view.replace_station_group_key(
                    hardware,
                    installed,
                    key_id,
                    material.as_bytes(),
                ));
                if let Err(_error) = result {
                    self.pending_command = Some(Command::Group {
                        at,
                        command: GroupCommand::Replace {
                            permission,
                            key,
                            key_id,
                            material,
                        },
                    });
                    // Observe actual quarantined owners after moving them to
                    // permanent custody, never a synthetic success terminal.
                    #[cfg(test)]
                    if let Some(Command::Group {
                        command:
                            GroupCommand::Replace {
                                permission,
                                key,
                                key_id,
                                ..
                            },
                        ..
                    }) = &self.pending_command
                    {
                        let shared = &held.storage.shared;
                        let _first = shared
                            .retained_group
                            .set((key.identity.slot, permission.index()));
                        let _first = shared.rolled_back.set((
                            key.identity.key_id.get(),
                            key_id.get(),
                            matches!(
                                _error,
                                crate::mac::crypto::StaGroupCcmpReplaceError::ReplacementRolledBack(
                                    _
                                )
                            ),
                        ));
                    }
                    #[cfg(all(oer_trace, oer_debug_control))]
                    if let Some(Command::Group {
                        command:
                            GroupCommand::Replace {
                                permission,
                                key,
                                key_id,
                                ..
                            },
                        ..
                    }) = &self.pending_command
                    {
                        oer_espressif_staged_runtime::trace::emit(&trace::GroupRollback {
                            owners: [key.identity.slot, permission.index()],
                            previous_key_id: key.identity.key_id.get(),
                            attempted_key_id: key_id.get(),
                            restored: matches!(
                                _error,
                                crate::mac::crypto::StaGroupCcmpReplaceError::ReplacementRolledBack(
                                    _
                                )
                            ),
                        });
                    }
                    return Err(Fault::Core(core_mac::LowerMacFault::Key(
                        crate::mac::crypto::CryptoKeyError::HardwareRejected,
                    )));
                }
                let identity = RawKeyIdentity {
                    serial: held.serial(),
                    key_id,
                    ..key.identity
                };
                entry.0 = identity;
                identity.activity.set(identity.serial);
                return Ok(owned::OwnedTerminal::GroupKeyReplaced {
                    key: GroupKey {
                        identity,
                        brand: PhantomData,
                    },
                    permission,
                });
            }
            GroupCommand::Select { permission, key } => {
                if !self
                    .keys
                    .iter()
                    .flatten()
                    .any(|(identity, _)| *identity == key.identity)
                {
                    return Err(Fault::Capacity);
                }
                return Ok(owned::OwnedTerminal::GroupKeySelected { key, permission });
            }
            GroupCommand::Remove { identity, removal } => {
                let removed = self.remove(device, identity)?;
                return Ok(owned::OwnedTerminal::KeyRemoved(
                    self.removed_terminal(removal, removed)?,
                ));
            }
        }
    }
}
fn queue(category: WmmAccessCategory) -> usize {
    usize::from(LegacyTxQueue::from_access_category(category).hardware_index())
}
fn vif(role: VifRole) -> usize {
    if role == VifRole::Station { 0 } else { 1 }
}
impl<Id: SlotIdentity> GroupCommand<'_, Id> {
    fn uses(&self, key: Option<RawKeyIdentity>) -> bool {
        key.is_some_and(|key| match self {
            Self::Remove { identity, .. } => *identity == key,
            Self::Replace { key: installed, .. } | Self::Select { key: installed, .. } => {
                installed.identity == key
            }
            _ => false,
        })
    }
}

/// The port asked for the radio and the device has not started it yet.
fn starting<Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations>(
    held: &Held<'_, '_, Id, D, B, O>,
) -> bool {
    held.want_enabled.get() && !held.enabled.get()
}
