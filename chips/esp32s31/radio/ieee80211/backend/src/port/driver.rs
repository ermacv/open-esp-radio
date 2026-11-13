use super::*;
use super::{
    installation::PortCoreOwner,
    keys::{
        AccessPointGroupCommand, ApPermission, GroupInstallation, Installation, Installed,
        KeyPlace, RawKeyIdentity, RemovalRights, Removed, Removing, StaPermission,
        StationGroupCommand, Waiting, WaitingRemoval,
    },
};
use crate::mac::crypto::{CryptoKeyError, StaGroupCcmpReplaceError as ReplaceError};
use crate::ordinary_tx::sets_retry_bit;
use oer_protocols_ieee80211_wire::qos::WmmAccessCategory;

pub(super) mod arena;
pub(super) use arena::DataArenas;
pub(super) mod core_views;
#[cfg(oer_debug_control)]
mod key_replacement;
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
    /// The descriptor's work before the publication now on the air: what
    /// that publication adds is its cost.
    pub try_work: Option<crate::mac::tx_work::MacTxWork>,
    /// What the plan's publications cost so far, by the backend's model
    /// ([`TxEstimate`]): its terminal carries it.
    pub cost: TxEstimate,
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
        self.operation.payload().buffer.admission()
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
/// One ended attempt the core handed to the port's sink.
pub(super) type Completion<'id, Id, const B: usize> =
    crate::lower_mac::TxCompletion<'id, 'static, B, CorePath<Id>, TX_TICKETS>;
/// What one core call hands the port. Completions it has no room for, and
/// every removal receipt, the sink refuses: the core keeps them whole in
/// their own places, a completion offered again and a receipt taken by its
/// removal's place ([`Driver::finished_removal`]).
pub(super) struct Sink<'id, Id: SlotIdentity, const B: usize> {
    pub(super) completed: heapless::Vec<Completion<'id, Id, B>, 4>,
    /// Whether this sink takes completions: a call that collects none
    /// leaves them with the core.
    takes_completions: bool,
    tbtt: Option<TbttEvent>,
}
impl<Id: SlotIdentity, const B: usize> Sink<'_, Id, B> {
    /// The sink of a call whose results the port does not collect: the
    /// core keeps every completion and receipt for a later offer.
    pub(super) fn refusing() -> Self {
        Self {
            completed: heapless::Vec::new(),
            takes_completions: false,
            tbtt: None,
        }
    }
    /// A sink that takes the core's completions.
    const fn taking() -> Self {
        Self {
            completed: heapless::Vec::new(),
            takes_completions: true,
            tbtt: None,
        }
    }
}
impl<'id, Id: SlotIdentity, const B: usize> LowerMacSink<'id, 'static, B, CorePath<Id>, TX_TICKETS>
    for Sink<'id, Id, B>
{
    fn tx_completed(
        &mut self,
        completion: Completion<'id, Id, B>,
    ) -> Result<(), Completion<'id, Id, B>> {
        // One completion per hardware queue per service; the core keeps
        // one it has no room for.
        if !self.takes_completions {
            return Err(completion);
        }
        self.completed.push(completion)
    }
    fn key_removed<S: core_mac::RemovableSlot<'id, CorePath<Id>>>(
        &mut self,
        removed: KeyRemoved<'id, CorePath<Id>, S>,
    ) -> Result<(), KeyRemoved<'id, CorePath<Id>, S>> {
        // The core keeps it in its removal's place, where the removal that
        // waits for it takes it: no other command answers with it.
        Err(removed)
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
    /// The call's right to hand out one item, which a step obtains before
    /// it takes the owner the item carries ([`Self::vacancy`]).
    type Vacancy;
    /// The right to hand out one item, while the call has it: without it a
    /// step takes no owner, and the rest stay with the driver.
    fn vacancy(&mut self) -> Option<Self::Vacancy>;
    /// Give back a right a step did not use.
    fn vacate(&mut self, vacancy: Self::Vacancy);
    /// Whether the radio stops: queued attempts end withdrawn.
    fn stopping(&self) -> bool;
    /// The MSDU lifetime of `category` on interface `vif`.
    fn lifetime(&self, vif: usize, category: WmmAccessCategory) -> Duration;
    /// Count MSDUs that expired before publication.
    fn count_expired(&mut self, msdus: u32);
    /// A single MPDU's terminal, at its ticket's `place`, handed out under
    /// `vacancy`.
    fn tx_completed(
        &mut self,
        vacancy: Self::Vacancy,
        place: u8,
        completed: brand::OwnedTxCompleted<'id, Id, D, B, O>,
    );
    /// A TBTT of the station's schedule.
    fn tbtt(&mut self, event: TbttEvent);
    /// A unit a receive cut drained, ahead of the cut's terminal, handed
    /// out under `vacancy`.
    fn received(
        &mut self,
        vacancy: Self::Vacancy,
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
    /// Whether the call stopped on an item it could not keep.
    fn halted(&self) -> bool;
}

/// The attempts one access category's queue holds. Every queued or
/// published attempt holds one of the installation's [`TX_TICKETS`]
/// tickets, so a queue that holds them all never refuses a push.
pub(super) const QUEUE_DEPTH: usize = TX_TICKETS;
/// The transmit queues an installation keeps, one per access category.
pub const TX_QUEUES: usize = 4;
const _: () = {
    if QUEUE_DEPTH < TX_TICKETS {
        panic!("each transmit queue holds every ticket's attempt");
    }
};

/// A step a test refuses, which the installation's counts exclude.
/// What one publication of a single MPDU cost by the backend's model, as
/// its completion `end` tells it: the work it added to its descriptor's
/// record (`before` to `after`) and its response, as an arena's publication
/// is costed. A completion of no publication costs nothing; one that did not
/// reach the air (a collision, a withdrawal, an expiry) nothing; one whose
/// air the model does not describe (a hardware timeout, a refused key, a
/// protection exchange left unanswered) is unknown, never nothing.
fn attempt_cost(
    before: Option<crate::mac::tx_work::MacTxWork>,
    after: crate::mac::tx_work::MacTxWork,
    rate: PhyRate,
    end: &PublicationEnd,
) -> TxEstimate {
    use crate::lower_mac::arena::cost::{Response, publication_cost};
    use crate::ordinary_tx::OrdinaryTxFailure as Failure;
    let Some(before) = before else {
        return TxEstimate::NONE;
    };
    let response = match end {
        PublicationEnd::Expired { .. }
        | PublicationEnd::Withdrawn
        | PublicationEnd::Collision
        | PublicationEnd::Undelivered(Failure::Collision) => return TxEstimate::NONE,
        PublicationEnd::TimedOut
        | PublicationEnd::Undelivered(
            Failure::TimedOut | Failure::KeyRefused | Failure::CtsTimeout,
        ) => return TxEstimate::Unknown,
        PublicationEnd::Delivered { response, .. } => match response {
            PublicationResponse::Unsolicited => Response::None,
            PublicationResponse::Ack => Response::Ack,
            PublicationResponse::BlockAck(_) => Response::BlockAck,
        },
        PublicationEnd::Undelivered(Failure::AckTimeout) => Response::Ack,
    };
    let Ok(rate) = crate::mac::tx::TxPhyRate::try_from(rate) else {
        return TxEstimate::Unknown;
    };
    publication_cost(Some(before), Some(after), rate, response)
}

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

/// What is known of an attempt that waits beside the lease its buffer
/// refused back.
#[derive(Clone, Copy, Debug)]
pub(super) enum Left {
    /// Its publication ended so, at this rate.
    Ended { end: PublicationEnd, rate: PhyRate },
    /// It was never published: the core refused it, or its publication
    /// unwound before the air.
    Unpublished,
}

