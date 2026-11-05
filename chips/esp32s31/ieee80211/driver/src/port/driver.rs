use super::*;
use super::{
    installation::PortCoreOwner,
    keys::{GroupCommand, Installation, Installed, RawKeyIdentity, Removal},
};
use oer_protocols_ieee80211_wire::qos::WmmAccessCategory;

pub(super) mod arena;
pub(super) use arena::DataArenas;
pub(super) mod core_views;
#[cfg(oer_debug_control)]
mod key_replacement;
// The idle core lends the shared command set; a core in another view is a
// typed refusal.
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
    /// When the port admitted its MPDU.
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
    /// never returned to a pool, and the service ends with the fault.
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

/// A received unit a receive rule admitted, and its metadata.
pub(super) type Admitted<'id, Id, D, const B: usize> = (
    ReceiveBuffer<'id, Id, <D as Device<B>>::Receive>,
    Esp32s31RxMeta,
);

/// Where the driver's steps post what they produce and read the settings
/// they need: the data port's settings and its turn's sink.
pub(super) trait DriverEnv<'id, Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> {
    /// Whether the radio stops: queued attempts end withdrawn.
    fn stopping(&self) -> bool;
    /// The MSDU lifetime of `category` on interface `vif`.
    fn lifetime(&self, vif: usize, category: WmmAccessCategory) -> Duration;
    /// Count MSDUs that expired before publication.
    fn count_expired(&mut self, msdus: u32);
    /// A single MPDU's terminal, at its ticket's `place`.
    fn tx_completed(&mut self, place: u8, completed: brand::OwnedTxCompleted<'id, Id, D, B, O>);
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
    /// A group key replacement the hardware refused, kept with the port's
    /// quarantine.
    pub(super) retained_group: Option<GroupCommand<'id, Id>>,
    /// The key of a removal the data port's caller holds at its barrier:
    /// queued attempts that name it end withdrawn.
    pub(super) barrier: Option<RawKeyIdentity>,
    pub(super) configuring: Option<brand::OwnedConfigured<'id, Id, D, B>>,
    pub(super) closing: Option<GateOpen<'id, Part<Id, 1>>>,
    pub(super) closed: bool,
    pub(super) keys: [Option<(RawKeyIdentity, Installed<'id, Id>)>; 20],
    pub(super) applied_ba: [Option<RxBlockAckValues>; 8],
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
            retained_group: None,
            barrier: None,
            configuring: None,
            closing: None,
            closed: false,
            keys: [const { None }; 20],
            applied_ba: [None; 8],
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
    ) -> Result<(), Fault> {
        // The one caller checked `retired` before taking any resource.
        self.core = core;
        self.tickets = tickets.map(Some);
        self.free = queues.map(Some);
        let TxDescriptors { single, chains } = descriptors;
        self.descriptors = single.map(Some);
        self.observations = Some(observations);
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
        drop(self.retained_group.take());
        self.barrier = None;
        drop(self.configuring.take());
        drop(self.closing.take());
        self.closed = false;
        self.applied_ba = [None; 8];
        let (transmitter, descriptors) = transmitter.zip(descriptors)?;
        Some((transmitter, descriptors, observations?))
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
            brand::OwnedTxCompleted::Single(pending.operation.complete(outcome)),
        );
    }
    // The retained barrier is also the withdrawal marker; no owner or result
    // is moved out of its AC while an earlier owner can still complete.
    pub(super) fn withdrawal(
        &self,
        role: VifRole,
        key: Option<RawKeyIdentity>,
    ) -> Option<WithdrawCause> {
        // A configuration is the station's.
        if self.configuring.is_some() && role == VifRole::Station {
            return Some(WithdrawCause::Disabled);
        }
        (key.is_some() && key == self.barrier).then_some(WithdrawCause::KeyRemoved)
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
        let selective = self.configuring.is_some() || self.barrier.is_some();
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
                    status: TxStatus::KeyRefused,
                    ..
                } => Some(TxOutcome::Indeterminate(IndeterminateCause::KeyRefused)),
                PublicationEnd::Completed {
                    status: TxStatus::Success,
                    ack_snr_db,
                    block_ack,
                } => {
                    let response = match pending.operation.payload().layout.kind() {
                        MpduKind::Group | MpduKind::IndividualNoAck => SingleResponse::Unsolicited,
                        MpduKind::Individual => SingleResponse::Ack(Ack::from_sample(ack_snr_db)),
                        MpduKind::BlockAckRequest => SingleResponse::BlockAck {
                            report: block_ack.ok_or(Fault::BlockAckReportMissing { queue: at })?,
                            ack: Ack::from_sample(ack_snr_db),
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
        arena_ended: bool,
        stopping: bool,
        now: Instant,
    ) -> Result<Option<brand::ArenaDone>, Fault> {
        if arena_ended || !self.arena_on_air() {
            // No arena to offer the edge: the MPDUs' settles are the core's
            // own.
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
    /// Post one unit a cut drained to the caller, ahead of the cut's
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
    /// The caller's received unit, counted in `rx_out`, or
    /// `None` once the unit is back in the ring because no receive rule
    /// admits it. The port's receive pull and its cut both admit here.
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
            ReceiveBuffer::new(unit, super::LossGuard::new(&shared.lost)),
            meta,
        )))
    }
}
/// Publication reports to the driver's observation receiver.
impl<'id, Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations>
    Driver<'id, Id, D, B, O>
{
    pub(super) fn publish(
        &mut self,
        device: &mut D,
        index: usize,
        mut pending: Pending<'id, Id, D, B, O>,
    ) -> Result<(), Fault> {
        let at = index as u8;
        let refused = |step| Fault::Publish { queue: at, step };
        let stage = pending
            .plan
            .stages()
            .nth(pending.stage)
            .ok_or(Fault::StageMissing { queue: at })?;
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
            return Err(Fault::CoreViewUnavailable(CoreOp::Publish));
        };
        let observations = self
            .observations
            .as_mut()
            .ok_or(refused(PublishStep::Observations))?;
        let backoff_slots = core.draw_backoff(role, pending.category, observations);
        let core_ticket = self
            .tickets
            .iter_mut()
            .find_map(Option::take)
            .ok_or(refused(PublishStep::Ticket))?;
        let key_handle = admitted_key(&self.keys, pending.key())?;
        let admission = match key_handle {
            Some(Installed::Transmit(key)) => core
                .admit_key(key, core_ticket)
                .map(TxAdmission::Protected)
                .map_err(|_| refused(PublishStep::KeyAdmission))?,
            Some(Installed::Receive(_)) => return Err(refused(PublishStep::ReceiveKey)),
            None => TxAdmission::Open(core_ticket),
        };
        let mpdu = pending.operation.execute().mpdu;
        // The hardware takes the memory through the queue's descriptor; the
        // buffer keeps its return right.
        let lease = mpdu
            .buffer
            .dma
            .publish()
            .ok_or(refused(PublishStep::Buffer))?;
        let mut slot = self
            .descriptors
            .get_mut(index)
            .and_then(Option::take)
            .ok_or(refused(PublishStep::Descriptor))?;
        if slot.as_mut().attach(lease).is_err() {
            return Err(refused(PublishStep::Attach));
        }
        let TxSlotAccess::Free(mut memory) = slot.access() else {
            return Err(refused(PublishStep::DescriptorBusy));
        };
        if !mpdu.published {
            // A new MSDU: the descriptor's exchange record starts over.
            memory.reset_work();
            if let (Some(Installed::Transmit(key)), Some(pn)) = (key_handle, mpdu.pn) {
                let buffer = memory
                    .buffer_mut()
                    .map_err(|_| refused(PublishStep::HeaderBuffer))?;
                core.write_ccmp_header(key, buffer, mpdu.layout.header_len(), pn)
                    .map_err(|_| refused(PublishStep::CcmpHeader))?;
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
        .map_err(|_| refused(PublishStep::Values))?;
        mpdu.published = true;
        pending.publications = pending
            .publications
            .checked_add(1)
            .ok_or(refused(PublishStep::Count))?;
        #[cfg(all(oer_trace, oer_debug_control))]
        let publication = super::trace::Publication {
            ticket: pending.operation.ticket().index(),
            buffer: pending.operation.execute().mpdu.buffer.index(),
        };
        let queue = self.free[index].take().ok_or(refused(PublishStep::Queue))?;
        // Durable backing owns the indivisible operation before publication.
        self.running[index] = Some(pending);
        if let Err(failed) = core.submit_checked(device.hardware()?, queue, admission, values) {
            let fault = failed.fault;
            core::mem::forget(failed);
            return Err(Fault::Core(fault));
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
            .ok_or(Fault::KeySlotMissing(KeyOp::Install))?;
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
            .ok_or(Fault::KeySlotMissing(KeyOp::Remove))?;
        let (_, installed) = place.take().ok_or(Fault::KeySlotMissing(KeyOp::Remove))?;
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
        sink.removed.pop().ok_or(Fault::RemovalMissing)
    }
    pub(super) fn removed_terminal(
        &self,
        removal: Removal<'id, Id>,
        removed: KeyRemoved<'id, CorePath<Id>>,
    ) -> Result<brand::OwnedRemoved<'id, Id>, Fault> {
        use core_mac::association::VacantKeySlot as Slot;
        let removed = match (removal, removed.into_slot()) {
            (Removal::StationPairwise(association), Slot::StationPairwise(slot)) => {
                brand::OwnedRemoved::Station(brand::RemovedOwners::Pairwise { slot, association })
            }
            (Removal::AccessPointPairwise(association), Slot::AccessPointPairwise(slot)) => {
                brand::OwnedRemoved::AccessPoint(brand::RemovedOwners::Pairwise {
                    slot,
                    association,
                })
            }
            (Removal::StationGroup(permission), Slot::StationGroup(slot)) => {
                brand::OwnedRemoved::Station(brand::RemovedOwners::Group { slot, permission })
            }
            (Removal::AccessPointGroup(permission), Slot::AccessPointGroup(slot)) => {
                brand::OwnedRemoved::AccessPoint(brand::RemovedOwners::Group { slot, permission })
            }
            _ => return Err(Fault::RemovalMismatch),
        };
        Ok(removed)
    }
    pub(super) fn group(
        &mut self,
        env: &mut impl DriverEnv<'id, Id, D, B, O>,
        device: &mut D,
        command: GroupCommand<'id, Id>,
    ) -> Result<brand::OwnedTerminal<'id, Id, D, B>, Fault> {
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
                return Ok(brand::OwnedTerminal::GroupKeyInstalled(
                    brand::OwnedInstalledGroup::Station {
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
                    return Err(Fault::GroupKeyIdRange);
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
                return Ok(brand::OwnedTerminal::GroupKeyInstalled(
                    brand::OwnedInstalledGroup::AccessPoint {
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
                    .ok_or(Fault::KeySlotMissing(KeyOp::Replace))?;
                let Installed::Receive(installed) = &mut entry.1 else {
                    return Err(Fault::KeySlotMissing(KeyOp::Replace));
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
                    self.retained_group = Some(GroupCommand::Replace {
                        permission,
                        key,
                        key_id,
                        material,
                    });
                    // Observe actual quarantined owners after moving them to
                    // permanent custody, never a synthetic success terminal.
                    #[cfg(test)]
                    if let Some(GroupCommand::Replace {
                        permission,
                        key,
                        key_id,
                        ..
                    }) = &self.retained_group
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
                    if let Some(GroupCommand::Replace {
                        permission,
                        key,
                        key_id,
                        ..
                    }) = &self.retained_group
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
                return Ok(brand::OwnedTerminal::GroupKeyReplaced {
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
                    return Err(Fault::KeySlotMissing(KeyOp::Select));
                }
                return Ok(brand::OwnedTerminal::GroupKeySelected { key, permission });
            }
            GroupCommand::Remove { identity, removal } => {
                let removed = self.remove(device, identity)?;
                return Ok(brand::OwnedTerminal::KeyRemoved(
                    self.removed_terminal(removal, removed)?,
                ));
            }
        }
    }
}
/// Take the network's lends off the caller's receive count and rearm the
/// ring with the units it returned. The port's receive pull and its retire
/// checks both settle, whichever runs first.
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
        .ok_or(Fault::KeySlotMissing(KeyOp::Transmit))
}
