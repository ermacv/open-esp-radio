use super::*;
use super::{
    keys::{GroupCommand, Installation, Installed, RawKeyIdentity, Removal},
    lifecycle::CoreOwner,
};
use oer_ieee80211_wire::qos::WmmAccessCategory;

pub(super) struct Pending<'id, Id: SlotIdentity, D: Device<B>, const B: usize> {
    pub operation: Single<'id, Id, D, B>,
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
impl<Id: SlotIdentity, D: Device<B>, const B: usize> Pending<'_, Id, D, B> {
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
pub(super) enum Command<'id, Id: SlotIdentity, D: Device<B>, const B: usize> {
    Submit(Pending<'id, Id, D, B>),
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
    removed: heapless::Vec<KeyRemoved<'id, CorePath<Id>>, 20>,
    tbtt: Option<TbttEvent>,
}
impl<Id: SlotIdentity, const B: usize> Default for Sink<'_, Id, B> {
    fn default() -> Self {
        Self {
            completed: heapless::Vec::new(),
            removed: heapless::Vec::new(),
            tbtt: None,
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
        if self
            .completed
            .push(Completion {
                queue,
                ticket,
                slot,
                end,
                rate,
            })
            .is_err()
        {
            panic!("one completion per hardware queue per service")
        }
    }
    fn ampdu_completed(
        &mut self,
        _: FreeQueue<'id, CorePath<Id>>,
        _: TxTicket<'id, CorePath<Id>>,
        _: PublicationEnd,
    ) {
        unreachable!("ordinary port never publishes an aggregate")
    }
    fn key_removed(&mut self, removed: KeyRemoved<'id, CorePath<Id>>) {
        if self.removed.push(removed).is_err() {
            panic!("one removal per installed hardware key")
        }
    }
    fn disabled(&mut self) {}
    fn tbtt(&mut self, event: TbttEvent) {
        self.tbtt = Some(event);
    }
}