/// The queue places at home, each with its queue's ordinary descriptor.
///
/// A single MPDU's publication takes a place and its descriptor together
/// and gives them back together, at its completion or its refusal: the
/// descriptor's place is the one its queue place left, so it is vacant.
/// A data arena takes a place alone; its queue's descriptor stays home.
pub(super) struct QueueHomes<'id, Id: SlotIdentity, const B: usize> {
    pub(super) bank: crate::lower_mac::QueueBank<'id, CorePath<Id>>,
    descriptors: [Option<Pin<&'static mut TxSlot<B>>>; TX_DESCRIPTORS],
}
impl<'id, Id: SlotIdentity, const B: usize> QueueHomes<'id, Id, B> {
    const fn empty() -> Self {
        Self {
            bank: crate::lower_mac::QueueBank::empty(),
            descriptors: [const { None }; TX_DESCRIPTORS],
        }
    }
    fn new(
        queues: [FreeQueue<'id, CorePath<Id>>; 4],
        single: [Pin<&'static mut TxSlot<B>>; TX_DESCRIPTORS],
    ) -> Self {
        Self {
            bank: crate::lower_mac::QueueBank::new(queues),
            descriptors: single.map(Some),
        }
    }
    /// The descriptor place of the queue of `category`.
    fn descriptor_place(
        &mut self,
        category: WmmAccessCategory,
    ) -> &mut Option<Pin<&'static mut TxSlot<B>>> {
        let [voice, video, best_effort, background] = &mut self.descriptors;
        match LegacyTxQueue::from_access_category(category) {
            LegacyTxQueue::Voice => voice,
            LegacyTxQueue::Video => video,
            LegacyTxQueue::BestEffort => best_effort,
            LegacyTxQueue::Background => background,
        }
    }
    /// The descriptor of the queue of `category`, while it is home.
    fn descriptor(&self, category: WmmAccessCategory) -> Option<&Pin<&'static mut TxSlot<B>>> {
        self.descriptors
            .get(queue(category))
            .and_then(Option::as_ref)
    }
    /// The place of the queue of `category` with its descriptor, when both
    /// are home; otherwise neither leaves.
    fn take_paired(
        &mut self,
        category: WmmAccessCategory,
    ) -> Option<(FreeQueue<'id, CorePath<Id>>, Pin<&'static mut TxSlot<B>>)> {
        self.descriptor(category)?;
        let queue = self.bank.take(category)?;
        match self.descriptor_place(category).take() {
            Some(slot) => Some((queue, slot)),
            None => {
                self.bank.put(queue);
                None
            }
        }
    }
    /// Give a place back with the descriptor it left with.
    fn put_paired(
        &mut self,
        queue: FreeQueue<'id, CorePath<Id>>,
        slot: Pin<&'static mut TxSlot<B>>,
    ) {
        *self.descriptor_place(queue.category()) = Some(slot);
        self.bank.put(queue);
    }
    /// Give a place back alone: its descriptor stayed home, or is gone
    /// with its memory.
    fn put(&mut self, queue: FreeQueue<'id, CorePath<Id>>) {
        self.bank.put(queue);
    }
    fn is_home(&self, category: WmmAccessCategory) -> bool {
        self.bank.is_home(category)
    }
    pub(super) fn is_home_at(&self, index: usize) -> bool {
        self.bank.is_home_at(index)
    }
    /// Whether every descriptor is home and idle.
    fn descriptors_idle(&self) -> bool {
        self.descriptors.iter().all(|slot| {
            slot.as_ref().is_some_and(|slot| {
                slot.state() == crate::mac::tx::TxDmaState::Free && !slot.attached()
            })
        })
    }
    /// Every descriptor, when each is home; otherwise all stay.
    fn take_descriptors(&mut self) -> Option<[Pin<&'static mut TxSlot<B>>; TX_DESCRIPTORS]> {
        if self.descriptors.iter().any(Option::is_none) {
            return None;
        }
        let [Some(a), Some(b), Some(c), Some(d)] = self.descriptors.each_mut().map(Option::take)
        else {
            return None;
        };
        Some([a, b, c, d])
    }
}

/// A station group key replacement whose rollback failed: the command's
/// owners, kept with the port until its cleanup removes the key.
pub(super) struct RetainedGroup<'id, Id: SlotIdentity> {
    pub(super) permission: StaPermission<'id, Id>,
    pub(super) key: GroupKey<'id, Id, Station>,
    /// Whether the cleanup's one write, the entry's clear, was issued.
    pub(super) clear: GroupClear,
}

/// Where the cleanup of a retained replacement stands with its one write.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum GroupClear {
    /// The entry's clear is not issued yet.
    NotIssued,
    /// The clear was issued: the cleanup only reads the table from now on.
    Issued,
}

