use super::*;
use super::{
    keys::{GroupCommand, Installation, Installed, RawKeyIdentity, Removal},
    lifecycle::PortCoreOwner,
};
use oer_protocols_ieee80211_wire::qos::WmmAccessCategory;

pub(super) mod arena;
pub(super) use arena::DataArenas;
pub use arena::StationArena;
pub(super) mod core_views;
#[cfg(oer_debug_control)]
mod key_replacement;
// A single phase projection lends the shared command set. The operation
// itself has one body; invalid body commands remain typed capacity refusals.
macro_rules! idle_command {
    ($owner:expr, |$view:ident| $command:expr) => {{
        let mut $view = $owner
            .view()
            .commands()
            .ok_or(Fault::CoreViewUnavailable(CoreOp::Command))?;
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
    /// When the body admitted its MPDU.
    fn admission(&self) -> Option<Instant> {
        self.operation.payload().buffer.admission
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
pub(super) struct Completion<'id, Id: SlotIdentity, const B: usize> {
    queue: FreeQueue<'id, CorePath<Id>>,
    ticket: TxTicket<'id, CorePath<Id>>,
    slot: Pin<&'static mut TxSlot<B>>,
    end: PublicationEnd,
    rate: PhyRate,
}
pub(super) struct Sink<'id, Id: SlotIdentity, const B: usize> {
    pub(super) completed: heapless::Vec<Completion<'id, Id, B>, 4>,
    pub(super) removed: heapless::Vec<KeyRemoved<'id, CorePath<Id>>, 20>,
    tbtt: Option<TbttEvent>,
    /// The first callback the port's core contract forbids: a second
    /// completion of a queue in one service or more removals than
    /// installed keys. Its owners are retained here,
    /// never returned to a pool, and the service poisons.
    refused: Option<(Fault, Refused<'id, Id, B>)>,
}
/// Owners of a refused callback, kept until the port's quarantine ends.
#[allow(dead_code, reason = "retained owners are never read, only kept")]
enum Refused<'id, Id: SlotIdentity, const B: usize> {
    Completion(Completion<'id, Id, B>),
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
    pub(super) fn fault(&mut self) -> Option<Fault> {
        let (fault, owners) = self.refused.take()?;
        let _quarantined = core::mem::ManuallyDrop::new(owners);
        Some(fault)
    }
}
impl<Id: SlotIdentity, const B: usize> Default for Sink<'_, Id, B> {
    fn default() -> Self {
        Self {
            completed: heapless::Vec::new(),
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
            let at = self::queue(completion.queue.category()) as u8;
            self.refuse(
                Fault::DuplicateCompletion { queue: at },
                Refused::Completion(completion),
            );
        }
    }
    fn key_removed(&mut self, removed: KeyRemoved<'id, CorePath<Id>>) {
        // One removal per installed hardware key.
        if let Err(removed) = self.removed.push(removed) {
            self.refuse(Fault::RemovalOverflow, Refused::Removal(removed));
        }
    }
    fn disabled(&mut self) {}
    fn tbtt(&mut self, event: TbttEvent) {
        self.tbtt = Some(event);
    }
}

/// Where the driver's shared steps post what they produce and read the
/// settings they need: the slot-loan path's [`Held`] queues, or the data
/// port's own settings and its turn's sink.
/// A received unit a receive rule admitted, and its metadata.
pub(super) type Admitted<'id, Id, D, const B: usize> = (
    ReceiveBuffer<'id, Id, <D as Device<B>>::Receive>,
    Esp32s31RxMeta,
);

pub(super) trait DriverEnv<'id, Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> {
    /// Whether the radio stops: queued attempts end withdrawn.
    fn stopping(&self) -> bool;
    /// The MSDU lifetime of `category` on interface `vif`.
    fn lifetime(&self, vif: usize, category: WmmAccessCategory) -> Duration;
    /// Count MSDUs that expired before publication.
    fn count_expired(&mut self, msdus: u32);
    /// A single MPDU's terminal, at its ticket's `place`.
    fn tx_completed(&mut self, place: u8, completed: owned::OwnedTxCompleted<'id, Id, D, B, O>);
    /// A TBTT of the station's schedule.
    fn tbtt(&mut self, event: TbttEvent);
    /// A unit a receive cut drained, ahead of the cut's terminal.
    fn received(
        &mut self,
        frame: ReceiveBuffer<'id, Id, D::Receive>,
        meta: Esp32s31RxMeta,
    ) -> Result<(), Fault>;
    /// The next key installation serial.
    fn serial(&mut self) -> u64;
    /// The installation's cross-context storage.
    fn shared(&self) -> &'static PortSharedStorage<B, D::Lent>;
    /// The received units the caller holds.
    fn rx_out(&self) -> &Cell<usize>;
    /// Whether a diagnostic image rejects the next group replacement.
    #[cfg(oer_debug_control)]
    fn reject_group_replacement(&mut self) -> bool;
}

impl<'id, Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations>
    DriverEnv<'id, Id, D, B, O> for &Held<'_, 'id, Id, D, B, O>
{
    fn stopping(&self) -> bool {
        !self.want_enabled.get()
    }
    fn lifetime(&self, vif: usize, category: WmmAccessCategory) -> Duration {
        self.edca.lifetime(vif, category)
    }
    fn count_expired(&mut self, msdus: u32) {
        self.storage
            .expired
            .set(self.storage.expired.get().saturating_add(msdus));
    }
    fn tx_completed(&mut self, place: u8, completed: owned::OwnedTxCompleted<'id, Id, D, B, O>) {
        Held::tx_completed(self, place, completed);
    }
    fn tbtt(&mut self, event: TbttEvent) {
        if self.events.push(owned::OwnedEvent::Tbtt(event)).is_err() {
            self.loss.set(true);
        }
        self.storage.shared.wake_body();
    }
    fn received(
        &mut self,
        frame: ReceiveBuffer<'id, Id, D::Receive>,
        meta: Esp32s31RxMeta,
    ) -> Result<(), Fault> {
        if let Err(event) = self
            .events
            .push(owned::OwnedEvent::Received { frame, meta })
        {
            core::mem::forget(event);
            return Err(Fault::EventQueueFull);
        }
        self.storage.shared.wake_body();
        Ok(())
    }
    fn serial(&mut self) -> u64 {
        Held::serial(self)
    }
    fn shared(&self) -> &'static PortSharedStorage<B, D::Lent> {
        &*self.storage.shared
    }
    fn rx_out(&self) -> &Cell<usize> {
        self.rx_out
    }
    #[cfg(oer_debug_control)]
    fn reject_group_replacement(&mut self) -> bool {
        self.reject_group_replacement.replace(false)
    }
}