pub(super) struct Driver<'id, Id: SlotIdentity, D: Device<B>, const B: usize> {
    core: CoreOwner<'id, Id, D, B>,
    tickets: [Option<TxTicket<'id, CorePath<Id>>>; TX_TICKETS],
    free: [Option<FreeQueue<'id, CorePath<Id>>>; 4],
    queues: [heapless::Deque<Pending<'id, Id, D, B>, TX_TICKETS>; 4],
    running: [Option<Pending<'id, Id, D, B>>; 4],
    pending_command: Option<Command<'id, Id, D, B>>,
    configuring: Option<owned::OwnedConfigured<'id, Id, D, B>>,
    closing: Option<GateOpen<'id, Part<Id, 1>>>,
    closed: bool,
    keys: [Option<(RawKeyIdentity, Installed<'id, Id>)>; 20],
    applied_ba: [Option<RxBlockAckValues>; 8],
    transition: Option<Instant>,
    clock_deadline: Instant,
}
impl<'id, Id: SlotIdentity, D: Device<B>, const B: usize> Driver<'id, Id, D, B> {
    pub(super) fn take_transmitter(
        &mut self,
    ) -> OrdinaryTxIdleOwner<'static, D::Profile, D::Power, D::Entropy, D::Timer, B> {
        self.core.take_transmitter()
    }
    pub(super) const fn vacant() -> Self {
        Self {
            core: CoreOwner::vacant(),
            tickets: [const { None }; TX_TICKETS],
            free: [const { None }; 4],
            queues: [const { heapless::Deque::new() }; 4],
            running: [const { None }; 4],
            pending_command: None,
            configuring: None,
            closing: None,
            closed: false,
            keys: [const { None }; 20],
            applied_ba: [None; 8],
            transition: None,
            clock_deadline: Instant::from_micros(0),
        }
    }
    #[inline(never)]
    pub(super) fn prepare(
        &mut self,
        core: DisabledCore<Core<'id, Id, D, B>>,
        tickets: [TxTicket<'id, CorePath<Id>>; TX_TICKETS],
        queues: [FreeQueue<'id, CorePath<Id>>; 4],
        now: Instant,
    ) {
        assert!(
            self.queues.iter().all(|q| q.is_empty()) && self.running.iter().all(Option::is_none),
            "retired driver has no admitted work"
        );
        self.core.prepare(core);
        self.tickets = tickets.map(Some);
        self.free = queues.map(Some);
        self.clock_deadline = now.saturating_add(Duration::from_secs(60));
    }
    pub(super) fn retire(
        &mut self,
    ) -> OrdinaryTxIdleOwner<'static, D::Profile, D::Power, D::Entropy, D::Timer, B> {
        let transmitter = self.take_transmitter();
        for queue in &mut self.queues {
            queue.clear();
        }
        for owner in &mut self.running {
            drop(owner.take());
        }
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
        transmitter
    }
    pub(super) fn deadline(&mut self, held: &Held<'id, Id, D, B>) -> Instant {
        let mut deadline = self
            .core
            .deadline()
            .unwrap_or(self.clock_deadline)
            .min(self.clock_deadline);
        if let Some(device) = held.storage.device.borrow().next_deadline() {
            deadline = deadline.min(device);
        }
        if let Some(start) = self.transition {
            deadline = deadline.min(start.saturating_add(COMMAND_BOUND));
        }
        if let Some(Command::Group { at, .. }) = self.pending_command.as_ref() {
            deadline = deadline.min(at.saturating_add(COMMAND_BOUND));
        }
        if !self.core.configuring() {
            for pending in self.queues.iter().flat_map(|q| q.iter()) {
                if let Some((_, at)) = pending.operation.payload().buffer.admission {
                    let lifetime = held.edca.get()[vif(pending.role())]
                        .access_category(pending.category)
                        .msdu_lifetime;
                    deadline = deadline.min(at.saturating_add(lifetime));
                }
            }
        }
        deadline
    }
    fn finish(
        &mut self,
        held: &Held<'id, Id, D, B>,
        pending: Pending<'id, Id, D, B>,
        outcome: SingleOutcome<Ack, Infallible>,
    ) {
        if pending.publications != 0 {
            self.core.reset_contention(pending.role(), pending.category);
        }
        if let TxOutcome::Expired { msdus } = outcome {
            held.storage.expired.set(
                held.storage
                    .expired
                    .get()
                    .saturating_add(msdus.count_ones()),
            );
        }
        held.terminal(owned::OwnedTerminal::TxCompleted(
            owned::OwnedTxCompleted::Single(pending.operation.complete(outcome)),
        ));
    }
    // The retained barrier is also the withdrawal marker; no owner or result
    // is moved out of its AC while an earlier owner can still complete.
    fn withdrawal(&self, pending: &Pending<'id, Id, D, B>) -> Option<WithdrawCause> {
        if self
            .configuring
            .as_ref()
            .is_some_and(|p| p.role() == pending.role())
        {
            return Some(WithdrawCause::Disabled);
        }
        match &self.pending_command {
            Some(Command::Remove { identity, .. })
            | Some(Command::Group {
                command: GroupCommand::Remove { identity, .. },
                ..
            }) if pending.key() == Some(*identity) => Some(WithdrawCause::KeyRemoved),
            _ => None,
        }
    }
    #[inline(never)]
    fn withdraw(&mut self, held: &Held<'id, Id, D, B>) -> bool {
        let global = if !held.want_enabled.get() {
            Some(WithdrawCause::Disabled)
        } else if self.closing.is_some() {
            Some(WithdrawCause::GateClosed)
        } else {
            None
        };
        for index in 0..4 {
            if self.running[index].is_some() {
                continue;
            }
            // A selective barrier cannot return an owner past either a
            // published head or an earlier unaffected queued owner.
            while let Some(cause) = self.queues[index]
                .front()
                .and_then(|pending| global.or_else(|| self.withdrawal(pending)))
            {
                let pending = self.queues[index]
                    .pop_front()
                    .expect("matched head retained");
                self.finish(held, pending, TxOutcome::Withdrawn(cause));
            }
        }
        !self
            .queues
            .iter()
            .flat_map(|queue| queue.iter())
            .chain(self.running.iter().flatten())
            .any(|pending| self.withdrawal(pending).is_some())
    }
    #[inline(always)]
    fn blocked(&self, pending: &Pending<'id, Id, D, B>) -> bool {
        self.withdrawal(pending).is_some()
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
    fn expired(held: &Held<'id, Id, D, B>, pending: &Pending<'id, Id, D, B>, now: Instant) -> bool {
        pending
            .operation
            .payload()
            .buffer
            .admission
            .is_some_and(|(_, at)| {
                now.saturating_duration_since(at)
                    >= held.edca.get()[vif(pending.role())]
                        .access_category(pending.category)
                        .msdu_lifetime
            })
    }
    fn collect(
        &mut self,
        held: &Held<'id, Id, D, B>,
        mut sink: Sink<'id, Id, B>,
        stopping: bool,
    ) -> Result<(), Fault> {
        for completion in sink.completed.drain(..) {
            let index = queue(completion.queue.category());
            let Some(mut pending) = self.running[index].take() else {
                return Err(Fault::Capacity);
            };
            self.free[index] = Some(completion.queue);
            let ticket_index = completion.ticket.index();
            self.tickets[ticket_index] = Some(completion.ticket);
            let slot = completion.slot;
            let TxSlotAccess::Free(memory) = slot.access() else {
                return Err(Fault::Capacity);
            };
            pending.operation.execute().mpdu.buffer.dma.memory = Some(memory);
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
                        MpduKind::Individual => SingleResponse::Ack(Ack { snr_db: ack_snr_db }),
                        MpduKind::BlockAckRequest => SingleResponse::BlockAck {
                            report: block_ack.ok_or(Fault::Capacity)?,
                            ack: Ack { snr_db: ack_snr_db },
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
            } else if self.queues[index].push_front(pending).is_err() {
                return Err(Fault::Capacity);
            }
        }
        if let Some(tbtt) = sink.tbtt {
            if held.events.push(owned::OwnedEvent::Tbtt(tbtt)).is_err() {
                held.loss.set(true);
            }
            held.storage.body.wake();
        }
        if !sink.removed.is_empty() {
            return Err(Fault::Capacity);
        }
        self.withdraw(held);
        Ok(())
    }
    fn receive(
        &mut self,
        held: &Held<'id, Id, D, B>,
        device: &mut D,
        unit: D::Receive,
    ) -> Result<(), Fault> {
        if held.rx_out.get() >= TX_TICKETS {
            core::mem::forget(unit);
            return Err(Fault::Capacity);
        }
        let accepted = {
            match unit.normalized() {
                Ok(frame) => self
                    .core
                    .received(device.hardware(), &frame)
                    .map(|(bytes, meta)| {
                        (
                            bytes
                                .as_ptr()
                                .addr()
                                .checked_sub(unit.bytes().as_ptr().addr()),
                            bytes.len(),
                            meta,
                        )
                    }),
                Err(_) => None,
            }
        };
        if let Some((Some(start), len, meta)) = accepted {
            let end = start
                .checked_add(len)
                .filter(|end| *end <= unit.bytes().len())
                .ok_or(Fault::Capacity)?;
            held.rx_out.set(held.rx_out.get() + 1);
            if let Err(event) = held.events.push(owned::OwnedEvent::Received {
                frame: ReceiveBuffer {
                    brand: PhantomData,
                    unit: Some(unit),
                    range: start..end,
                    poison: &held.storage.poisoned,
                    driver: &held.storage.driver,
                },
                meta,
            }) {
                core::mem::forget(event);
                return Err(Fault::Capacity);
            }
            held.storage.body.wake();
        } else {
            device.recycle(unit);
        }
        Ok(())
    }
    pub(super) fn poll(
        &mut self,
        cx: &mut Context<'_>,
        held: &Held<'id, Id, D, B>,
        device: &mut D,
    ) -> Result<(), Fault> {
        while let Some(mut frame) = held.recycled.pop() {
            device.recycle(
                frame
                    .unit
                    .take()
                    .expect("recycle consumes the original RX unit"),
            );
            held.rx_out
                .set(held.rx_out.get().checked_sub(1).ok_or(Fault::Capacity)?);
        }
        let now = device.timer().now();
        if now >= self.clock_deadline {
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
            let channel = held.channel.get().wifi_channel().ok_or(Fault::Capacity)?;
            match device.poll_start(cx, channel) {
                Poll::Pending => return Ok(()),
                Poll::Ready(Err(cause)) => return Err(cause),
                Poll::Ready(Ok(())) => {
                    if let Some(power) = device.take_power_profile() {
                        self.core.core().replace_power_profile(power);
                    }
                    self.core.tune(channel);
                    held.enabled.set(true);
                    self.transition = None;
                    held.storage.body.wake();
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
            let mut sink = Sink::default();
            self.core
                .service(device.hardware(), WifiTxWake::Deadline, &mut sink)?;
            self.collect(held, sink, stopping)?;
        }
        for _ in 0..4 {
            match device.poll_receive(TX_TICKETS - held.rx_out.get(), cx) {
                Poll::Ready(Ok(DeviceEvent::Tx(wake))) => {
                    let mut sink = Sink::default();
                    self.core.service(device.hardware(), wake, &mut sink)?;
                    self.collect(held, sink, stopping)?;
                    cx.waker().wake_by_ref();
                }
                Poll::Ready(Ok(DeviceEvent::Receive(unit))) => {
                    self.receive(held, device, unit)?;
                    cx.waker().wake_by_ref();
                }
                Poll::Ready(Ok(DeviceEvent::Tbtt)) => {
                    let mut sink = Sink::default();
                    if !self.core.configuring() {
                        self.core.core().station_tbtt(device.hardware(), &mut sink);
                    }
                    self.collect(held, sink, stopping)?;
                }
                Poll::Ready(Err(cause)) => return Err(cause),
                Poll::Pending => break,
            }
        }
        if self.configuring.is_some() {
            if self.core.configuring() {
                let mut sink = Sink::default();
                let done = self
                    .core
                    .advance_configuration(device.hardware(), &mut sink);
                self.collect(held, sink, stopping)?;
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
                self.core
                    .core()
                    .set_checked_edca(
                        role,
                        SupportedEdca::<Esp32s31Caps<B>>::new(held.edca.get()[vif(role)])
                            .ok_or(Fault::Capacity)?,
                    )
                    .map_err(|_| Fault::Capacity)?;
            }
        }
        if let Some(color) = held.color.take() {
            self.core.core().set_checked_he_bss_color(color);
        }
        if held.dirty_ba.replace(false) {
            let desired = *held.block_acks.borrow();
            for (old, wanted) in self.applied_ba.iter_mut().zip(desired) {
                if *old == wanted {
                    continue;
                }
                if let Some(old) = old.take() {
                    self.core.core().end_checked_rx_block_ack(
                        device.hardware(),
                        old.role,
                        old.peer,
                        old.tid,
                    )?;
                }
                if let Some(agreement) = wanted {
                    self.core
                        .core()
                        .start_checked_rx_block_ack(device.hardware(), agreement)?;
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
            self.pending_command = self.pending_command.take().or_else(|| held.commands.pop());
            let drained = self.withdraw(held);
            let Some(command) = self.pending_command.take() else {
                break;
            };
            match command {
                Command::Submit(pending) => {
                    let index = queue(pending.category);
                    if self.queues[index].push_back(pending).is_err() {
                        return Err(Fault::Capacity);
                    }
                }
                Command::Configure { permission, config } => {
                    self.core.configure(config)?;
                    self.configuring = Some(permission);
                    cx.waker().wake_by_ref();
                    break;
                }
                Command::CloseGate(gate) => {
                    self.core.hold();
                    self.closing = Some(gate);
                }
                Command::OpenGate => {
                    let mut sink = Sink::default();
                    self.core.open(device.hardware(), &mut sink)?;
                    self.closed = false;
                    self.collect(held, sink, stopping)?;
                }
                Command::Install {
                    identity,
                    installation,
                    material,
                } => {
                    self.install(device, identity, installation, &material)?;
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
                        self.pending_command = Some(Command::Group { at, command });
                        return Err(Fault::Deadline);
                    }
                    if self.running.iter().flatten().any(|p| command.uses(p.key())) || !drained {
                        self.pending_command = Some(Command::Group { at, command });
                        break;
                    }
                    match device.poll_rx_cut(TX_TICKETS - held.rx_out.get(), cx) {
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
                            let terminal = self.group(held, device, command)?;
                            if let Err(cause) = device.finish_rx_cut() {
                                *held.failed_terminal.borrow_mut() = Some(terminal);
                                return Err(cause);
                            }
                            held.terminal(terminal);
                        }
                    }
                }
            }
        }
        self.withdraw(held);
        if self.closing.is_some() && !self.core.configuring() && self.core.close(device.hardware())
        {
            // The core has returned every published head. Existing AC queues
            // retain their tails in order until those heads are terminal.
            let gate = held
                .gate
                .borrow_mut()
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
                let pending = self.queues[index]
                    .pop_front()
                    .expect("expired head retained");
                self.finish(held, pending, TxOutcome::Expired { msdus: 1 });
            }
            if stopping
                || self.closed
                || self.closing.is_some()
                || self.core.configuring()
                || self.queues[index].front().is_some_and(|p| self.blocked(p))
            {
                continue;
            }
            #[cfg(oer_debug_control)]
            if held.publication_hold.get() & (1 << index) != 0 {
                continue;
            }
            if let Some(pending) = self.queues[index].pop_front() {
                self.publish(held, device, index, pending)?;
            }
        }
        if stopping
            && self.pending_command.is_none()
            && held.commands.empty()
            && !self.core.configuring()
            && self.configuring.is_none()
            && self.closing.is_none()
        {
            let mut sink = Sink::default();
            self.core.disable(device.hardware(), &mut sink);
            self.collect(held, sink, true)?;
            let mut sink = Sink::default();
            let disabled = self.core.finish_disable(&mut sink);
            self.collect(held, sink, true)?;
            if disabled {
                match device.poll_stop(cx) {
                    Poll::Ready(Ok(())) => {
                        held.enabled.set(false);
                        self.transition = None;
                        held.storage.body.wake();
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
        held: &Held<'id, Id, D, B>,
        device: &mut D,
        index: usize,
        mut pending: Pending<'id, Id, D, B>,
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
        let core = self.core.core();
        let backoff_slots = core.draw_backoff(role, pending.category);
        let core_ticket = self
            .tickets
            .iter_mut()
            .find_map(Option::take)
            .ok_or(Fault::Capacity)?;
        let key = pending.key();
        let key_handle = key
            .map(|identity| {
                self.keys
                    .iter()
                    .flatten()
                    .find(|(key, _)| *key == identity)
                    .map(|(_, key)| key)
            })
            .transpose_option()
            .ok_or(Fault::Capacity)?;
        let admission = match key_handle {
            Some(Installed::Transmit(key)) => core
                .admit_key(key, core_ticket)
                .map(TxAdmission::Protected)
                .map_err(|_| Fault::Capacity)?,
            Some(Installed::Receive(_)) => return Err(Fault::Capacity),
            None => TxAdmission::Open(core_ticket),
        };
        let mpdu = pending.operation.execute().mpdu;
        let mut memory = mpdu
            .buffer
            .dma
            .memory
            .take()
            .expect("queued MPDU owns free DMA");
        if !mpdu.published {
            if let (Some(Installed::Transmit(key)), Some(pn)) = (key_handle, mpdu.pn) {
                core.write_ccmp_header(key, &mut memory, mpdu.layout.header_len(), pn)
                    .map_err(|_| Fault::Capacity)?;
            }
        }
        let values = MpduValues::checked(
            memory,
            core_mac::MpduParameters {
                role,
                admitted_at: mpdu.buffer.admission.ok_or(Fault::Clock)?.1,
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
        self.running[index] = Some(pending);
        if let Err(refused) = self
            .core
            .submit(device.hardware(), queue, admission, values)
        {
            core::mem::forget(refused);
            return Err(Fault::Capacity);
        }
        #[cfg(all(oer_trace, oer_debug_control))]
        oer_trace::emit(&publication);
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
                self.core
                    .core()
                    .install_key_parts(device.hardware(), spp, scope, slot, material.as_bytes())
                    .map_err(|refused| {
                        let fault = refused.fault;
                        core::mem::forget(refused);
                        Fault::Core(fault)
                    })?,
            ),
            Installation::Receive { spp, slot, key_id } => Installed::Receive(
                self.core
                    .core()
                    .install_station_group_parts(
                        device.hardware(),
                        spp,
                        slot,
                        key_id,
                        material.as_bytes(),
                    )
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
                self.core
                    .core()
                    .remove_checked_key(device.hardware(), key, &mut sink)
            }
            Installed::Receive(key) => {
                self.core
                    .core()
                    .remove_checked_key(device.hardware(), key, &mut sink)
            }
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
        held: &Held<'id, Id, D, B>,
        device: &mut D,
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
                    spp,
                    key_id,
                    activity: &held.storage.key_live[1],
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
                    spp,
                    key_id,
                    activity: &held.storage.key_live[17 + usize::from(slot.index())],
                };
                let Some(key_id) =
                    oer_esp32s31_ieee80211_mac::crypto::ApGroupKeyId::new(key_id.get())
                else {
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
                let removed = self.remove(device, key.identity)?;
                let core_mac::association::VacantKeySlot::StationGroup(slot) = removed.into_slot()
                else {
                    return Err(Fault::Capacity);
                };
                let identity = RawKeyIdentity {
                    serial: held.serial(),
                    key_id,
                    ..key.identity
                };
                self.install(
                    device,
                    identity,
                    Installation::Receive {
                        spp: identity.spp,
                        slot,
                        key_id,
                    },
                    &material,
                )?;
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

trait TransposeOption<T> {
    fn transpose_option(self) -> Option<Option<T>>;
}
impl<T> TransposeOption<T> for Option<Option<T>> {
    fn transpose_option(self) -> Option<Option<T>> {
        match self {
            None => Some(None),
            Some(Some(value)) => Some(Some(value)),
            Some(None) => None,
        }
    }
}