pub(super) struct Driver<'id, Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> {
    pub(super) core: PortCoreOwner<'id, Id, D, B>,
    /// The tickets at home, each at its own place.
    pub(super) tickets: crate::lower_mac::TicketBank<'id, CorePath<Id>, TX_TICKETS>,
    /// Refuse the next publication's values, past its buffer's attachment.
    #[cfg(test)]
    pub(super) refuse_values_for_test: bool,
    /// Refuse the next publication as a missing key, on the path the
    /// image's one-shot fault takes, with the key installed.
    #[cfg(test)]
    pub(super) refuse_publication_for_test: bool,
    /// Refuse the next step of this kind, which the counts exclude.
    #[cfg(test)]
    pub(super) refuse_for_test: Option<TestRefusal>,
    /// The queue places at home, each at its own place with its
    /// descriptor.
    pub(super) free: QueueHomes<'id, Id, B>,
    pub(super) queues: [heapless::Deque<Pending<'id, Id, D, B, O>, QUEUE_DEPTH>; TX_QUEUES],
    pub(super) running: [Option<Pending<'id, Id, D, B, O>>; TX_QUEUES],
    /// What is known of each attempt that waits in `running` beside the
    /// lease its buffer refused back ([`Self::strand`]): the cleanup after
    /// the confirmed stop ends it with this, never with a guess.
    stranded: [Option<Left>; 4],
    /// Completions the core gave a call that halted before it brought them
    /// home, newest first: the next collection takes them first, and no
    /// sink takes the core's completions while any wait here.
    uncollected: heapless::Vec<Completion<'id, Id, B>, 4>,
    /// The station's two data arenas over the A-MPDU chains, from
    /// preparation until retirement returns their chains and tickets.
    pub(super) arena: Option<DataArenas<'id, Id, B>>,
    /// A station group key replacement whose rollback failed: its
    /// command's owners, until the stopped port's cleanup confirms the
    /// key's entry cleared and removes the key ([`Driver::clean_retained_group`]).
    pub(super) retained_group: Option<RetainedGroup<'id, Id>>,
    /// The key of a removal the data port's caller holds at its barrier:
    /// queued attempts that name it end withdrawn.
    pub(super) barrier: Option<RawKeyIdentity>,
    pub(super) configuring: Option<brand::OwnedConfigured<'id, Id, D, B>>,
    /// The access point's configuration waits at its barrier: its queued
    /// attempts end withdrawn.
    pub(super) access_point_configuring: bool,
    pub(super) closing: Option<GateOpen<'id, Part<Id, 1>>>,
    pub(super) closed: bool,
    /// The installed keys, and in the place of each key whose removal the
    /// core finishes after its command, that command's rights until its
    /// receipt is delivered.
    pub(super) keys: [Option<KeyPlace<'id, Id>>; 20],
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
        if !self.free.descriptors_idle() {
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
        let single = self.free.take_descriptors()?;
        Some(TxDescriptors {
            single,
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
            tickets: crate::lower_mac::TicketBank::empty(),
            #[cfg(test)]
            refuse_values_for_test: false,
            #[cfg(test)]
            refuse_publication_for_test: false,
            #[cfg(test)]
            refuse_for_test: None,
            free: QueueHomes::empty(),
            queues: [const { heapless::Deque::new() }; 4],
            running: [const { None }; 4],
            stranded: [None; 4],
            uncollected: heapless::Vec::new(),
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
        tickets: [TxTicket<'id, CorePath<Id>, TX_TICKETS>; TX_TICKETS],
        queues: [FreeQueue<'id, CorePath<Id>>; 4],
        descriptors: TxDescriptors<B>,
        observations: O,
    ) -> Result<(), Fault> {
        // The one caller checked `retired` before taking any resource.
        self.core = core;
        self.tickets = crate::lower_mac::TicketBank::new(tickets);
        let TxDescriptors { single, chains } = descriptors;
        self.free = QueueHomes::new(queues, single);
        self.observations = Some(observations);
        let (Some(first), Some(second)) = (self.tickets.take(), self.tickets.take()) else {
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
        self.stranded = [None; 4];
        let descriptors = self.take_descriptors();
        // The installation ends: its tickets and queue places end with it.
        self.tickets = crate::lower_mac::TicketBank::empty();
        self.free = QueueHomes::empty();
        for key in &mut self.keys {
            drop(key.take());
        }
        self.barrier = None;
        drop(self.configuring.take());
        self.access_point_configuring = false;
        drop(self.closing.take());
        self.closed = false;
        self.applied_ba = [None; 8];
        let (transmitter, descriptors) = transmitter.zip(descriptors)?;
        Some((transmitter, descriptors, observations?))
    }
    pub(super) fn finish<E: DriverEnv<'id, Id, D, B, O>>(
        &mut self,
        env: &mut E,
        vacancy: E::Vacancy,
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
        let completed = pending.operation.complete(outcome, pending.cost);
        let completed = match pending.beacon {
            Some((permission, _)) => brand::OwnedTxCompleted::Beacon {
                completed,
                permission,
            },
            None => brand::OwnedTxCompleted::Single(completed),
        };
        env.tx_completed(vacancy, place, completed);
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
            if self.running[index].is_some() || !self.free.is_home_at(index) {
                continue;
            }
            // A selective barrier cannot return an owner past either a
            // published head or an earlier unaffected queued owner. Each
            // owner leaves under the call's right, obtained before it
            // leaves: without it the rest stay queued.
            while let Some(cause) = self
                .queues
                .get(index)
                .and_then(heapless::Deque::front)
                .and_then(|pending| {
                    global.or_else(|| self.withdrawal(pending.role(), pending.key()))
                })
            {
                let Some(vacancy) = env.vacancy() else {
                    break;
                };
                let Some(queued) = self
                    .queues
                    .get_mut(index)
                    .and_then(heapless::Deque::pop_front)
                else {
                    env.vacate(vacancy);
                    break;
                };
                self.finish(env, vacancy, queued, TxOutcome::Withdrawn(cause));
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
    pub(super) fn expire_beacons<E: DriverEnv<'id, Id, D, B, O>>(
        &mut self,
        env: &mut E,
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
        // The beacon leaves under the call's right, obtained before it
        // leaves: without it, it stays queued for the next call.
        if !queue.front().is_some_and(late) {
            return;
        }
        let Some(vacancy) = env.vacancy() else {
            return;
        };
        match self
            .queues
            .get_mut(index)
            .and_then(heapless::Deque::pop_front)
        {
            Some(pending) => self.finish(env, vacancy, pending, TxOutcome::Expired { msdus: 1 }),
            None => env.vacate(vacancy),
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
    ///
    /// Without the call's right no further completion leaves: the rest
    /// wait in the driver ([`Self::completion_sink`]), and the next
    /// collection brings them home first.
    pub(super) fn collect<E: DriverEnv<'id, Id, D, B, O>>(
        &mut self,
        env: &mut E,
        sink: &mut Sink<'id, Id, B>,
        stopping: bool,
    ) -> Result<(), Fault> {
        let mut first = None;
        // Each completion leaves under the call's right, obtained before it
        // leaves: after the first refusal the rest of the batch stays with
        // the driver.
        while let Some(vacancy) = env.vacancy() {
            let Some(completion) = self.uncollected.pop() else {
                env.vacate(vacancy);
                break;
            };
            if let Err(fault) = self.complete(env, vacancy, completion, stopping) {
                first.get_or_insert(fault);
            }
        }
        // Newest first, so that the oldest leaves first.
        let mut batch = core::mem::take(&mut sink.completed);
        batch.reverse();
        while let Some(vacancy) = env.vacancy() {
            let Some(completion) = batch.pop() else {
                env.vacate(vacancy);
                break;
            };
            if let Err(fault) = self.complete(env, vacancy, completion, stopping) {
                first.get_or_insert(fault);
            }
        }
        if !batch.is_empty() {
            // The sink took completions, so none waited when it was made
            // ([`Self::completion_sink`]), and the loop above took none
            // since: the driver's list is empty here.
            self.uncollected = batch;
        }
        if let Some(tbtt) = sink.tbtt.take() {
            env.tbtt(tbtt);
        }
        if let Some(fault) = first {
            return Err(fault);
        }
        self.withdraw(env);
        Ok(())
    }
    /// How many completions a halted call left here.
    #[cfg(test)]
    pub(super) fn uncollected_for_test(&self) -> usize {
        self.uncollected.len()
    }
    /// The sink of a core call: it takes the core's completions unless
    /// completions a halted call left wait in the driver, which come home
    /// first; the core keeps its own until then.
    pub(super) fn completion_sink(&self) -> Sink<'id, Id, B> {
        if self.uncollected.is_empty() {
            Sink::taking()
        } else {
            Sink::refusing()
        }
    }
    /// Bring one completion's owners home: its queue place, its ticket,
    /// its descriptor and the memory the descriptor held, then the attempt
    /// to its terminal or its next try. An owner whose home is taken or
    /// missing keeps the others home and names the fault; an attempt that
    /// cannot end stays published in `running`.
    fn complete<E: DriverEnv<'id, Id, D, B, O>>(
        &mut self,
        env: &mut E,
        vacancy: E::Vacancy,
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
        let category = free.category();
        let mut first = None;
        // The ticket returns to its own place.
        self.tickets.put(ticket);
        let Some(mut pending) = self.running.get_mut(index).and_then(Option::take) else {
            // No attempt to carry the lease home: the completion released the
            // hardware's hold, so it goes home by its own home. A bank lease
            // returns to its place now; a network's stays on its descriptor,
            // which goes home with its queue place, until the port's owner
            // takes it for the network ([`Self::take_orphan_network`]).
            if let Some((memory, home)) = slot.as_mut().detach_bank() {
                home.return_value(memory);
            }
            self.free.put_paired(free, slot);
            env.vacate(vacancy);
            return Err(*first.get_or_insert(Fault::CompletionWithoutPending { queue: at }));
        };
        // The lease leaves the queue's descriptor whole, back to its buffer.
        // Refused, it stays on the descriptor, which goes home with its
        // place and gives it back at the stop.
        let returned = match self.rejoin(&mut pending, slot.as_mut()) {
            Ok(()) => Ok(()),
            Err(TakeBackRefused::Missing) => Err(Fault::DescriptorMissing { queue: at }),
            Err(TakeBackRefused::Refused) => Err(Fault::ReturnRefused { queue: at }),
        };
        // What the publication added to the descriptor's record, read
        // before the descriptor goes home.
        let work = slot.as_ref().work();
        // The queue place and its descriptor go home together.
        self.free.put_paired(free, slot);
        pending.cost = pending
            .cost
            .add(attempt_cost(pending.try_work.take(), work, rate, &end));
        match returned {
            Ok(()) => {}
            Err(fault @ Fault::ReturnRefused { .. }) => {
                // The lease stays whole on the descriptor, which went home:
                // the attempt waits beside it with what its end says.
                self.keep_stranded(category, pending, Left::Ended { end, rate });
                env.vacate(vacancy);
                return Err(*first.get_or_insert(fault));
            }
            Err(fault) => {
                // Its memory is not back: the attempt stays published.
                self.keep_running(category, pending);
                env.vacate(vacancy);
                return Err(*first.get_or_insert(fault));
            }
        }
        self.conclude(env, vacancy, pending, end, rate, stopping)
    }
    /// End an attempt whose lease is back with its buffer by its known
    /// publication `end`: its terminal, or its next try.
    fn conclude<E: DriverEnv<'id, Id, D, B, O>>(
        &mut self,
        env: &mut E,
        vacancy: E::Vacancy,
        mut pending: Pending<'id, Id, D, B, O>,
        end: PublicationEnd,
        rate: PhyRate,
        stopping: bool,
    ) -> Result<(), Fault> {
        let category = pending.category;
        let index = queue(category);
        let at = index as u8;
        let mut first = None;
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
            self.finish(env, vacancy, pending, outcome);
        } else {
            env.vacate(vacancy);
            // A queue holds every ticket's attempt (`QUEUE_DEPTH`), so this
            // push has room; refused, the retry stays where it was.
            if let Err(pending) = self.enqueue(index, pending, true) {
                self.keep_running(category, pending);
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
    /// The lease a completion left on `slot` rejoins its buffer, whole;
    /// refused, it stays on the descriptor.
    fn rejoin(
        &mut self,
        pending: &mut Pending<'id, Id, D, B, O>,
        slot: Pin<&mut TxSlot<B>>,
    ) -> Result<(), TakeBackRefused> {
        #[cfg(test)]
        if self
            .refuse_for_test
            .take_if(|refusal| *refusal == TestRefusal::Return)
            .is_some()
        {
            return Err(TakeBackRefused::Refused);
        }
        pending.operation.execute().mpdu.buffer.take_back(slot)
    }
    /// A network's memory a home descriptor holds with no attempt beside it
    /// ([`Fault::CompletionWithoutPending`]), whole, once: its home is the
    /// network, through the port's owner. A descriptor the hardware holds,
    /// or one an attempt waits beside, gives nothing.
    pub(super) fn take_orphan_network(&mut self) -> Option<crate::mac::tx::TxBacking<B>> {
        for index in 0..TX_DESCRIPTORS {
            let alone = self.running.get(index).is_some_and(Option::is_none)
                && self.stranded.get(index).is_some_and(Option::is_none);
            if !alone {
                continue;
            }
            let Some(Some(slot)) = self.free.descriptors.get_mut(index) else {
                continue;
            };
            if slot.state() == crate::mac::tx::TxDmaState::Free
                && let Some(memory) = slot.as_mut().detach_network()
            {
                return Some(memory);
            }
        }
        None
    }
    /// Keep an attempt beside the lease its buffer refused back, in its
    /// queue's published place, with what is known of it.
    fn keep_stranded(
        &mut self,
        category: WmmAccessCategory,
        pending: Pending<'id, Id, D, B, O>,
        left: Left,
    ) {
        if let Some(place) = self.stranded.get_mut(queue(category)) {
            *place = Some(left);
        }
        self.keep_running(category, pending);
    }
    /// Keep an attempt a completion could not end in its queue's published
    /// place, which the completion just emptied.
    fn keep_running(&mut self, category: WmmAccessCategory, pending: Pending<'id, Id, D, B, O>) {
        let [voice, video, best_effort, background] = &mut self.running;
        let place = match LegacyTxQueue::from_access_category(category) {
            LegacyTxQueue::Voice => voice,
            LegacyTxQueue::Video => video,
            LegacyTxQueue::BestEffort => best_effort,
            LegacyTxQueue::Background => background,
        };
        *place = Some(pending);
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
            let mut sink = self.completion_sink();
            // What the service completed goes home even when a later queue
            // of the same service faulted; the service's fault comes first.
            let served = core_views::service::<Id, D, B>(
                self.core.view(),
                device.hardware()?,
                wake,
                &mut sink,
            );
            let collected = self.collect(env, &mut sink, stopping);
            served?;
            collected?;
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
        let mut sink = self.completion_sink();
        let served =
            core_views::service::<Id, D, B>(self.core.view(), device.hardware()?, wake, &mut sink);
        let collected = self.collect(env, &mut sink, stopping);
        served?;
        collected?;
        let settled = before && !settling(self);
        let (done, again) =
            self.service_arena(device, if settled { timeout } else { wake }, now)?;
        if again {
            let mut sink = self.completion_sink();
            let served = core_views::service::<Id, D, B>(
                self.core.view(),
                device.hardware()?,
                timeout,
                &mut sink,
            );
            let collected = self.collect(env, &mut sink, stopping);
            served?;
            collected?;
        }
        Ok(done)
    }
    /// Post one unit a cut drained to the caller, ahead of the cut's
    /// terminal, or return it to the ring when no rule admits it.
    pub(super) fn receive<E: DriverEnv<'id, Id, D, B, O>>(
        &mut self,
        env: &mut E,
        vacancy: E::Vacancy,
        device: &mut D,
        unit: D::Receive,
    ) -> Result<(), Fault> {
        let shared = env.shared();
        match self.admit(device, env.rx_out(), shared, unit) {
            Ok(Some((frame, meta))) => env.received(vacancy, frame, meta),
            Ok(None) => {
                env.vacate(vacancy);
                Ok(())
            }
            Err(fault) => {
                env.vacate(vacancy);
                Err(fault)
            }
        }
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
    pub(super) fn publish<E: DriverEnv<'id, Id, D, B, O>>(
        &mut self,
        device: &mut D,
        env: &mut E,
        vacancy: E::Vacancy,
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
        #[cfg(test)]
        let checked = checked.and_then(|checked| {
            if core::mem::take(&mut self.refuse_publication_for_test) {
                Err(Unpublished::Key)
            } else {
                Ok(checked)
            }
        });
        let refusal = match &checked {
            Err(refusal) => Some(*refusal),
            Ok((_, _, key)) => owner_free_refusal(
                self.observations.is_some(),
                self.tickets.at_home() != 0,
                self.free.descriptor(pending.category),
                self.free.is_home(pending.category),
                &mut pending,
                *key,
            ),
        };
        if let Some(refusal) = refusal {
            return Err(self.back_to_queue(env, vacancy, index, pending, refusal));
        }
        let Ok((mut core, admitted_at, key_handle)) = checked else {
            return Err(self.requeue(env, vacancy, index, pending, refused(PublishStep::Values)));
        };
        // The registers first: a device that holds none takes nothing of
        // the attempt, which goes back to the head of its queue.
        let hardware = match device.hardware() {
            Ok(hardware) => hardware,
            Err(fault) => return Err(self.requeue(env, vacancy, index, pending, fault)),
        };
        let Some(observations) = self.observations.as_mut() else {
            return Err(self.requeue(
                env,
                vacancy,
                index,
                pending,
                refused(PublishStep::Observations),
            ));
        };
        let backoff_slots = core.draw_backoff(role, pending.category, observations);
        let Some(core_ticket) = self.tickets.take() else {
            return Err(self.requeue(env, vacancy, index, pending, refused(PublishStep::Ticket)));
        };
        let admission = match key_handle {
            Some(Installed::Transmit(key)) => match core.admit_key(key, core_ticket) {
                Ok(key_use) => TxAdmission::Protected(key_use),
                Err(refusal) => {
                    self.tickets.put(refusal.ticket);
                    return Err(self.requeue(
                        env,
                        vacancy,
                        index,
                        pending,
                        refused(PublishStep::KeyAdmission),
                    ));
                }
            },
            Some(Installed::Receive(_)) => {
                self.tickets.put(core_ticket);
                return Err(self.requeue(
                    env,
                    vacancy,
                    index,
                    pending,
                    refused(PublishStep::ReceiveKey),
                ));
            }
            None => TxAdmission::Open(core_ticket),
        };
        // The queue place and its descriptor before the buffer's lease: a
        // refusal here holds the admission alone, which goes back as a
        // ticket.
        let Some((queue, mut slot)) = self.free.take_paired(pending.category) else {
            return Err(self.unadmit(
                hardware,
                env,
                vacancy,
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
                vacancy,
                index,
                (pending, admission),
                Some((queue, Some(slot))),
                refused(PublishStep::DescriptorBusy),
            ));
        }
        let mpdu = pending.operation.execute().mpdu;
        // The hardware takes the memory through the queue's descriptor, with
        // the right that takes it home, as one lease. Refused, the buffer
        // keeps both.
        if let Err(refusal) = mpdu.buffer.attach_to(slot.as_mut()) {
            let step = match refusal {
                AttachRefused::NotFree => PublishStep::Buffer,
                AttachRefused::Descriptor => PublishStep::Attach,
            };
            return Err(self.unadmit(
                hardware,
                env,
                vacancy,
                index,
                (pending, admission),
                Some((queue, Some(slot))),
                refused(step),
            ));
        }
        if slot.state() != crate::mac::tx::TxDmaState::Free {
            return self.unwind(
                hardware,
                env,
                vacancy,
                index,
                (pending, admission),
                (queue, slot),
                PublishStep::DescriptorBusy,
            );
        }
        // Free a moment ago, in the same call: the slot lends its memory.
        let TxSlotAccess::Free(mut memory) = slot.access() else {
            // Excluded by the read just above. The descriptor keeps the
            // memory out of every pool; the queue place and the other
            // owners go back beside the fault.
            return Err(self.unadmit(
                hardware,
                env,
                vacancy,
                index,
                (pending, admission),
                Some((queue, None)),
                Fault::DescriptorMissing { queue: at },
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
                        vacancy,
                        index,
                        (pending, admission),
                        (queue, memory.into_slot()),
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
                vacancy,
                index,
                (pending, admission),
                (queue, memory.into_slot()),
                PublishStep::Values,
            );
        }
        // What this publication adds to the descriptor's record is its cost.
        pending.try_work = Some(memory.slot().work());
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
                let slot = refused.slot.into_slot();
                return self.unwind(
                    hardware,
                    env,
                    vacancy,
                    index,
                    (pending, admission),
                    (queue, slot),
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
            // The core refused the publication: its owners go back as at a
            // refusal before it, and the attempt to the head of its queue.
            (
                pending.operation.execute().mpdu.published,
                pending.published,
            ) = before;
            return Err(self.put_back(
                hardware,
                env,
                vacancy,
                index,
                (pending, admission),
                (queue, slot),
                Fault::Core(fault),
            ));
        }
        // The core holds the publication; its attempt waits in its queue's
        // published place, which no attempt of this queue holds while its
        // queue place was home.
        self.keep_running(pending.category, pending);
        env.vacate(vacancy);
        #[cfg(all(oer_trace, oer_debug_control))]
        oer_portable_trace_recorder::emit(&publication);
        Ok(())
    }
    /// Put `pending` back at the head of its queue, every owner of it
    /// back in its place: nothing of it was published. A refusal of the
    /// registers or of a publication step is then
    /// [`Fault::Unpublished`], which a stop recovers from and whose stop
    /// withdraws the attempt; any other `fault` stays as it is. A queue
    /// without room ends the attempt and answers [`Fault::QueueFull`].
    fn requeue<E: DriverEnv<'id, Id, D, B, O>>(
        &mut self,
        env: &mut E,
        vacancy: E::Vacancy,
        index: usize,
        pending: Pending<'id, Id, D, B, O>,
        fault: Fault,
    ) -> Fault {
        let whole = match fault {
            Fault::HardwareUnavailable => Some(Unpublished::Registers),
            Fault::Publish { step, .. } => Some(Unpublished::Step(step)),
            _ => None,
        };
        match whole {
            Some(refusal) => self.back_to_queue(env, vacancy, index, pending, refusal),
            None => self.requeue_beside(env, vacancy, index, pending, fault),
        }
    }
    /// Put `pending` back at the head of its queue with `fault`, which an
    /// owner kept elsewhere makes the port's to keep.
    fn requeue_beside<E: DriverEnv<'id, Id, D, B, O>>(
        &mut self,
        env: &mut E,
        vacancy: E::Vacancy,
        index: usize,
        pending: Pending<'id, Id, D, B, O>,
        fault: Fault,
    ) -> Fault {
        match self.back_to_queue(env, vacancy, index, pending, Unpublished::CoreView) {
            Fault::Unpublished(_) => fault,
            full => full,
        }
    }
    /// Give a refused publication's admission back as a ticket and its
    /// descriptor back to its queue's place, then the attempt to the head of
    /// its queue: `fault` names the refusal. An admission the core does not
    /// take back is that fault instead.
    #[allow(
        clippy::too_many_arguments,
        reason = "the attempt's owners, its environment and the call's right are distinct values"
    )]
    fn unadmit<E: DriverEnv<'id, Id, D, B, O>>(
        &mut self,
        hardware: &mut D::Hardware,
        env: &mut E,
        vacancy: E::Vacancy,
        index: usize,
        (pending, admission): (
            Pending<'id, Id, D, B, O>,
            TxAdmission<'id, CorePath<Id>, TX_TICKETS>,
        ),
        home: Option<(
            FreeQueue<'id, CorePath<Id>>,
            Option<Pin<&'static mut TxSlot<B>>>,
        )>,
        fault: Fault,
    ) -> Fault {
        // The queue place goes home with the descriptor it took.
        match home {
            Some((queue, Some(slot))) => self.free.put_paired(queue, slot),
            Some((queue, None)) => self.free.put(queue),
            None => {}
        }
        self.returned_ticket(hardware, admission);
        self.requeue(env, vacancy, index, pending, fault)
    }
    /// A publication the core refused at its submission: the memory
    /// rejoins the buffer when the descriptor gives it up (otherwise it
    /// stays attached, and the descriptor keeps it from every pool), then
    /// every owner goes back as at a refusal before the submission.
    #[allow(
        clippy::too_many_arguments,
        reason = "the attempt's owners, its environment and the call's right are distinct values"
    )]
    fn put_back<E: DriverEnv<'id, Id, D, B, O>>(
        &mut self,
        hardware: &mut D::Hardware,
        env: &mut E,
        vacancy: E::Vacancy,
        index: usize,
        (mut pending, admission): (
            Pending<'id, Id, D, B, O>,
            TxAdmission<'id, CorePath<Id>, TX_TICKETS>,
        ),
        (queue, mut slot): (FreeQueue<'id, CorePath<Id>>, Pin<&'static mut TxSlot<B>>),
        fault: Fault,
    ) -> Fault {
        // The lease goes back to its buffer, whole. Refused, the attempt
        // stays beside it ([`Self::strand`]).
        if let Err(TakeBackRefused::Refused) = pending
            .operation
            .execute()
            .mpdu
            .buffer
            .take_back(slot.as_mut())
        {
            return self.strand(
                hardware,
                env,
                vacancy,
                index,
                (pending, admission),
                (queue, slot),
            );
        }
        self.unadmit(
            hardware,
            env,
            vacancy,
            index,
            (pending, admission),
            Some((queue, Some(slot))),
            fault,
        )
    }
    /// An attempt whose buffer refused its lease back: the lease stays
    /// whole on its queue's descriptor, which goes home with its place and
    /// keeps it from every pool, and the attempt waits beside it in its
    /// queue's published place, which no attempt of this queue holds while
    /// its queue place was taken. The ticket goes back. Both stay there
    /// until the cleanup after a confirmed stop ([`Self::rejoin_stranded`]).
    #[allow(
        clippy::too_many_arguments,
        reason = "the attempt's owners, its environment and the call's right are distinct values"
    )]
    fn strand<E: DriverEnv<'id, Id, D, B, O>>(
        &mut self,
        hardware: &mut D::Hardware,
        env: &mut E,
        vacancy: E::Vacancy,
        index: usize,
        (pending, admission): (
            Pending<'id, Id, D, B, O>,
            TxAdmission<'id, CorePath<Id>, TX_TICKETS>,
        ),
        (queue, slot): (FreeQueue<'id, CorePath<Id>>, Pin<&'static mut TxSlot<B>>),
    ) -> Fault {
        self.free.put_paired(queue, slot);
        self.returned_ticket(hardware, admission);
        self.keep_stranded(pending.category, pending, Left::Unpublished);
        env.vacate(vacancy);
        Fault::ReturnRefused { queue: index as u8 }
    }
    /// After a confirmed stop, which released every descriptor's DMA: each
    /// attempt a refused return left beside its lease ([`Self::strand`])
    /// takes the lease back from its queue's descriptor, once per call, and
    /// ends with what is known of it ([`Left`]): a completed attempt by its
    /// publication's end, as a completion during a stop ends it (a delivery
    /// stays delivered), one never published withdrawn as unpublishable.
    /// The attempts queued behind it, never sent, end withdrawn. Each
    /// leaves under the call's right; without it the rest wait for the next
    /// call.
    ///
    /// # Errors
    ///
    /// The buffer refused its lease again: the attempt and the lease stay
    /// where they were, for the owner's next call.
    pub(super) fn rejoin_stranded<E: DriverEnv<'id, Id, D, B, O>>(
        &mut self,
        env: &mut E,
    ) -> Result<(), Fault> {
        for index in 0..TX_DESCRIPTORS {
            let stranded = self.stranded.get(index).is_some_and(Option::is_some)
                && self.running.get(index).is_some_and(Option::is_some)
                && self.free.is_home_at(index)
                && self.free.descriptors.get(index).is_some_and(|slot| {
                    slot.as_ref().is_some_and(|slot| {
                        slot.state() == crate::mac::tx::TxDmaState::Free
                            && slot.home_kind().is_some()
                    })
                });
            if stranded {
                let Some(vacancy) = env.vacancy() else {
                    return Ok(());
                };
                let (Some(mut pending), Some(mut slot)) = (
                    self.running.get_mut(index).and_then(Option::take),
                    self.free.descriptors.get_mut(index).and_then(Option::take),
                ) else {
                    // Both were checked present above.
                    env.vacate(vacancy);
                    return Ok(());
                };
                let rejoined = self.rejoin(&mut pending, slot.as_mut());
                if let Some(place) = self.free.descriptors.get_mut(index) {
                    *place = Some(slot);
                }
                if rejoined.is_err() {
                    self.keep_running(pending.category, pending);
                    env.vacate(vacancy);
                    return Err(Fault::ReturnRefused { queue: index as u8 });
                }
                // The attempt ends with what was known of it: a completed
                // one by its end, one never published withdrawn.
                match self.stranded.get_mut(index).and_then(Option::take) {
                    Some(Left::Ended { end, rate }) => {
                        self.conclude(env, vacancy, pending, end, rate, true)?;
                    }
                    Some(Left::Unpublished) | None => self.finish(
                        env,
                        vacancy,
                        pending,
                        TxOutcome::Withdrawn(WithdrawCause::Unpublishable),
                    ),
                }
            }
            if self.running.get(index).is_some_and(Option::is_some) {
                continue;
            }
            // The queue behind it, which the stop's withdrawal passed by.
            while self
                .queues
                .get(index)
                .is_some_and(|queue| !queue.is_empty())
            {
                let Some(vacancy) = env.vacancy() else {
                    return Ok(());
                };
                let Some(queued) = self
                    .queues
                    .get_mut(index)
                    .and_then(heapless::Deque::pop_front)
                else {
                    env.vacate(vacancy);
                    break;
                };
                self.finish(
                    env,
                    vacancy,
                    queued,
                    TxOutcome::Withdrawn(WithdrawCause::Disabled),
                );
            }
        }
        Ok(())
    }
    /// Put `pending` back at the head of its queue: nothing of it was
    /// published, and a stop withdraws it there.
    fn back_to_queue<E: DriverEnv<'id, Id, D, B, O>>(
        &mut self,
        env: &mut E,
        vacancy: E::Vacancy,
        index: usize,
        pending: Pending<'id, Id, D, B, O>,
        refusal: Unpublished,
    ) -> Fault {
        let queue = index as u8;
        // The attempt left this queue in the same call, and a queue holds
        // every ticket's attempt (`QUEUE_DEPTH`): the push has room.
        match self.enqueue(index, pending, true) {
            Ok(()) => {
                env.vacate(vacancy);
                Fault::Unpublished(refusal)
            }
            Err(pending) => {
                // Refused all the same: the attempt ends withdrawn with
                // every owner, under the right its publication took, and
                // the fault stops the port.
                self.finish(
                    env,
                    vacancy,
                    pending,
                    TxOutcome::Withdrawn(WithdrawCause::Disabled),
                );
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
    #[allow(
        clippy::too_many_arguments,
        reason = "the attempt's owners, its environment and the call's right are distinct values"
    )]
    fn unwind<E: DriverEnv<'id, Id, D, B, O>>(
        &mut self,
        hardware: &mut D::Hardware,
        env: &mut E,
        vacancy: E::Vacancy,
        index: usize,
        (mut pending, admission): (
            Pending<'id, Id, D, B, O>,
            TxAdmission<'id, CorePath<Id>, TX_TICKETS>,
        ),
        (queue, mut slot): (FreeQueue<'id, CorePath<Id>>, Pin<&'static mut TxSlot<B>>),
        step: PublishStep,
    ) -> Result<(), Fault> {
        let terminal = Fault::Publish {
            queue: index as u8,
            step,
        };
        // The lease goes back to its buffer, whole. Refused, the attempt
        // stays beside it ([`Self::strand`]).
        match pending
            .operation
            .execute()
            .mpdu
            .buffer
            .take_back(slot.as_mut())
        {
            Ok(()) => {}
            Err(TakeBackRefused::Refused) => {
                return Err(self.strand(
                    hardware,
                    env,
                    vacancy,
                    index,
                    (pending, admission),
                    (queue, slot),
                ));
            }
            Err(TakeBackRefused::Missing) => {
                return Err(self.unadmit(
                    hardware,
                    env,
                    vacancy,
                    index,
                    (pending, admission),
                    Some((queue, Some(slot))),
                    terminal,
                ));
            }
        }
        self.free.put_paired(queue, slot);
        self.returned_ticket(hardware, admission);
        self.finish(
            env,
            vacancy,
            pending,
            TxOutcome::Withdrawn(WithdrawCause::Unpublishable),
        );
        Ok(())
    }
    /// Return the ticket of an admission never published to its own place,
    /// its key use returned first, in whichever phase the core is. A
    /// removal this return finishes keeps its receipt in the core's
    /// removal place (this sink takes none), and a removal takes its key
    /// out of `keys` before its barrier, so a publication's key has none
    /// waiting on this use.
    fn returned_ticket(
        &mut self,
        hardware: &mut D::Hardware,
        admission: TxAdmission<'id, CorePath<Id>, TX_TICKETS>,
    ) {
        let mut sink = Sink::refusing();
        let ticket = self
            .core
            .view()
            .return_admission(hardware, admission, &mut sink);
        self.tickets.put(ticket);
    }
    /// Install `installation` as the key `identity`. Refused, the
    /// installation comes back whole: before the core took its slot (no
    /// free table place, the registers or the core's command view
    /// unavailable) or with the slot the core's refusal returned.
    pub(super) fn install(
        &mut self,
        device: &mut D,
        identity: RawKeyIdentity,
        installation: Installation<'id, Id>,
        material: &Ccmp128Key,
    ) -> Result<(), (Installation<'id, Id>, Fault)> {
        match installation {
            Installation::Transmit { spp, scope, slot } => self
                .install_transmit(device, identity, spp, scope, slot, material)
                .map_err(|(slot, fault)| (Installation::Transmit { spp, scope, slot }, fault)),
            Installation::Receive { spp, slot, key_id } => self
                .install_receive(device, identity, spp, slot, key_id, material)
                .map_err(|(slot, fault)| (Installation::Receive { spp, slot, key_id }, fault)),
        }
    }
    /// Install a transmit key in `slot`; refused, the slot comes back as
    /// given.
    pub(super) fn install_transmit<S>(
        &mut self,
        device: &mut D,
        identity: RawKeyIdentity,
        spp: oer_protocols_ieee80211_wire::security::Spp,
        scope: TransmitKeyScope,
        slot: S,
        material: &Ccmp128Key,
    ) -> Result<(), (S, Fault)>
    where
        S: Into<core_mac::association::VacantKeySlot<'id, CorePath<Id>>>,
    {
        // The table place, the registers and the command view first: a
        // refusal there leaves the slot untouched.
        let Some(place) = self.keys.iter_mut().find(|place| place.is_none()) else {
            return Err((slot, Fault::KeySlotMissing(KeyOp::Install)));
        };
        let hardware = match device.hardware() {
            Ok(hardware) => hardware,
            Err(fault) => return Err((slot, fault)),
        };
        let Some(mut view) = self.core.view().commands() else {
            return Err((slot, Fault::CoreViewUnavailable(CoreOp::Command)));
        };
        match view.install_key_parts(hardware, spp, scope, slot, material.as_bytes()) {
            Ok(key) => {
                *place = Some(KeyPlace::Installed(identity, Installed::Transmit(key)));
                Ok(())
            }
            Err(refused) => Err((refused.slot, Fault::Core(refused.fault))),
        }
    }
    /// Install the station's group key for reception in `slot`; refused,
    /// the slot comes back.
    pub(super) fn install_receive(
        &mut self,
        device: &mut D,
        identity: RawKeyIdentity,
        spp: oer_protocols_ieee80211_wire::security::Spp,
        slot: core_mac::association::StationGroupSlot<'id, CorePath<Id>>,
        key_id: GroupKeyId,
        material: &Ccmp128Key,
    ) -> Result<
        (),
        (
            core_mac::association::StationGroupSlot<'id, CorePath<Id>>,
            Fault,
        ),
    > {
        let Some(place) = self.keys.iter_mut().find(|place| place.is_none()) else {
            return Err((slot, Fault::KeySlotMissing(KeyOp::Install)));
        };
        let hardware = match device.hardware() {
            Ok(hardware) => hardware,
            Err(fault) => return Err((slot, fault)),
        };
        let Some(mut view) = self.core.view().commands() else {
            return Err((slot, Fault::CoreViewUnavailable(CoreOp::Command)));
        };
        match view.install_station_group_parts(hardware, spp, slot, key_id, material.as_bytes()) {
            Ok(key) => {
                *place = Some(KeyPlace::Installed(identity, Installed::Receive(key)));
                Ok(())
            }
            Err(refused) => Err((refused.slot, Fault::Core(refused.fault))),
        }
    }
    /// Remove the installed key `identity`, whose slot must be of the kind
    /// its rights `R` take: its receipt now, or its core place while its
    /// last use is out, with the key's table place this removal empties.
    ///
    /// # Errors
    ///
    /// The key is not installed, is of another slot kind, or the registers
    /// or the core's command view are unavailable: nothing is taken, and
    /// the installed key stays in its place.
    pub(super) fn remove<R: RemovalRights<'id, Id>>(
        &mut self,
        device: &mut D,
        identity: RawKeyIdentity,
    ) -> Result<Removing<'_, 'id, Id, R>, Fault> {
        let place = self
            .keys
            .iter_mut()
            .find(|entry| {
                entry
                    .as_ref()
                    .and_then(KeyPlace::installed)
                    .is_some_and(|(key, _)| *key == identity)
            })
            .ok_or(Fault::KeySlotMissing(KeyOp::Remove))?;
        // The registers and the core's command view first: a removal they
        // refuse leaves the installed key in its place.
        let hardware = device.hardware()?;
        let mut view = self
            .core
            .view()
            .commands()
            .ok_or(Fault::CoreViewUnavailable(CoreOp::Command))?;
        let (identity, installed) = match place.take() {
            Some(KeyPlace::Installed(identity, installed)) => (identity, installed),
            other => {
                *place = other;
                return Err(Fault::KeySlotMissing(KeyOp::Remove));
            }
        };
        // The key's slot must be of the rights' own kind, which its receipt
        // returns: a key of another kind comes back to its place whole.
        let removing = match installed {
            Installed::Transmit(key) => key.into_removing().map_err(Installed::Transmit),
            Installed::Receive(key) => key.into_removing().map_err(Installed::Receive),
        };
        let removing = match removing {
            Ok(removing) => removing,
            Err(installed) => {
                *place = Some(KeyPlace::Installed(identity, installed));
                return Err(Fault::RemovalMismatch);
            }
        };
        let mut sink = Sink::refusing();
        let core = view.remove_checked_key(hardware, removing, &mut sink);
        // This removal's own core place: a receipt of another key stays in
        // its place for its own removal.
        let ended = match self.core.view().take_removed(core) {
            Ok(removed) => Removed::Now(removed),
            Err(core) => Removed::Waiting(core),
        };
        Ok(Removing { place, ended })
    }
    /// The removal waiting in table place `index`, once the core finished
    /// it: its terminal returns the waiting command's rights.
    pub(super) fn finished_removal(
        &mut self,
        index: usize,
    ) -> Option<brand::OwnedRemoved<'id, Id>> {
        let place = self.keys.get_mut(index)?;
        let waiting = match place.take() {
            Some(KeyPlace::Removing(waiting)) => waiting,
            other => {
                *place = other;
                return None;
            }
        };
        let finished = match waiting {
            WaitingRemoval::StationPairwise(waiting) => {
                finish::<Id, D, B, _>(&mut self.core, waiting)
            }
            WaitingRemoval::AccessPointPairwise(waiting) => {
                finish::<Id, D, B, _>(&mut self.core, waiting)
            }
            WaitingRemoval::StationGroup(waiting) => finish::<Id, D, B, _>(&mut self.core, waiting),
            WaitingRemoval::AccessPointGroup(waiting) => {
                finish::<Id, D, B, _>(&mut self.core, waiting)
            }
        };
        match finished {
            Ok(removed) => Some(removed),
            Err(waiting) => {
                *place = Some(KeyPlace::Removing(waiting));
                None
            }
        }
    }
    /// Admit one use of the first installed transmit key outside any
    /// publication: its last use then returns after a removal.
    #[cfg(test)]
    pub(super) fn hold_key_use_for_test(
        &mut self,
    ) -> Option<TxAdmission<'id, CorePath<Id>, TX_TICKETS>> {
        let ticket = self.tickets.take()?;
        let key = self
            .keys
            .iter()
            .flatten()
            .filter_map(KeyPlace::installed)
            .find_map(|(_, installed)| match installed {
                Installed::Transmit(key) => Some(key),
                Installed::Receive(_) => None,
            });
        let (Some(key), CoreView::Open(mut core)) = (key, self.core.view()) else {
            self.tickets.put(ticket);
            return None;
        };
        match core.admit_key(key, ticket) {
            Ok(key_use) => Some(TxAdmission::Protected(key_use)),
            Err(refused) => {
                self.tickets.put(refused.ticket);
                None
            }
        }
    }
    /// Return a use [`Self::hold_key_use_for_test`] admitted, as an
    /// unpublished admission returns: through a sink that takes no receipt.
    #[cfg(test)]
    pub(super) fn return_key_use_for_test(
        &mut self,
        hardware: &mut D::Hardware,
        admission: TxAdmission<'id, CorePath<Id>, TX_TICKETS>,
    ) {
        self.returned_ticket(hardware, admission);
    }
    /// Whether a waiting removal's receipt is ready to deliver.
    pub(super) fn removal_ready(&mut self) -> bool {
        let core = &mut self.core;
        self.keys.iter().any(|place| match place {
            Some(KeyPlace::Removing(WaitingRemoval::StationPairwise(waiting))) => {
                core.view().removed_ready(&waiting.core)
            }
            Some(KeyPlace::Removing(WaitingRemoval::AccessPointPairwise(waiting))) => {
                core.view().removed_ready(&waiting.core)
            }
            Some(KeyPlace::Removing(WaitingRemoval::StationGroup(waiting))) => {
                core.view().removed_ready(&waiting.core)
            }
            Some(KeyPlace::Removing(WaitingRemoval::AccessPointGroup(waiting))) => {
                core.view().removed_ready(&waiting.core)
            }
            _ => false,
        })
    }
    /// The kind of slot the installed key `identity` holds, while it is
    /// installed.
    pub(super) fn installed_kind(
        &self,
        identity: RawKeyIdentity,
    ) -> Option<core_mac::association::KeySlotKind> {
        self.keys
            .iter()
            .flatten()
            .filter_map(KeyPlace::installed)
            .find(|(key, _)| **key == identity)
            .map(|(_, installed)| installed.kind())
    }
    /// Whether a removal its command left waits in the key table.
    pub(super) fn removal_waits(&self) -> bool {
        self.removals_waiting() != 0
    }
    /// The key removals waiting for the core to clear their keys.
    pub(super) fn removals_waiting(&self) -> usize {
        self.keys
            .iter()
            .filter(|place| matches!(place, Some(KeyPlace::Removing(_))))
            .count()
    }
    /// Run a station group mutation whose cut drained.
    ///
    /// # Errors
    ///
    /// [`GroupFailed::Returned`]: a step refused before it took the
    /// command's owners, which come back whole; [`GroupFailed::Retained`]:
    /// the hardware rejected a replacement, whose owners the port keeps.
    #[allow(
        clippy::result_large_err,
        reason = "a refusal returns the command by value, as a hold does"
    )]
    pub(super) fn group_station(
        &mut self,
        env: &mut impl DriverEnv<'id, Id, D, B, O>,
        device: &mut D,
        command: StationGroupCommand<'id, Id>,
    ) -> Result<
        Option<brand::OwnedTerminal<'id, Id, D, B>>,
        GroupFailed<StationGroupCommand<'id, Id>>,
    > {
        match command {
            StationGroupCommand::Install(GroupInstallation {
                spp,
                slot,
                permission,
                key_id,
                material,
            }) => {
                let identity = RawKeyIdentity {
                    role: VifRole::Station,
                    slot: keys::SlotPlace::StationGroup(slot.place()),
                    serial: env.serial(),
                    key_id,
                    activity: &env.shared().key_live[1],
                };
                if let Err((slot, fault)) =
                    self.install_receive(device, identity, spp, slot, key_id, &material)
                {
                    return Err(GroupFailed::Returned(
                        StationGroupCommand::Install(GroupInstallation {
                            spp,
                            slot,
                            permission,
                            key_id,
                            material,
                        }),
                        fault,
                    ));
                }
                identity.activity.set(identity.serial);
                Ok(Some(brand::OwnedTerminal::GroupKeyInstalled(
                    brand::OwnedInstalledGroup::Station {
                        key: GroupKey {
                            identity,
                            brand: PhantomData,
                        },
                        permission,
                    },
                )))
            }
            StationGroupCommand::Replace {
                permission,
                key,
                key_id,
                material,
            } => {
                let back = |permission, key, material, fault| {
                    GroupFailed::Returned(
                        StationGroupCommand::Replace {
                            permission,
                            key,
                            key_id,
                            material,
                        },
                        fault,
                    )
                };
                let entry = self
                    .keys
                    .iter_mut()
                    .flatten()
                    .filter_map(KeyPlace::installed_mut)
                    .find(|(identity, _)| **identity == key.identity);
                let Some((entry_identity, Installed::Receive(installed))) = entry else {
                    return Err(back(
                        permission,
                        key,
                        material,
                        Fault::KeySlotMissing(KeyOp::Replace),
                    ));
                };
                let hardware = match device.hardware() {
                    Ok(hardware) => hardware,
                    Err(fault) => return Err(back(permission, key, material, fault)),
                };
                #[cfg(oer_debug_control)]
                let mut hardware = key_replacement::RejectOnce {
                    hardware,
                    reject: env.reject_group_replacement(),
                };
                #[cfg(oer_debug_control)]
                let hardware = &mut hardware;
                let Some(mut view) = self.core.view().commands() else {
                    return Err(back(
                        permission,
                        key,
                        material,
                        Fault::CoreViewUnavailable(CoreOp::Command),
                    ));
                };
                let result = view.replace_station_group_key(
                    hardware,
                    installed,
                    key_id,
                    material.as_bytes(),
                );
                if let Err(error) = result {
                    #[cfg(test)]
                    let _first = env.shared().rolled_back.set((
                        key.identity.key_id.get(),
                        key_id.get(),
                        matches!(error, ReplaceError::ReplacementRolledBack(_)),
                    ));
                    #[cfg(all(oer_trace, oer_debug_control))]
                    oer_portable_trace_recorder::emit(&trace::GroupRollback {
                        owners: [key.identity.slot.index(), permission.index()],
                        previous_key_id: key.identity.key_id.get(),
                        attempted_key_id: key_id.get(),
                        restored: matches!(error, ReplaceError::ReplacementRolledBack(_)),
                    });
                    return Err(match replacement_failed(error) {
                        Err(failure) => {
                            back(permission, key, material, Fault::GroupReplacement(failure))
                        }
                        Ok((replacement, rollback)) => {
                            // Neither key's entry is installed: the previous
                            // key is not live, and its slot stays with the
                            // port until its stop confirms the entry cleared.
                            key.identity.activity.set(0);
                            #[cfg(test)]
                            let _first = env
                                .shared()
                                .retained_group
                                .set((key.identity.slot.index(), permission.index()));
                            // The replacement's material has no use left:
                            // it zeroizes as it drops here.
                            drop(material);
                            self.retained_group = Some(RetainedGroup {
                                permission,
                                key,
                                clear: GroupClear::NotIssued,
                            });
                            GroupFailed::Retained(Fault::GroupReplacement(
                                GroupReplacement::RollbackFailed {
                                    replacement,
                                    rollback,
                                },
                            ))
                        }
                    });
                }
                let identity = RawKeyIdentity {
                    serial: env.serial(),
                    key_id,
                    ..key.identity
                };
                *entry_identity = identity;
                identity.activity.set(identity.serial);
                Ok(Some(brand::OwnedTerminal::GroupKeyReplaced {
                    key: GroupKey {
                        identity,
                        brand: PhantomData,
                    },
                    permission,
                }))
            }
            StationGroupCommand::Select { permission, key } => {
                if !self
                    .keys
                    .iter()
                    .flatten()
                    .filter_map(KeyPlace::installed)
                    .any(|(identity, _)| *identity == key.identity)
                {
                    return Err(GroupFailed::Returned(
                        StationGroupCommand::Select { permission, key },
                        Fault::KeySlotMissing(KeyOp::Select),
                    ));
                }
                Ok(Some(brand::OwnedTerminal::GroupKeySelected {
                    key,
                    permission,
                }))
            }
            StationGroupCommand::Remove { permission, key } => {
                match self.remove::<StaPermission<'id, Id>>(device, key.identity) {
                    Ok(removing) => Ok(removing
                        .settle(permission)
                        .map(brand::OwnedTerminal::KeyRemoved)),
                    Err(fault) => Err(GroupFailed::Returned(
                        StationGroupCommand::Remove { permission, key },
                        fault,
                    )),
                }
            }
        }
    }
    /// The stopped port's cleanup of a station group replacement whose
    /// rollback failed. Its one write, the clear of the key's entry, is
    /// issued once; from then on the cleanup only reads the key table. Once
    /// the table reads the entry cleared, the key is removed (writing the
    /// entry no more), its terminal returning the slot and the command's
    /// permission.
    ///
    /// # Errors
    ///
    /// This call's result, never recorded; the owners stay retained, the
    /// slot is not free and the previous key stays inactive, and the
    /// lifecycle owner's next call goes on from where it is.
    /// [`CleanupRefused::Unavailable`]: the step at hand cannot run (the
    /// registers, the core's command view, the key's place, or after a
    /// cleared read the removal) and wrote nothing.
    /// [`CleanupRefused::Held`]: the clear was issued and the table reads
    /// the entry valid ([`Fault::GroupEntryHeld`]). The read always answers:
    /// the entry's index lies in the table by its type.
    pub(super) fn clean_retained_group(
        &mut self,
        device: &mut D,
    ) -> Result<Option<brand::OwnedRemoved<'id, Id>>, CleanupRefused<Fault>> {
        let Some(mut retained) = self.retained_group.take() else {
            return Ok(None);
        };
        let identity = retained.key.identity;
        let observed = (|| {
            let hardware = device.hardware().map_err(CleanupRefused::Unavailable)?;
            let installed = self
                .keys
                .iter_mut()
                .flatten()
                .filter_map(KeyPlace::installed_mut)
                .find_map(|(held, installed)| match installed {
                    Installed::Receive(installed) if *held == identity => Some(installed),
                    _ => None,
                })
                .ok_or(CleanupRefused::Unavailable(Fault::KeySlotMissing(
                    KeyOp::Remove,
                )))?;
            let mut view = self
                .core
                .view()
                .commands()
                .ok_or(CleanupRefused::Unavailable(Fault::CoreViewUnavailable(
                    CoreOp::Command,
                )))?;
            if retained.clear == GroupClear::NotIssued {
                view.clear_failed_station_group(hardware, installed);
                retained.clear = GroupClear::Issued;
            }
            match view.read_failed_station_group(hardware, installed) {
                crate::mac::crypto::EntryRead::Cleared => Ok(()),
                crate::mac::crypto::EntryRead::Held => {
                    Err(CleanupRefused::Held(Fault::GroupEntryHeld))
                }
            }
        })();
        let removed = observed.and_then(|()| {
            self.remove::<StaPermission<'id, Id>>(device, identity)
                .map_err(CleanupRefused::Unavailable)
        });
        match removed {
            Ok(removing) => Ok(removing.settle(retained.permission)),
            Err(refused) => {
                self.retained_group = Some(retained);
                Err(refused)
            }
        }
    }
    /// Run an access-point group mutation whose cut drained.
    ///
    /// # Errors
    ///
    /// A step refused before it took the command's owners: they come back
    /// whole.
    pub(super) fn group_access_point(
        &mut self,
        env: &mut impl DriverEnv<'id, Id, D, B, O>,
        device: &mut D,
        command: AccessPointGroupCommand<'id, Id>,
    ) -> Result<
        Option<brand::OwnedTerminal<'id, Id, D, B>>,
        GroupFailed<AccessPointGroupCommand<'id, Id>>,
    > {
        match command {
            AccessPointGroupCommand::Install(GroupInstallation {
                spp,
                slot,
                permission,
                key_id,
                material,
            }) => {
                let back = |slot, permission, material, fault| {
                    GroupFailed::Returned(
                        AccessPointGroupCommand::Install(GroupInstallation {
                            spp,
                            slot,
                            permission,
                            key_id,
                            material,
                        }),
                        fault,
                    )
                };
                // The access point's checks refuse both before the command
                // reaches here; refused, the command comes back whole.
                let (Some(activity), Some(ap_key_id)) = (
                    env.shared().key_live.get(17 + usize::from(slot.index())),
                    crate::mac::crypto::ApGroupKeyId::new(key_id.get()),
                ) else {
                    return Err(back(slot, permission, material, Fault::GroupKeyIdRange));
                };
                let identity = RawKeyIdentity {
                    role: VifRole::AccessPoint,
                    slot: keys::SlotPlace::AccessPointGroup(slot.place()),
                    serial: env.serial(),
                    key_id,
                    activity,
                };
                if let Err((slot, fault)) = self.install_transmit(
                    device,
                    identity,
                    spp,
                    TransmitKeyScope::AccessPointGroup { key_id: ap_key_id },
                    slot,
                    &material,
                ) {
                    return Err(back(slot, permission, material, fault));
                }
                identity.activity.set(identity.serial);
                Ok(Some(brand::OwnedTerminal::GroupKeyInstalled(
                    brand::OwnedInstalledGroup::AccessPoint {
                        key: GroupKey {
                            identity,
                            brand: PhantomData,
                        },
                        permission,
                    },
                )))
            }
            AccessPointGroupCommand::Remove { permission, key } => {
                match self.remove::<ApPermission<'id, Id>>(device, key.identity) {
                    Ok(removing) => Ok(removing
                        .settle(permission)
                        .map(brand::OwnedTerminal::KeyRemoved)),
                    Err(fault) => Err(GroupFailed::Returned(
                        AccessPointGroupCommand::Remove { permission, key },
                        fault,
                    )),
                }
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
    // A lend leaves a buffer the held count holds, so the lends read here
    // are within it; refused, nothing is settled and both counts stay.
    let transferred = network.transferred();
    let held = rx_out
        .get()
        .checked_sub(transferred)
        .ok_or(Fault::ReceiveAccounting)?;
    rx_out.set(held);
    network.settle_transferred(transferred);
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
    ticket: bool,
    descriptor: Option<&Pin<&'static mut TxSlot<B>>>,
    queue: bool,
    pending: &mut Pending<'id, Id, D, B, O>,
    key: Option<&Installed<'id, Id>>,
) -> Option<Unpublished> {
    let step = if !observations {
        PublishStep::Observations
    } else if !ticket {
        PublishStep::Ticket
    } else if matches!(key, Some(Installed::Receive(_))) {
        PublishStep::ReceiveKey
    } else if pending
        .operation
        .execute()
        .mpdu
        .buffer
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
    keys: &'a [Option<KeyPlace<'id, Id>>],
    identity: Option<RawKeyIdentity>,
) -> Option<Option<&'a Installed<'id, Id>>> {
    let Some(identity) = identity else {
        return Some(None);
    };
    keys.iter()
        .flatten()
        .filter_map(KeyPlace::installed)
        .find(|(key, _)| **key == identity)
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

/// A group mutation of one role, as the driver runs it once its receive cut
/// drained.
pub(super) trait GroupMutation<'id, Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations>:
    Sized
{
    /// Run the mutation.
    ///
    /// # Errors
    ///
    /// The mutation's [`GroupFailed`].
    fn run(
        self,
        driver: &mut Driver<'id, Id, D, B, O>,
        env: &mut impl DriverEnv<'id, Id, D, B, O>,
        device: &mut D,
    ) -> Result<Option<brand::OwnedTerminal<'id, Id, D, B>>, GroupFailed<Self>>;
}
impl<'id, Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations>
    GroupMutation<'id, Id, D, B, O> for StationGroupCommand<'id, Id>
{
    fn run(
        self,
        driver: &mut Driver<'id, Id, D, B, O>,
        env: &mut impl DriverEnv<'id, Id, D, B, O>,
        device: &mut D,
    ) -> Result<Option<brand::OwnedTerminal<'id, Id, D, B>>, GroupFailed<Self>> {
        driver.group_station(env, device, self)
    }
}
impl<'id, Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations>
    GroupMutation<'id, Id, D, B, O> for AccessPointGroupCommand<'id, Id>
{
    fn run(
        self,
        driver: &mut Driver<'id, Id, D, B, O>,
        env: &mut impl DriverEnv<'id, Id, D, B, O>,
        device: &mut D,
    ) -> Result<Option<brand::OwnedTerminal<'id, Id, D, B>>, GroupFailed<Self>> {
        driver.group_access_point(env, device, self)
    }
}

