use super::*;
use super::{
    installation::PortCoreOwner,
    keys::{GroupCommand, Installation, Installed, RawKeyIdentity, Removal},
};
use crate::ordinary_tx::sets_retry_bit;
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
    /// The transmissions left at the plan's first stage after this one.
    pub tries_left: u8,
    /// Whether this attempt was published before.
    pub published: bool,
    /// What it has put on the air.
    pub air: Option<Air>,
    /// A beacon's permission, which its terminal returns, and the instant
    /// it must be published by.
    pub beacon: Option<(BeaconPermission<'static, Part<Id, 10>>, Instant)>,
}
impl<Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> Pending<'_, Id, D, B, O> {
    /// A beacon's deadline.
    pub(super) fn deadline(&self) -> Option<Instant> {
        self.beacon.as_ref().map(|(_, deadline)| *deadline)
    }
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
    /// One more transmission went out at `rate`. A plan bounds them, and the
    /// bound fits the count.
    fn transmitted(&mut self, rate: PhyRate) -> Air {
        const {
            if RETRY_STAGES * (u8::MAX as usize) > u16::MAX as usize {
                panic!("a retry plan's transmissions fit their count");
            }
        }
        let transmissions = self.air.map_or(core::num::NonZeroU16::MIN, |air| {
            air.transmissions.saturating_add(1)
        });
        let air = Air {
            transmissions,
            rate,
        };
        self.air = Some(air);
        air
    }
    /// Spend one transmission of the plan: whether any is left.
    fn spend(&mut self) -> bool {
        if let Some(left) = self.tries_left.checked_sub(1) {
            self.tries_left = left;
            return true;
        }
        match self.plan.after_first() {
            Some(rest) => {
                self.tries_left = rest.first().transmissions.get().saturating_sub(1);
                self.plan = rest;
                true
            }
            None => false,
        }
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
            // A later refusal adds no cause. It lies past the four unique
            // queue places and the installed keys, a second time: its
            // tokens own no memory, and a descriptor it holds stays
            // attached to its memory, outside every pool.
            let _excess = owners;
        }
    }
    /// The recorded contract fault, retaining its owners in quarantine.
    pub(super) fn fault(&mut self) -> Option<Fault> {
        let (fault, owners) = self.refused.take()?;
        let _quarantined = core::mem::ManuallyDrop::new(owners);
        Some(fault)
    }
    /// The recorded contract fault with the owners it refused.
    fn take_refused(&mut self) -> Option<(Fault, Refused<'id, Id, B>)> {
        self.refused.take()
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

/// The attempts one access category's queue holds. Every queued or
/// published attempt holds one of the installation's [`TX_TICKETS`]
/// tickets, so a queue that holds them all never refuses a push.
pub(super) const QUEUE_DEPTH: usize = TX_TICKETS;
const _: () = {
    if QUEUE_DEPTH < TX_TICKETS {
        panic!("each transmit queue holds every ticket's attempt");
    }
};

/// An admission the core did not take back as a ticket: kept with the
/// driver until its retirement.
#[allow(dead_code, reason = "withheld owners are never read, only kept")]
pub(super) enum Withheld<'id, Id: SlotIdentity> {
    /// The core's view could not take it back.
    Admission(TxAdmission<'id, CorePath<Id>>),
    /// The faulted core keeps it.
    Quarantined(crate::lower_mac::QuarantinedAdmission<'id, CorePath<Id>>),
}

/// A step a test refuses, which the installation's counts exclude.
#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TestRefusal {
    /// The next push into a transmit queue.
    QueuePush,
    /// The next completion's return of its memory to its buffer.
    Return,
    /// The core's next submission.
    Submit,
}

pub(super) struct Driver<'id, Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> {
    pub(super) core: PortCoreOwner<'id, Id, D, B>,
    pub(super) tickets: [Option<TxTicket<'id, CorePath<Id>>>; TX_TICKETS],
    /// Refuse the next publication's values, past its buffer's attachment.
    #[cfg(test)]
    pub(super) refuse_values_for_test: bool,
    /// Refuse the next step of this kind, which the counts exclude.
    #[cfg(test)]
    pub(super) refuse_for_test: Option<TestRefusal>,
    pub(super) free: [Option<FreeQueue<'id, CorePath<Id>>>; 4],
    pub(super) queues: [heapless::Deque<Pending<'id, Id, D, B, O>, QUEUE_DEPTH>; 4],
    /// Admissions of refused publications the core did not take back.
    pub(super) withheld: [Option<Withheld<'id, Id>>; TX_TICKETS],
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
    /// The access point's configuration waits at its barrier: its queued
    /// attempts end withdrawn.
    pub(super) access_point_configuring: bool,
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
            #[cfg(test)]
            refuse_values_for_test: false,
            #[cfg(test)]
            refuse_for_test: None,
            free: [const { None }; 4],
            queues: [const { heapless::Deque::new() }; 4],
            withheld: [const { None }; TX_TICKETS],
            running: [const { None }; 4],
            descriptors: [const { None }; TX_DESCRIPTORS],
            arena: None,
            retained_group: None,
            barrier: None,
            access_point_configuring: false,
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
        for admission in &mut self.withheld {
            drop(admission.take());
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
        self.access_point_configuring = false;
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
        if pending.published {
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
        let completed = pending.operation.complete(outcome);
        let completed = match pending.beacon {
            Some((permission, _)) => brand::OwnedTxCompleted::Beacon {
                completed,
                permission,
            },
            None => brand::OwnedTxCompleted::Single(completed),
        };
        env.tx_completed(place, completed);
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
        // So is the access point's, which ends its own attempts.
        if self.access_point_configuring && role == VifRole::AccessPoint {
            return Some(WithdrawCause::GateClosed);
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
        let selective =
            self.configuring.is_some() || self.access_point_configuring || self.barrier.is_some();
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
        pending.deadline().is_some_and(|deadline| now >= deadline)
            || pending.admission().is_some_and(|at| {
                now.saturating_duration_since(at)
                    >= env.lifetime(vif(pending.role()), pending.category)
            })
    }
    /// End the queued beacon of queue `index` whose deadline passed at
    /// `now`, wherever it waits in the queue; the others keep their order.
    /// The late attempt moves to the head past the ones before it and
    /// leaves from there: no attempt leaves the queue but the one that ends.
    pub(super) fn expire_beacons(
        &mut self,
        env: &mut impl DriverEnv<'id, Id, D, B, O>,
        index: usize,
        now: Instant,
    ) {
        let Some(queue) = self.queues.get_mut(index) else {
            return;
        };
        let late = |pending: &Pending<'id, Id, D, B, O>| {
            pending.deadline().is_some_and(|deadline| now >= deadline)
        };
        // One beacon permission exists, so at most one beacon is late.
        let Some(at) = queue.iter().position(late) else {
            return;
        };
        let (front, back) = queue.as_mut_slices();
        to_head(front, back, at);
        if queue.front().is_some_and(late)
            && let Some(pending) = queue.pop_front()
        {
            self.finish(env, pending, TxOutcome::Expired { msdus: 1 });
        }
    }
    /// A transmission ended without delivery: `end` if the plan is spent, a
    /// withdrawal behind a stop or a gate, otherwise `None` to go again.
    fn next_try(
        &self,
        pending: &mut Pending<'id, Id, D, B, O>,
        stopping: bool,
        end: SingleEnd<Ack, Infallible>,
    ) -> Option<SingleOutcome<Ack, Infallible>> {
        if !pending.spend() {
            Some(TxOutcome::Ended(end))
        } else if stopping {
            Some(TxOutcome::Withdrawn(WithdrawCause::Disabled))
        } else if self.closing.is_some() || self.closed {
            Some(TxOutcome::Withdrawn(WithdrawCause::GateClosed))
        } else {
            None
        }
    }
    /// Bring every completion of one service home, then answer the first
    /// fault: an owner a completion cannot return keeps its place with the
    /// fault, and the completions after it still go home.
    pub(super) fn collect(
        &mut self,
        env: &mut impl DriverEnv<'id, Id, D, B, O>,
        sink: &mut Sink<'id, Id, B>,
        stopping: bool,
    ) -> Result<(), Fault> {
        let refused = sink.take_refused();
        let mut first = refused.as_ref().map(|(fault, _)| *fault);
        for completion in sink.completed.drain(..) {
            if let Err(fault) = self.complete(env, completion, stopping) {
                first.get_or_insert(fault);
            }
        }
        match refused {
            // A completion past the queue places goes home as the others.
            Some((_, Refused::Completion(completion))) => {
                if let Err(fault) = self.complete(env, completion, stopping) {
                    first.get_or_insert(fault);
                }
            }
            Some((_, Refused::Removal(removed))) => {
                let _quarantined = core::mem::ManuallyDrop::new(removed);
            }
            None => {}
        }
        if let Some(tbtt) = sink.tbtt.take() {
            env.tbtt(tbtt);
        }
        if let Some(fault) = first {
            return Err(fault);
        }
        if !sink.removed.is_empty() {
            return Err(Fault::RemovalOverflow);
        }
        self.withdraw(env);
        Ok(())
    }
    /// Put a ticket in its own place, or in the first free one: `false`,
    /// with the ticket gone, when no place is free, which the
    /// installation's counts exclude (the ticket owns no memory).
    fn home_ticket(&mut self, ticket: TxTicket<'id, CorePath<Id>>) -> bool {
        let at = ticket.index();
        let place = match self.tickets.get(at) {
            Some(None) => self.tickets.get_mut(at),
            _ => self.tickets.iter_mut().find(|place| place.is_none()),
        };
        match place {
            Some(place) => {
                *place = Some(ticket);
                true
            }
            None => {
                drop(ticket);
                false
            }
        }
    }
    /// Bring one completion's owners home: its queue place, its ticket,
    /// its descriptor and the memory the descriptor held, then the attempt
    /// to its terminal or its next try. An owner whose home is taken or
    /// missing keeps the others home and names the fault; an attempt that
    /// cannot end stays published in `running`.
    fn complete(
        &mut self,
        env: &mut impl DriverEnv<'id, Id, D, B, O>,
        completion: Completion<'id, Id, B>,
        stopping: bool,
    ) -> Result<(), Fault> {
        let Completion {
            queue: free,
            ticket,
            mut slot,
            end,
            rate,
        } = completion;
        let index = queue(free.category());
        let at = index as u8;
        let mut first = None;
        // The queue's one place: a second owner of it lies past the unique
        // queue places, and owns no memory.
        match self.free.get_mut(index) {
            Some(place) if place.is_none() => *place = Some(free),
            _ => {
                drop(free);
                first.get_or_insert(Fault::DuplicateCompletion { queue: at });
            }
        }
        if !self.home_ticket(ticket) {
            first.get_or_insert(Fault::CompletionWithoutPending { queue: at });
        }
        let Some(mut pending) = self.running.get_mut(index).and_then(Option::take) else {
            // No attempt to take the memory: it stays attached to its
            // descriptor, which goes home and keeps it out of every pool.
            self.home_descriptor(index, slot);
            return Err(*first.get_or_insert(Fault::CompletionWithoutPending { queue: at }));
        };
        // The buffer leaves the queue's descriptor and rejoins the right
        // its publication kept.
        let returned = match slot.as_mut().detach() {
            None => Err(Fault::DescriptorMissing { queue: at }),
            Some(memory) => match self.rejoin(&mut pending, memory) {
                Ok(()) => Ok(()),
                Err(memory) => {
                    // The buffer refused it: the memory goes back to the
                    // descriptor it just left, which is free.
                    let _attached = slot.as_mut().attach(memory).is_ok();
                    Err(Fault::ReturnRefused { queue: at })
                }
            },
        };
        if !self.home_descriptor(index, slot) {
            first.get_or_insert(Fault::DescriptorMissing { queue: at });
        }
        if let Err(fault) = returned {
            // Its memory is not back: the attempt stays published.
            self.keep_running(index, pending);
            return Err(*first.get_or_insert(fault));
        }
        use crate::ordinary_tx::OrdinaryTxFailure as Failure;
        // The decision stays with the MPDU, which outlives this attempt:
        // its returned `Sent` may be sent again under a plan of its own.
        if end.status().is_some_and(sets_retry_bit) {
            pending.operation.execute().mpdu.retry = true;
        }
        let outcome = match end {
            PublicationEnd::Expired { msdus } => Some(TxOutcome::Expired { msdus }),
            PublicationEnd::Withdrawn => Some(TxOutcome::Withdrawn(WithdrawCause::Disabled)),
            PublicationEnd::TimedOut | PublicationEnd::Undelivered(Failure::TimedOut) => Some(
                TxOutcome::Indeterminate(IndeterminateCause::HardwareTimeout),
            ),
            PublicationEnd::Undelivered(Failure::KeyRefused) => {
                Some(TxOutcome::Indeterminate(IndeterminateCause::KeyRefused))
            }
            PublicationEnd::Delivered {
                response,
                ack_snr_db,
            } => Some(TxOutcome::Ended(SingleEnd::Delivered {
                air: pending.transmitted(rate),
                response: match response {
                    PublicationResponse::Unsolicited => SingleResponse::Unsolicited,
                    PublicationResponse::Ack => SingleResponse::Ack(Ack::from_sample(ack_snr_db)),
                    PublicationResponse::BlockAck(report) => SingleResponse::BlockAck {
                        report,
                        ack: Ack::from_sample(ack_snr_db),
                    },
                },
            })),
            PublicationEnd::Undelivered(Failure::AckTimeout) => {
                let air = pending.transmitted(rate);
                self.next_try(&mut pending, stopping, SingleEnd::AckTimeout(air))
            }
            PublicationEnd::Undelivered(Failure::CtsTimeout) => {
                let end = SingleEnd::CtsTimeout(pending.air);
                self.next_try(&mut pending, stopping, end)
            }
            PublicationEnd::Collision | PublicationEnd::Undelivered(Failure::Collision) => {
                let end = SingleEnd::ContentionLimit(pending.air);
                self.next_try(&mut pending, stopping, end)
            }
        };
        if let Some(outcome) = outcome {
            self.finish(env, pending, outcome);
        } else {
            // A queue holds every ticket's attempt (`QUEUE_DEPTH`), so this
            // push has room; refused, the retry stays where it was.
            if let Err(pending) = self.enqueue(index, pending, true) {
                self.keep_running(index, pending);
                first.get_or_insert(Fault::QueueFull { queue: at });
            }
        }
        first.map_or(Ok(()), Err)
    }
    /// Queue `pending` at the head (`front`) or the tail of queue `index`;
    /// refused, it comes back.
    #[allow(
        clippy::result_large_err,
        reason = "a refused attempt comes back by value"
    )]
    pub(super) fn enqueue(
        &mut self,
        index: usize,
        pending: Pending<'id, Id, D, B, O>,
        front: bool,
    ) -> Result<(), Pending<'id, Id, D, B, O>> {
        #[cfg(test)]
        if self
            .refuse_for_test
            .take_if(|refusal| *refusal == TestRefusal::QueuePush)
            .is_some()
        {
            return Err(pending);
        }
        match self.queues.get_mut(index) {
            Some(queue) if front => queue.push_front(pending),
            Some(queue) => queue.push_back(pending),
            None => Err(pending),
        }
    }
    /// The memory a completion handed back rejoins its buffer; refused, it
    /// comes back.
    fn rejoin(
        &mut self,
        pending: &mut Pending<'id, Id, D, B, O>,
        memory: crate::mac::tx::TxBacking<B>,
    ) -> Result<(), crate::mac::tx::TxBacking<B>> {
        #[cfg(test)]
        if self
            .refuse_for_test
            .take_if(|refusal| *refusal == TestRefusal::Return)
            .is_some()
        {
            return Err(memory);
        }
        pending.operation.execute().mpdu.buffer.dma.complete(memory)
    }
    /// Put a descriptor back in its queue's place: `false`, with the
    /// descriptor kept out of every pool, when the place is taken.
    fn home_descriptor(&mut self, index: usize, slot: Pin<&'static mut TxSlot<B>>) -> bool {
        match self.descriptors.get_mut(index) {
            Some(place) if place.is_none() => {
                *place = Some(slot);
                true
            }
            _ => false,
        }
    }
    /// Keep an attempt a completion could not end in its queue's published
    /// place, which the completion just emptied.
    fn keep_running(&mut self, index: usize, pending: Pending<'id, Id, D, B, O>) {
        if let Some(place) = self.running.get_mut(index) {
            *place = Some(pending);
        }
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
            // The caller holds every unit it may: this one goes back to
            // the ring, unread, beside the fault.
            device.recycle(unit);
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
        env: &mut impl DriverEnv<'id, Id, D, B, O>,
        index: usize,
        mut pending: Pending<'id, Id, D, B, O>,
    ) -> Result<(), Fault> {
        let at = index as u8;
        let refused = |step| Fault::Publish { queue: at, step };
        let stage = pending.plan.first();
        let (rate, protection) = (
            stage.rate,
            if !pending.published {
                pending.protection
            } else {
                stage.protection
            },
        );
        let role = pending.role();
        // Everything that can refuse without an owner in hand comes first:
        // the MPDU goes back to the head of its queue and nothing was taken.
        // An open core publishes; a configuration barrier of the other
        // interface lets this one's attempt through.
        let core = match self.core.view() {
            CoreView::Open(core) => Some(core),
            CoreView::ConfigureOpen(view) => view.other_interface(role).ok(),
            _ => None,
        };
        let checked = match (
            core,
            pending.admission(),
            admitted_key(&self.keys, pending.key()),
        ) {
            (Some(core), Some(admitted_at), Some(key)) => Ok((core, admitted_at, key)),
            (Some(_), None, _) => Err(Unpublished::Admission),
            (Some(_), Some(_), None) => Err(Unpublished::Key),
            (None, _, _) => Err(Unpublished::CoreView),
        };
        #[cfg(oer_fault_injection)]
        let checked = checked.and_then(|checked| {
            if super::debug::publication_refused() {
                Err(Unpublished::Key)
            } else {
                Ok(checked)
            }
        });
        let refusal = match &checked {
            Err(refusal) => Some(*refusal),
            Ok((_, _, key)) => owner_free_refusal(
                self.observations.is_some(),
                &self.tickets,
                self.descriptors.get(index).and_then(Option::as_ref),
                self.free.get(index).is_some_and(Option::is_some),
                &mut pending,
                *key,
            ),
        };
        if let Some(refusal) = refusal {
            return Err(self.back_to_queue(env, index, pending, refusal));
        }
        let Ok((mut core, admitted_at, key_handle)) = checked else {
            return Err(self.requeue(env, index, pending, refused(PublishStep::Values)));
        };
        // The registers first: a device that holds none takes nothing of
        // the attempt, which goes back to the head of its queue.
        let hardware = match device.hardware() {
            Ok(hardware) => hardware,
            Err(fault) => return Err(self.requeue(env, index, pending, fault)),
        };
        let Some(observations) = self.observations.as_mut() else {
            return Err(self.requeue(env, index, pending, refused(PublishStep::Observations)));
        };
        let backoff_slots = core.draw_backoff(role, pending.category, observations);
        let Some(place) = self.tickets.iter_mut().find(|place| place.is_some()) else {
            return Err(self.requeue(env, index, pending, refused(PublishStep::Ticket)));
        };
        let Some(core_ticket) = place.take() else {
            return Err(self.requeue(env, index, pending, refused(PublishStep::Ticket)));
        };
        let admission = match key_handle {
            Some(Installed::Transmit(key)) => match core.admit_key(key, core_ticket) {
                Ok(key_use) => TxAdmission::Protected(key_use),
                Err(refusal) => {
                    *place = Some(refusal.ticket);
                    return Err(self.requeue(
                        env,
                        index,
                        pending,
                        refused(PublishStep::KeyAdmission),
                    ));
                }
            },
            Some(Installed::Receive(_)) => {
                *place = Some(core_ticket);
                return Err(self.requeue(env, index, pending, refused(PublishStep::ReceiveKey)));
            }
            None => TxAdmission::Open(core_ticket),
        };
        // The descriptor before the buffer's lease: a refusal here holds the
        // admission alone, which goes back as a ticket.
        let Some(mut slot) = self.descriptors.get_mut(index).and_then(Option::take) else {
            return Err(self.unadmit(
                hardware,
                env,
                index,
                (pending, admission),
                None,
                refused(PublishStep::Descriptor),
            ));
        };
        if slot.state() != crate::mac::tx::TxDmaState::Free || slot.attached() {
            return Err(self.unadmit(
                hardware,
                env,
                index,
                (pending, admission),
                Some(slot),
                refused(PublishStep::DescriptorBusy),
            ));
        }
        let mpdu = pending.operation.execute().mpdu;
        // The hardware takes the memory through the queue's descriptor; the
        // buffer keeps its return right.
        let Some(lease) = mpdu.buffer.dma.publish() else {
            return Err(self.unadmit(
                hardware,
                env,
                index,
                (pending, admission),
                Some(slot),
                refused(PublishStep::Buffer),
            ));
        };
        if let Err(lease) = slot.as_mut().attach(lease) {
            // The lease goes back to the buffer it left, which is published:
            // its return cannot refuse.
            let _returned = mpdu.buffer.dma.complete(lease).is_ok();
            return Err(self.unadmit(
                hardware,
                env,
                index,
                (pending, admission),
                Some(slot),
                refused(PublishStep::Attach),
            ));
        }
        if slot.state() != crate::mac::tx::TxDmaState::Free {
            return self.unwind(
                hardware,
                env,
                index,
                (pending, admission),
                slot,
                PublishStep::DescriptorBusy,
            );
        }
        // Free a moment ago, in the same call: the slot lends its memory.
        let TxSlotAccess::Free(mut memory) = slot.access() else {
            // Excluded by the read just above. The descriptor keeps the
            // memory out of every pool; the other owners go back.
            return Err(self.unadmit(
                hardware,
                env,
                index,
                (pending, admission),
                None,
                refused(PublishStep::DescriptorBusy),
            ));
        };
        if !mpdu.published {
            // A new MSDU: the descriptor's exchange record starts over.
            memory.reset_work();
            if let (Some(Installed::Transmit(key)), Some(pn)) = (key_handle, mpdu.pn) {
                let header = memory.buffer_mut().map(|buffer| {
                    core.write_ccmp_header(key, buffer, mpdu.layout.header_len(), pn)
                });
                let refusal = match header {
                    Ok(Ok(_)) => None,
                    Ok(Err(_)) => Some(PublishStep::CcmpHeader),
                    Err(_) => Some(PublishStep::HeaderBuffer),
                };
                if let Some(step) = refusal {
                    return self.unwind(
                        hardware,
                        env,
                        index,
                        (pending, admission),
                        memory.into_slot(),
                        step,
                    );
                }
            }
        }
        #[cfg(test)]
        if core::mem::take(&mut self.refuse_values_for_test) {
            return self.unwind(
                hardware,
                env,
                index,
                (pending, admission),
                memory.into_slot(),
                PublishStep::Values,
            );
        }
        // The queue before the values that hold the memory: without it the
        // attempt unwinds whole.
        let (Some(free), Some(running)) = (self.free.get_mut(index), self.running.get_mut(index))
        else {
            return self.unwind(
                hardware,
                env,
                index,
                (pending, admission),
                memory.into_slot(),
                PublishStep::Queue,
            );
        };
        let Some(queue) = free.take() else {
            return self.unwind(
                hardware,
                env,
                index,
                (pending, admission),
                memory.into_slot(),
                PublishStep::Queue,
            );
        };
        let values = MpduValues::checked(
            memory,
            core_mac::MpduParameters {
                role,
                admitted_at,
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
                // A data or management frame carries the Retry bit once an
                // earlier transmission may have reached a receiver; a
                // BlockAckReq, a control frame, never does (IEEE Std
                // 802.11-2020, 9.2.4.1.5).
                retry: mpdu.retry && mpdu.layout.kind() != MpduKind::BlockAckRequest,
            },
        );
        let values = match values {
            Ok(values) => values,
            Err(refused) => {
                if let Some(place) = self.free.get_mut(index) {
                    *place = Some(queue);
                }
                let slot = refused.slot.into_slot();
                return self.unwind(
                    hardware,
                    env,
                    index,
                    (pending, admission),
                    slot,
                    PublishStep::Values,
                );
            }
        };
        let before = (mpdu.published, pending.published);
        mpdu.published = true;
        pending.published = true;
        #[cfg(all(oer_trace, oer_debug_control))]
        let publication = super::trace::Publication {
            ticket: pending.operation.ticket().index(),
            buffer: pending.operation.execute().mpdu.buffer.index(),
        };
        // Durable backing owns the indivisible operation before publication.
        *running = Some(pending);
        #[cfg(test)]
        let submitted = if self
            .refuse_for_test
            .take_if(|refusal| *refusal == TestRefusal::Submit)
            .is_some()
        {
            Err(crate::lower_mac::SubmitFault {
                queue,
                fault: crate::lower_mac::LowerMacFault::KeyUseMismatch,
                slot: values.into_slot_for_test(),
                admission,
            })
        } else {
            core.submit_checked(hardware, queue, admission, values)
        };
        #[cfg(not(test))]
        let submitted = core.submit_checked(hardware, queue, admission, values);
        if let Err(failed) = submitted {
            let crate::lower_mac::SubmitFault {
                queue,
                fault,
                slot,
                admission,
            } = failed;
            let fault = Fault::Core(fault);
            // The core refused the publication: its owners go back as at a
            // refusal before it, and the attempt to the head of its queue.
            if let Some(place) = self.free.get_mut(index) {
                *place = Some(queue);
            }
            let Some(mut pending) = self.running.get_mut(index).and_then(Option::take) else {
                return Err(fault);
            };
            (
                pending.operation.execute().mpdu.published,
                pending.published,
            ) = before;
            return Err(self.put_back(hardware, env, index, (pending, admission), slot, fault));
        }
        #[cfg(all(oer_trace, oer_debug_control))]
        oer_portable_trace_recorder::emit(&publication);
        Ok(())
    }
    /// Put `pending` back at the head of its queue with `fault`: nothing of
    /// it was published. A queue without room ends the attempt and answers
    /// [`Fault::QueueFull`].
    fn requeue(
        &mut self,
        env: &mut impl DriverEnv<'id, Id, D, B, O>,
        index: usize,
        pending: Pending<'id, Id, D, B, O>,
        fault: Fault,
    ) -> Fault {
        match self.back_to_queue(env, index, pending, Unpublished::CoreView) {
            Fault::Unpublished(_) => fault,
            full => full,
        }
    }
    /// Give a refused publication's admission back as a ticket and its
    /// descriptor back to its queue's place, then the attempt to the head of
    /// its queue: `fault` names the refusal. An admission the core does not
    /// take back is that fault instead.
    fn unadmit(
        &mut self,
        hardware: &mut D::Hardware,
        env: &mut impl DriverEnv<'id, Id, D, B, O>,
        index: usize,
        (pending, admission): (Pending<'id, Id, D, B, O>, TxAdmission<'id, CorePath<Id>>),
        slot: Option<Pin<&'static mut TxSlot<B>>>,
        fault: Fault,
    ) -> Fault {
        if let Some(slot) = slot {
            // A taken place lies past the queue's one descriptor: the
            // descriptor stays out of every pool.
            let _home = self.home_descriptor(index, slot);
        }
        let fault = match self.returned_ticket(hardware, index, pending.role(), admission, fault) {
            Ok(()) => fault,
            Err(cause) => cause,
        };
        self.requeue(env, index, pending, fault)
    }
    /// A publication the core refused at its submission: the memory
    /// rejoins the buffer when the descriptor gives it up (otherwise it
    /// stays attached, and the descriptor keeps it from every pool), then
    /// every owner goes back as at a refusal before the submission.
    fn put_back(
        &mut self,
        hardware: &mut D::Hardware,
        env: &mut impl DriverEnv<'id, Id, D, B, O>,
        index: usize,
        (mut pending, admission): (Pending<'id, Id, D, B, O>, TxAdmission<'id, CorePath<Id>>),
        mut slot: Pin<&'static mut TxSlot<B>>,
        fault: Fault,
    ) -> Fault {
        if let Some(memory) = slot.as_mut().detach()
            && let Err(memory) = pending.operation.execute().mpdu.buffer.dma.complete(memory)
        {
            // The buffer is published, so this return does not refuse;
            // refused, the memory goes back to the free descriptor.
            let _attached = slot.as_mut().attach(memory).is_ok();
        }
        self.unadmit(
            hardware,
            env,
            index,
            (pending, admission),
            Some(slot),
            fault,
        )
    }
    /// Put `pending` back at the head of its queue: nothing of it was
    /// published, and a stop withdraws it there.
    fn back_to_queue(
        &mut self,
        env: &mut impl DriverEnv<'id, Id, D, B, O>,
        index: usize,
        pending: Pending<'id, Id, D, B, O>,
        refusal: Unpublished,
    ) -> Fault {
        let queue = index as u8;
        // The attempt left this queue in the same call, and a queue holds
        // every ticket's attempt (`QUEUE_DEPTH`): the push has room.
        match self.enqueue(index, pending, true) {
            Ok(()) => Fault::Unpublished(refusal),
            Err(pending) => {
                // Refused all the same: the attempt ends withdrawn with
                // every owner, and the fault stops the port.
                self.finish(env, pending, TxOutcome::Withdrawn(WithdrawCause::Disabled));
                Fault::QueueFull { queue }
            }
        }
    }
    /// A publication refused after its buffer joined the descriptor, which
    /// the hardware has not seen: the memory rejoins the buffer as at a
    /// completion, the admission gives back its key use and ticket and the
    /// descriptor takes its place. The refusal is the attempt's own (its
    /// values or its header do not fit the transmit path), so it would
    /// refuse again: the attempt ends
    /// [`WithdrawCause::Unpublishable`] with every owner, and the port runs
    /// on. An owner that cannot go back keeps all of them with the
    /// quarantine.
    fn unwind(
        &mut self,
        hardware: &mut D::Hardware,
        env: &mut impl DriverEnv<'id, Id, D, B, O>,
        index: usize,
        (mut pending, admission): (Pending<'id, Id, D, B, O>, TxAdmission<'id, CorePath<Id>>),
        mut slot: Pin<&'static mut TxSlot<B>>,
        step: PublishStep,
    ) -> Result<(), Fault> {
        let terminal = Fault::Publish {
            queue: index as u8,
            step,
        };
        let Some(memory) = slot.as_mut().detach() else {
            // The memory stays attached to its descriptor: every owner goes
            // back to its place with the fault.
            return Err(self.unadmit(
                hardware,
                env,
                index,
                (pending, admission),
                Some(slot),
                terminal,
            ));
        };
        {
            let buffer = &mut pending.operation.execute().mpdu.buffer;
            // The buffer is published, so its memory's return cannot
            // refuse.
            let _returned = buffer.dma.complete(memory).is_ok();
        }
        let _home = self.home_descriptor(index, slot);
        if let Err(fault) =
            self.returned_ticket(hardware, index, pending.role(), admission, terminal)
        {
            return Err(self.requeue(env, index, pending, fault));
        }
        self.finish(
            env,
            pending,
            TxOutcome::Withdrawn(WithdrawCause::Unpublishable),
        );
        Ok(())
    }
    /// Return the ticket of an admission never published to its place, its
    /// key use returned, through the view the publication of `role` took
    /// (an open core, or the other interface's configuration barrier). A
    /// removal takes its key out of `keys` first, so a publication's key
    /// has none waiting and the sink stays empty. An admission the core
    /// does not take back waits in `withheld` with `terminal`.
    fn returned_ticket(
        &mut self,
        hardware: &mut D::Hardware,
        index: usize,
        role: VifRole,
        admission: TxAdmission<'id, CorePath<Id>>,
        terminal: Fault,
    ) -> Result<(), Fault> {
        let core = match self.core.view() {
            CoreView::Open(core) => Some(core),
            CoreView::ConfigureOpen(view) => view.other_interface(role).ok(),
            _ => None,
        };
        let Some(mut core) = core else {
            self.withhold(Withheld::Admission(admission));
            return Err(terminal);
        };
        let mut sink = Sink::default();
        let ticket = match core.return_admission(hardware, admission, &mut sink) {
            Ok(ticket) => ticket,
            Err(quarantined) => {
                self.withhold(Withheld::Quarantined(quarantined));
                return Err(terminal);
            }
        };
        let contract = sink.fault();
        let quiet = sink.removed.is_empty() && contract.is_none();
        if !self.home_ticket(ticket) {
            return Err(Fault::Publish {
                queue: index as u8,
                step: PublishStep::Ticket,
            });
        }
        if quiet {
            return Ok(());
        }
        // A removal this key's return finished, which the order above
        // excludes: its tokens own no memory.
        drop(sink);
        Err(contract.unwrap_or(terminal))
    }
    /// Keep an admission the core did not take back, in a place of its
    /// own: it holds one of the installation's tickets, so a place is free.
    fn withhold(&mut self, admission: Withheld<'id, Id>) {
        if let Some(place) = self.withheld.iter_mut().find(|place| place.is_none()) {
            *place = Some(admission);
        } else {
            // More admissions than tickets, which the counts exclude: its
            // tokens own no memory.
            let _excess = admission;
        }
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
        let index = self
            .keys
            .iter()
            .position(|entry| entry.as_ref().is_some_and(|(key, _)| *key == identity))
            .ok_or(Fault::KeySlotMissing(KeyOp::Remove))?;
        // The registers and the core's command view first: a removal they
        // refuse leaves the installed key in its place.
        let hardware = device.hardware()?;
        let mut view = self
            .core
            .view()
            .commands()
            .ok_or(Fault::CoreViewUnavailable(CoreOp::Command))?;
        let (_, installed) = self
            .keys
            .get_mut(index)
            .and_then(Option::take)
            .ok_or(Fault::KeySlotMissing(KeyOp::Remove))?;
        let mut sink = Sink::default();
        match installed {
            Installed::Transmit(key) => view.remove_checked_key(hardware, key, &mut sink),
            Installed::Receive(key) => view.remove_checked_key(hardware, key, &mut sink),
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
                    activity: env
                        .shared()
                        .key_live
                        .get(17 + usize::from(slot.index()))
                        .ok_or(Fault::GroupKeyIdRange)?,
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
                        oer_portable_trace_recorder::emit(&trace::GroupRollback {
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

/// What refuses `pending` with no owner in hand: everything the publication's
/// steps would find missing, read before any of them takes one.
fn owner_free_refusal<'id, Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations>(
    observations: bool,
    tickets: &[Option<TxTicket<'id, CorePath<Id>>>],
    descriptor: Option<&Pin<&'static mut TxSlot<B>>>,
    queue: bool,
    pending: &mut Pending<'id, Id, D, B, O>,
    key: Option<&Installed<'id, Id>>,
) -> Option<Unpublished> {
    let step = if !observations {
        PublishStep::Observations
    } else if !tickets.iter().any(Option::is_some) {
        PublishStep::Ticket
    } else if matches!(key, Some(Installed::Receive(_))) {
        PublishStep::ReceiveKey
    } else if pending
        .operation
        .execute()
        .mpdu
        .buffer
        .dma
        .memory_mut()
        .is_none()
    {
        PublishStep::Buffer
    } else if descriptor.is_none() {
        PublishStep::Descriptor
    } else if descriptor
        .is_some_and(|slot| slot.state() != crate::mac::tx::TxDmaState::Free || slot.attached())
    {
        PublishStep::DescriptorBusy
    } else if !queue {
        PublishStep::Queue
    } else {
        return None;
    };
    Some(Unpublished::Step(step))
}

/// The attempt's key: `Some(None)` for an open attempt, `None` when the key
/// it names is not installed.
fn admitted_key<'a, 'id, Id: SlotIdentity>(
    keys: &'a [Option<(RawKeyIdentity, Installed<'id, Id>)>],
    identity: Option<RawKeyIdentity>,
) -> Option<Option<&'a Installed<'id, Id>>> {
    let Some(identity) = identity else {
        return Some(None);
    };
    keys.iter()
        .flatten()
        .find(|(key, _)| *key == identity)
        .map(|(_, key)| Some(key))
}

/// Move the element at `at` of the sequence `front` then `back` to its
/// head by adjacent swaps: the elements before it keep their order one
/// place later, the ones after it stay. An `at` past the end moves nothing.
fn to_head<T>(front: &mut [T], back: &mut [T], at: usize) {
    let mut at = at;
    if let Some(within) = at.checked_sub(front.len()) {
        // In `back`: to its head, then across to the end of `front`.
        bubble(back, within);
        if within < back.len()
            && let (Some(last), Some(first)) = (front.last_mut(), back.first_mut())
        {
            core::mem::swap(last, first);
            at = front.len().saturating_sub(1);
        } else {
            return;
        }
    }
    bubble(front, at);
}
/// Move `slice[at]` to the head of `slice` by adjacent swaps.
fn bubble<T>(slice: &mut [T], at: usize) {
    let mut at = at;
    while let Some(before) = at.checked_sub(1) {
        if let Some([earlier, later]) = slice.get_mut(before..=at) {
            core::mem::swap(earlier, later);
        } else {
            return;
        }
        at = before;
    }
}

#[cfg(test)]
mod tests {
    use super::to_head;

    /// The deque `front` then `back` after moving element `at` to its head.
    fn moved(front: &[u8], back: &[u8], at: usize) -> std::vec::Vec<u8> {
        let (mut front, mut back) = (front.to_vec(), back.to_vec());
        to_head(&mut front, &mut back, at);
        front.into_iter().chain(back).collect()
    }

    #[test]
    fn a_late_attempt_moves_to_the_head_and_the_others_keep_their_order() {
        assert_eq!(moved(&[1, 2, 3], &[4, 5], 0), [1, 2, 3, 4, 5]);
        assert_eq!(moved(&[1, 2, 3], &[4, 5], 2), [3, 1, 2, 4, 5]);
        assert_eq!(moved(&[1, 2, 3], &[4, 5], 3), [4, 1, 2, 3, 5]);
        assert_eq!(moved(&[1, 2, 3], &[4, 5], 4), [5, 1, 2, 3, 4]);
        assert_eq!(moved(&[], &[4, 5], 1), [5, 4]);
        assert_eq!(moved(&[1, 2], &[], 1), [2, 1]);
        // Past the end, nothing moves.
        assert_eq!(moved(&[1, 2], &[3], 3), [1, 2, 3]);
    }
}