pub(super) struct Driver<'id, Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> {
    pub(super) core: PortCoreOwner<'id, Id, D, B>,
    pub(super) tickets: [Option<TxTicket<'id, CorePath<Id>>>; TX_TICKETS],
    pub(super) free: [Option<FreeQueue<'id, CorePath<Id>>>; 4],
    pub(super) queues: [heapless::Deque<Pending<'id, Id, D, B, O>, TX_TICKETS>; 4],
    pub(super) running: [Option<Pending<'id, Id, D, B, O>>; 4],
    /// Each queue's ordinary descriptor, while no MPDU of it is published.
    pub(super) descriptors: [Option<Pin<&'static mut TxSlot<B>>>; TX_DESCRIPTORS],
    /// The station's two data arenas over the A-MPDU chains, from
    /// preparation until retirement returns their chains and tickets.
    pub(super) arena: Option<DataArenas<'id, Id, B>>,
    pub(super) pending_command: Option<Command<'id, Id, D, B, O>>,
    /// The key of a removal the data port's caller holds at its barrier:
    /// queued attempts that name it end withdrawn, as behind
    /// `pending_command`.
    pub(super) barrier: Option<RawKeyIdentity>,
    pub(super) configuring: Option<owned::OwnedConfigured<'id, Id, D, B>>,
    pub(super) closing: Option<GateOpen<'id, Part<Id, 1>>>,
    pub(super) closed: bool,
    pub(super) keys: [Option<(RawKeyIdentity, Installed<'id, Id>)>; 20],
    pub(super) applied_ba: [Option<RxBlockAckValues>; 8],
    pub(super) transition: Option<Instant>,
    pub(super) clock_deadline: Instant,
    /// The one receiver of this driver's publication observations, present
    /// from preparation until retirement.
    pub(super) observations: Option<O>,
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
    pub(super) fn take_descriptors(&mut self) -> Option<TxDescriptors<B>> {
        let idle = self
            .descriptors
            .iter()
            .flatten()
            .all(|slot| slot.state() == crate::mac::tx::TxDmaState::Free && !slot.attached());
        if !idle || self.descriptors.iter().any(Option::is_none) {
            return None;
        }
        // Idle, empty arenas give their chains and tickets back; any other
        // keeps them, and every descriptor with them.
        let arena = self.arena.take()?;
        let ([first, second], _tickets) = match arena.into_parts() {
            Ok(parts) => parts,
            Err(arena) => {
                self.arena = Some(arena);
                return None;
            }
        };
        let [Some(a), Some(b), Some(c), Some(d)] = self.descriptors.each_mut().map(Option::take)
        else {
            return None;
        };
        Some(TxDescriptors {
            single: [a, b, c, d],
            chains: [first, second],
        })
    }
    /// A retired driver holds no admitted work for a new installation.
    pub(super) fn retired(&self) -> bool {
        self.queues.iter().all(|q| q.is_empty())
            && self.running.iter().all(Option::is_none)
            && self.arena.is_none()
    }
    pub(super) const fn vacant() -> Self {
        Self {
            core: PortCoreOwner::<Id, D, B>::vacant(),
            tickets: [const { None }; TX_TICKETS],
            free: [const { None }; 4],
            queues: [const { heapless::Deque::new() }; 4],
            running: [const { None }; 4],
            descriptors: [const { None }; TX_DESCRIPTORS],
            arena: None,
            pending_command: None,
            barrier: None,
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
    /// Take an installation's resources. The data arenas take the chains
    /// and two of the core's tickets.
    ///
    /// # Errors
    ///
    /// A chain is not idle: the chains and tickets stay quarantined.
    #[inline(never)]
    pub(super) fn prepare(
        &mut self,
        core: PortCoreOwner<'id, Id, D, B>,
        tickets: [TxTicket<'id, CorePath<Id>>; TX_TICKETS],
        queues: [FreeQueue<'id, CorePath<Id>>; 4],
        descriptors: TxDescriptors<B>,
        observations: O,
        now: Instant,
    ) -> Result<(), Fault> {
        // The one caller checked `retired` before taking any resource.
        self.core = core;
        self.tickets = tickets.map(Some);
        self.free = queues.map(Some);
        let TxDescriptors { single, chains } = descriptors;
        self.descriptors = single.map(Some);
        self.observations = Some(observations);
        self.clock_deadline = now.saturating_add(Duration::from_secs(60));
        let mut taken = self.tickets.iter_mut().filter_map(Option::take);
        let (Some(first), Some(second)) = (taken.next(), taken.next()) else {
            core::mem::forget(chains);
            return Err(Fault::InstallationResources);
        };
        match DataArenas::new(chains, [first, second]) {
            Ok(arena) => {
                self.arena = Some(arena);
                Ok(())
            }
            Err(parts) => {
                // A busy chain: its owners stay quarantined.
                core::mem::forget(parts);
                Err(Fault::InstallationResources)
            }
        }
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
        self.barrier = None;
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
                    let lifetime = held.edca.lifetime(vif(pending.role()), pending.category);
                    deadline = deadline.min(at.saturating_add(lifetime));
                }
            }
        }
        if let Some(at) = self.arena.as_ref().and_then(DataArenas::deadline) {
            deadline = deadline.min(at);
        }
        Ok(deadline)
    }
    pub(super) fn finish(
        &mut self,
        env: &mut impl DriverEnv<'id, Id, D, B, O>,
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
            env.count_expired(msdus.count_ones());
        }
        let place = pending.operation.ticket().index();
        env.tx_completed(
            place,
            owned::OwnedTxCompleted::Single(pending.operation.complete(outcome)),
        );
    }
    // The retained barrier is also the withdrawal marker; no owner or result
    // is moved out of its AC while an earlier owner can still complete.
    pub(super) fn withdrawal(
        &self,
        role: VifRole,
        key: Option<RawKeyIdentity>,
    ) -> Option<WithdrawCause> {
        if self.configuring.as_ref().is_some_and(|p| p.role() == role) {
            return Some(WithdrawCause::Disabled);
        }
        match &self.pending_command {
            Some(Command::Remove { identity, .. })
            | Some(Command::Group {
                command: GroupCommand::Remove { identity, .. },
                ..
            }) if key == Some(*identity) => Some(WithdrawCause::KeyRemoved),
            _ if key.is_some() && key == self.barrier => Some(WithdrawCause::KeyRemoved),
            _ => None,
        }
    }
    #[inline(never)]
    pub(super) fn withdraw(&mut self, env: &mut impl DriverEnv<'id, Id, D, B, O>) -> bool {
        let global = if env.stopping() {
            Some(WithdrawCause::Disabled)
        } else if self.closing.is_some() {
            Some(WithdrawCause::GateClosed)
        } else {
            None
        };
        // Without a global cause, a configuration or a key removal, no
        // owner is withdrawn (`withdrawal` names none): the common turn.
        let selective = self.configuring.is_some()
            || self.barrier.is_some()
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
            // A published arena holds its queue as a published MPDU does.
            if self.running[index].is_some() || self.free[index].is_none() {
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
                self.finish(env, queued, TxOutcome::Withdrawn(cause));
            }
        }
        !self
            .queues
            .iter()
            .flat_map(|queue| queue.iter())
            .chain(self.running.iter().flatten())
            .map(|pending| (pending.role(), pending.key()))
            .any(|(role, key)| self.withdrawal(role, key).is_some())
            && !self.arena_holds_removed_key()
    }
    #[inline(always)]
    pub(super) fn blocked(&self, pending: &Pending<'id, Id, D, B, O>) -> bool {
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
    pub(super) fn expired(
        env: &impl DriverEnv<'id, Id, D, B, O>,
        pending: &Pending<'id, Id, D, B, O>,
        now: Instant,
    ) -> bool {
        pending.admission().is_some_and(|at| {
            now.saturating_duration_since(at) >= env.lifetime(vif(pending.role()), pending.category)
        })
    }
    pub(super) fn collect(
        &mut self,
        env: &mut impl DriverEnv<'id, Id, D, B, O>,
        sink: &mut Sink<'id, Id, B>,
        stopping: bool,
    ) -> Result<(), Fault> {
        if let Some(fault) = sink.fault() {
            return Err(fault);
        }
        for completion in sink.completed.drain(..) {
            let index = queue(completion.queue.category());
            let at = index as u8;
            let Some(mut pending) = self.running[index].take() else {
                return Err(Fault::CompletionWithoutPending { queue: at });
            };
            self.free[index] = Some(completion.queue);
            let ticket_index = completion.ticket.index();
            self.tickets[ticket_index] = Some(completion.ticket);
            let mut slot = completion.slot;
            // The buffer leaves the queue's descriptor and rejoins the right
            // its publication kept.
            let memory = slot
                .as_mut()
                .detach()
                .ok_or(Fault::DescriptorMissing { queue: at })?;
            *self
                .descriptors
                .get_mut(index)
                .ok_or(Fault::DescriptorMissing { queue: at })? = Some(slot);
            if pending
                .operation
                .execute()
                .mpdu
                .buffer
                .dma
                .complete(memory)
                .is_err()
            {
                return Err(Fault::ReturnRefused { queue: at });
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
                    .ok_or(Fault::AttemptCounter { queue: at })?;
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
                            report: block_ack.ok_or(Fault::BlockAckReportMissing { queue: at })?,
                            ack: Ack::from_sample(ack_snr_db)?,
                        },
                    };
                    Some(TxOutcome::Ended(SingleEnd::Delivered {
                        air: pending.air().ok_or(Fault::AirMissing { queue: at })?,
                        response,
                    }))
                }
                end => {
                    pending.stage_tries = pending
                        .stage_tries
                        .checked_add(1)
                        .ok_or(Fault::AttemptCounter { queue: at })?;
                    let stage = pending
                        .plan
                        .stages()
                        .nth(pending.stage)
                        .ok_or(Fault::StageMissing { queue: at })?;
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
                            _ => SingleEnd::AckTimeout(
                                pending.air().ok_or(Fault::AirMissing { queue: at })?,
                            ),
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
                self.finish(env, pending, outcome);
            } else if let Err(pending) = self.queues[index].push_front(pending) {
                // The retry stays in custody with the quarantine.
                core::mem::forget(pending);
                return Err(Fault::QueueFull { queue: at });
            }
        }
        if let Some(tbtt) = sink.tbtt.take() {
            env.tbtt(tbtt);
        }
        if !sink.removed.is_empty() {
            return Err(Fault::RemovalOverflow);
        }
        self.withdraw(env);
        Ok(())
    }
    /// Offer one TX edge to the core's MPDUs and to the published arena.
    /// Their timeout aborts force CCA one at a time: when one side's
    /// settle ends, the other side's latched timeouts are offered again, as
    /// the core does among its queues.
    pub(super) fn service_tx(
        &mut self,
        env: &mut impl DriverEnv<'id, Id, D, B, O>,
        device: &mut D,
        wake: WifiTxWake,
        stopping: bool,
        now: Instant,
    ) -> Result<Option<owned::ArenaDone>, Fault> {
        if !self.arena_on_air() {
            // No arena on the air: the MPDUs' settles are the core's own.
            let mut sink = Sink::default();
            core_views::service::<Id, D, B>(self.core.view(), device.hardware()?, wake, &mut sink)?;
            self.collect(env, &mut sink, stopping)?;
            return Ok(None);
        }
        let settling = |driver: &mut Self| {
            driver
                .core
                .view()
                .arena_core()
                .is_some_and(|core| core.abort_settling())
        };
        let timeout = WifiTxWake::Interrupt {
            events: crate::mac::irq::EVENT_TX_TIMEOUT,
        };
        let before = settling(self);
        let mut sink = Sink::default();
        core_views::service::<Id, D, B>(self.core.view(), device.hardware()?, wake, &mut sink)?;
        self.collect(env, &mut sink, stopping)?;
        let settled = before && !settling(self);
        let (done, again) =
            self.service_arena(device, if settled { timeout } else { wake }, now)?;
        if again {
            let mut sink = Sink::default();
            core_views::service::<Id, D, B>(
                self.core.view(),
                device.hardware()?,
                timeout,
                &mut sink,
            )?;
            self.collect(env, &mut sink, stopping)?;
        }
        Ok(done)
    }
    /// Queue one unit a cut drained for the body, ahead of the cut's
    /// terminal, or return it to the ring when no rule admits it.
    pub(super) fn receive(
        &mut self,
        env: &mut impl DriverEnv<'id, Id, D, B, O>,
        device: &mut D,
        unit: D::Receive,
    ) -> Result<(), Fault> {
        let shared = env.shared();
        if let Some((frame, meta)) = self.admit(device, env.rx_out(), shared, unit)? {
            env.received(frame, meta)?;
        }
        Ok(())
    }
    /// The body's event for one received unit, counted in `rx_out`, or
    /// `None` once the unit is back in the ring because no receive rule
    /// admits it. The body's pull and the driver's cut both admit here.
    pub(super) fn admit(
        &mut self,
        device: &mut D,
        rx_out: &Cell<usize>,
        shared: &'static PortSharedStorage<B, D::Lent>,
        unit: D::Receive,
    ) -> Result<Option<Admitted<'id, Id, D, B>>, Fault> {
        if rx_out.get() >= RX_HELD {
            // The unit stays out of the ring with the quarantine.
            core::mem::forget(unit);
            return Err(Fault::ReceiveOverflow);
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
        let Some((Some(start), len, meta)) = accepted else {
            device.recycle(unit);
            return Ok(None);
        };
        let mut unit = unit;
        // The buffer lends the MPDU from here on: its view is narrowed
        // once, checked against the received unit.
        unit.narrow(start, len).map_err(|_| Fault::ReceiveRange)?;
        rx_out.set(rx_out.get().saturating_add(1));
        Ok(Some((
            ReceiveBuffer::new(
                unit,
                super::LossGuard::new(&shared.poisoned, &shared.driver),
            ),
            meta,
        )))
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
        let mut env = held;
        settle_lent(&held.storage.shared.network, held.rx_out, device)?;
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
                .ok_or(Fault::Setting(settings::SettingFault::ChannelUnset))?;
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
        if !completion_held
            && let Some(done) =
                self.service_tx(&mut env, device, WifiTxWake::Deadline, stopping, now)?
        {
            held.arena_completed(done);
        }
        // One turn serves up to `EVENTS_PER_POLL` transmit and TBTT events.
        // A turn that ends on `Pending` is registered for the next one; a
        // turn that used its budget, or served a TX wake whose completions
        // freed queues, polls again. Received units stay in the ring for the
        // body's pull: the driver wakes the body when the ring has work.
        let mut again = true;
        let mut served_tx = false;
        for _ in 0..EVENTS_PER_POLL {
            match device.poll_event(cx) {
                Poll::Ready(Ok(DeviceEvent::Tx(wake))) => {
                    if let Some(done) = self.service_tx(&mut env, device, wake, stopping, now)? {
                        held.arena_completed(done);
                    }
                    served_tx = true;
                }
                Poll::Ready(Ok(DeviceEvent::Tbtt)) => {
                    let mut sink = Sink::default();
                    if !core_views::configuring::<Id, D, B>(self.core.view()) {
                        self.core
                            .view()
                            .commands()
                            .ok_or(Fault::CoreViewUnavailable(CoreOp::Tbtt))?
                            .station_tbtt(device.hardware()?, &mut sink);
                    }
                    self.collect(&mut env, &mut sink, stopping)?;
                }
                Poll::Ready(Err(cause)) => return Err(cause),
                Poll::Pending => {
                    again = false;
                    break;
                }
            }
        }
        if device.receive_due() {
            held.receive_due.set(true);
            held.storage.shared.wake_body();
        }
        // Returns wake the driver only when it asks, with many units in
        // the network, where the radio may run short. Otherwise the next
        // interrupt's poll counts them.
        let network = &held.storage.shared.network;
        let short = network.held() >= D::LENT_RX / 2 && network.want_units(cx);
        if again || served_tx || short {
            cx.waker().wake_by_ref();
        }
        // A station configuration barrier waits for the station's published
        // arena as the core waits for its MPDUs.
        let configuring_arena = self
            .configuring
            .as_ref()
            .is_some_and(|permission| permission.role() == VifRole::Station)
            && self.arena_on_air();
        if self.configuring.is_some() && !configuring_arena {
            if core_views::configuring::<Id, D, B>(self.core.view()) {
                let mut sink = Sink::default();
                let done = core_views::advance_configuration::<Id, D, B>(
                    self.core.view(),
                    device.hardware()?,
                    &mut sink,
                );
                self.collect(&mut env, &mut sink, stopping)?;
                if !done {
                    return Ok(());
                }
            }
            if self.withdraw(&mut env)
                && let Some(permission) = self.configuring.take()
            {
                held.terminal(owned::OwnedTerminal::Configured(permission));
            }
        }
        if held.dirty_edca.replace(false) {
            for role in [VifRole::Station, VifRole::AccessPoint] {
                let Some(table) = held.edca.get(vif(role)) else {
                    // The body is publishing it: its commit wake serves it again.
                    held.dirty_edca.set(true);
                    break;
                };
                let _installed = idle_command!(self.core, |view| view.set_checked_edca(
                    role,
                    SupportedEdca::<Esp32s31Caps<B>>::new(table).ok_or(Fault::EdcaUnsupported)?,
                ))
                .map_err(|_| Fault::EdcaUnsupported)?;
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
                    // The body is publishing it: its commit wake serves it again.
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
                    self.withdraw(&mut env);
                    break;
                }
                self.pending_command = held.commands.pop();
            }
            let drained = self.withdraw(&mut env);
            let Some(command) = self.pending_command.take() else {
                break;
            };
            match command {
                Command::Submit(pending) => {
                    let index = queue(pending.category);
                    if let Err(pending) = self.queues[index].push_back(pending) {
                        // The attempt stays in custody with the quarantine.
                        core::mem::forget(pending);
                        return Err(Fault::QueueFull { queue: index as u8 });
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
                    self.collect(&mut env, &mut sink, stopping)?;
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
                            self.receive(&mut env, device, unit)?;
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
                            let terminal = self.group(&mut env, device, at, command)?;
                            if let Err(cause) = device.finish_rx_cut() {
                                handoff::retain_terminal(held.failed_terminal, terminal.into());
                                return Err(cause);
                            }
                            // The resumed ring is the body's again; the
                            // terminal's wake brings its next pull.
                            held.receive_due.set(true);
                            held.terminal(terminal);
                        }
                    }
                }
            }
        }
        self.withdraw(&mut env);
        // A gate closes after the published arena ends, as after the MPDUs
        // the core holds.
        if self.closing.is_some()
            && !self.arena_on_air()
            && !core_views::configuring::<Id, D, B>(self.core.view())
            && core_views::close::<Id, D, B>(self.core.view(), device.hardware()?)
            && let Some(gate) = self.closing.take()
        {
            // The core has returned every published head. Existing AC queues
            // retain their tails in order until those heads are terminal.
            let gate = held.gate.close(gate);
            self.closed = true;
            held.terminal(owned::OwnedTerminal::GateClosed(gate));
        }
        for index in 0..4 {
            // A published MPDU or arena holds the queue.
            if self.running[index].is_some() || self.free[index].is_none() {
                continue;
            }
            // Expiry continues behind a gate or a command barrier.
            while self.queues[index]
                .front()
                .is_some_and(|p| Self::expired(&env, p, now))
            {
                let Some(pending) = self.queues[index].pop_front() else {
                    break;
                };
                self.finish(&mut env, pending, TxOutcome::Expired { msdus: 1 });
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
            if let Some(pending) = self.queues[index].pop_front() {
                self.publish(device, index, pending)?;
            }
        }
        if stopping
            && !self.arena_on_air()
            && self.pending_command.is_none()
            && held.commands.empty()
            && !core_views::configuring::<Id, D, B>(self.core.view())
            && self.configuring.is_none()
            && self.closing.is_none()
        {
            let mut sink = Sink::default();
            core_views::disable::<Id, D, B>(self.core.view(), device.hardware()?, &mut sink);
            self.collect(&mut env, &mut sink, true)?;
            let mut sink = Sink::default();
            let disabled = core_views::finish_disable::<Id, D, B>(self.core.view(), &mut sink);
            self.collect(&mut env, &mut sink, true)?;
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
    pub(super) fn publish(
        &mut self,
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
                // A data or management frame sent again carries the Retry
                // bit; a BlockAckReq, a control frame, never does (IEEE Std
                // 802.11-2020, 9.2.4.1.5).
                retry: mpdu.published && mpdu.layout.kind() != MpduKind::BlockAckRequest,
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
        if let Err(refused) = core.submit_checked(device.hardware()?, queue, admission, values) {
            core::mem::forget(refused);
            return Err(Fault::Capacity);
        }
        #[cfg(all(oer_trace, oer_debug_control))]
        oer_chips_family_staged_runtime::trace::emit(&publication);
        Ok(())
    }
    pub(super) fn install(
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
    pub(super) fn remove(
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
    pub(super) fn removed_terminal(
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
    pub(super) fn group(
        &mut self,
        env: &mut impl DriverEnv<'id, Id, D, B, O>,
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
                    serial: env.serial(),
                    key_id,
                    activity: &env.shared().key_live[1],
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
                    serial: env.serial(),
                    key_id,
                    activity: &env.shared().key_live[17 + usize::from(slot.index())],
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
                    reject: env.reject_group_replacement(),
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
                        let shared = env.shared();
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
                        oer_chips_family_staged_runtime::trace::emit(&trace::GroupRollback {
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
                    serial: env.serial(),
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
/// Take the network's lends off the body's receive count and rearm the
/// ring with the units it returned. The driver's turn and the body's pull
/// both settle, whichever runs first.
pub(super) fn settle_lent<D: Device<B>, const B: usize>(
    network: &lent::NetworkCredit<D::Lent>,
    rx_out: &Cell<usize>,
    device: &mut D,
) -> Result<(), Fault> {
    let transferred = network.take_transferred();
    rx_out.set(
        rx_out
            .get()
            .checked_sub(transferred)
            .ok_or(Fault::ReceiveAccounting)?,
    );
    if network.take_returned() {
        device.returned();
    }
    Ok(())
}
pub(super) fn queue(category: WmmAccessCategory) -> usize {
    usize::from(LegacyTxQueue::from_access_category(category).hardware_index())
}
pub(super) fn vif(role: VifRole) -> usize {
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

/// The installed key `identity` names, `None` for an open attempt.
fn admitted_key<'a, 'id, Id: SlotIdentity>(
    keys: &'a [Option<(RawKeyIdentity, Installed<'id, Id>)>],
    identity: Option<RawKeyIdentity>,
) -> Result<Option<&'a Installed<'id, Id>>, Fault> {
    let Some(identity) = identity else {
        return Ok(None);
    };
    keys.iter()
        .flatten()
        .find(|(key, _)| *key == identity)
        .map(|(_, key)| Some(key))
        .ok_or(Fault::Capacity)
}

/// The port asked for the radio and the device has not started it yet.
fn starting<Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations>(
    held: &Held<'_, '_, Id, D, B, O>,
) -> bool {
    held.want_enabled.get() && !held.enabled.get()
}