/// Why a group mutation failed in the driver.
pub(super) enum GroupFailed<C> {
    /// A step refused before it took the command's owners, or a station
    /// group key replacement the hardware rolled back: the command, whole.
    Returned(C, Fault),
    /// A station group key replacement whose rollback failed: the port
    /// keeps its owners until its stop's cleanup returns them
    /// ([`Driver::clean_retained_group`]).
    Retained(Fault),
}

/// Why a station group key replacement failed: `Err` when the previous key
/// stands (refused before the table changed, or rolled back), `Ok` with the
/// replacement's and the rollback's causes when neither key's entry is
/// installed.
pub(super) const fn replacement_failed(
    error: ReplaceError,
) -> Result<(CryptoKeyError, CryptoKeyError), GroupReplacement> {
    match error {
        ReplaceError::InvalidReplacement(_) | ReplaceError::CurrentMaterialMismatch => {
            Err(GroupReplacement::Refused)
        }
        ReplaceError::ReplacementRolledBack(cause) => Err(GroupReplacement::RolledBack(cause)),
        ReplaceError::RollbackFailed {
            replacement,
            rollback,
        } => Ok((replacement, rollback)),
    }
}

impl<'id, Id: SlotIdentity, R: RemovalRights<'id, Id>> Removing<'_, 'id, Id, R> {
    /// The terminal of a removal its command made, with the command's
    /// `rights`: now, or, while its key's last use is out, with the rights
    /// kept in the key's table place this removal emptied until
    /// [`Driver::finished_removal`] delivers it.
    pub(super) fn settle(self, rights: R) -> Option<brand::OwnedRemoved<'id, Id>> {
        match self.ended {
            Removed::Now(removed) => Some(rights.terminal(removed.into_slot())),
            Removed::Waiting(core) => {
                *self.place = Some(KeyPlace::Removing(R::waiting(Waiting { core, rights })));
                None
            }
        }
    }
}

/// The terminal of a waiting removal whose receipt the core has, with its
/// rights; otherwise the waiting removal, unchanged.
fn finish<'id, Id: SlotIdentity, D: Device<B>, const B: usize, R: RemovalRights<'id, Id>>(
    core: &mut PortCoreOwner<'id, Id, D, B>,
    waiting: Waiting<'id, Id, R>,
) -> Result<brand::OwnedRemoved<'id, Id>, WaitingRemoval<'id, Id>> {
    let Waiting {
        core: place,
        rights,
    } = waiting;
    match core.view().take_removed(place) {
        Ok(removed) => Ok(rights.terminal(removed.into_slot())),
        Err(place) => Err(R::waiting(Waiting {
            core: place,
            rights,
        })),
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

#[cfg(test)]
mod replacement_tests {
    use super::{CryptoKeyError, GroupReplacement, ReplaceError, replacement_failed};

    /// A refusal before the table changed and a rollback both leave the
    /// previous key standing, so their command comes back whole; only a
    /// failed rollback is retained, with both causes.
    #[test]
    fn a_replacement_is_retained_only_when_its_rollback_failed() {
        let cause = CryptoKeyError::HardwareRejected;
        assert_eq!(
            replacement_failed(ReplaceError::CurrentMaterialMismatch),
            Err(GroupReplacement::Refused)
        );
        assert_eq!(
            replacement_failed(ReplaceError::InvalidReplacement(cause)),
            Err(GroupReplacement::Refused)
        );
        assert_eq!(
            replacement_failed(ReplaceError::ReplacementRolledBack(cause)),
            Err(GroupReplacement::RolledBack(cause))
        );
        assert_eq!(
            replacement_failed(ReplaceError::RollbackFailed {
                replacement: cause,
                rollback: CryptoKeyError::Occupied,
            }),
            Ok((cause, CryptoKeyError::Occupied))
        );
    }
}
