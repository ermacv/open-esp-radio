//! Bounded application handoff for standalone-monitor frame injection.
//!
//! This module owns no radio registers or DMA descriptor.  It fences every
//! accepted no-FCS MPDU to one exact monitor dwell, retains bytes in a fixed
//! caller-provisioned slot, and reserves completion capacity before admitting
//! a request.  A chip role may bind the scheduler-side owner only after it can
//! prove a physical monitor TX route.
//! Epoch creation reserves the next invalidation value before changing storage.
//! Neither mailbox epochs nor request IDs wrap: exhaustion refuses admission,
//! preserving already queued bytes and completion capacity.

use core::{
    cell::RefCell,
    sync::atomic::{AtomicU32, Ordering},
};

use embassy_sync::{
    blocking_mutex::{Mutex as BlockingMutex, raw::RawMutex},
    channel::{Channel, Receiver, Sender, TrySendError},
};
use oer_ieee80211_softmac::{
    MonitorInjectionChannelBinding, MonitorInjectionFrameError, MonitorInjectionMpdu,
    MonitorInjectionRate, MonitorInjectionRequest, MonitorSink,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MonitorInjectionTicket {
    mailbox_epoch: u32,
    request: u32,
}

impl MonitorInjectionTicket {
    pub const fn mailbox_epoch(self) -> u32 {
        self.mailbox_epoch
    }

    pub const fn request(self) -> u32 {
        self.request
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MonitorInjectionCancelReason {
    MonitorStopped,
    DwellEnded,
    StaleChannelBinding,
}

/// Portable terminal result.  A backend maps its typed ordinary-TX report
/// only after the descriptor has reached a terminal state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MonitorInjectionTerminal {
    Transmitted {
        attempts: u8,
        /// Group-addressed MPDUs have no ACK exchange and must publish `None`.
        acknowledged: Option<bool>,
    },
    HardwareFailure,
    DeadlineExpired,
    Cancelled(MonitorInjectionCancelReason),
    BackendUnsupported,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MonitorInjectionCompletion {
    pub ticket: MonitorInjectionTicket,
    pub binding: MonitorInjectionChannelBinding,
    pub terminal: MonitorInjectionTerminal,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MonitorInjectionMailboxEpochError {
    ZeroCapacity,
    ZeroFrameCapacity,
    GenerationExhausted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MonitorInjectionBackpressure {
    StaleMailboxEpoch,
    NoActiveDwell,
    ChannelBindingMismatch {
        requested: MonitorInjectionChannelBinding,
        active: MonitorInjectionChannelBinding,
    },
    FrameTooLong {
        requested: usize,
        capacity: usize,
    },
    QueueFull,
    RequestIdExhausted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MonitorInjectionMailboxInvariantError {
    Closed,
    MissingTaskDwellBinding,
    PublisherInFlight,
    ActiveTransmit,
    PendingFromEarlierDwell,
    DwellAlreadyActive,
    MissingPayloadSlot,
    CorruptStoredFrame(MonitorInjectionFrameError),
    CompletionQueueFull,
}

#[derive(Clone, Copy)]
struct MonitorInjectionSlotHeader {
    ticket: MonitorInjectionTicket,
    binding: MonitorInjectionChannelBinding,
    rate: MonitorInjectionRate,
    length: usize,
}

struct MonitorInjectionSlot<const FRAME_CAPACITY: usize> {
    header: Option<MonitorInjectionSlotHeader>,
    bytes: [u8; FRAME_CAPACITY],
}

impl<const FRAME_CAPACITY: usize> MonitorInjectionSlot<FRAME_CAPACITY> {
    const EMPTY: Self = Self {
        header: None,
        bytes: [0; FRAME_CAPACITY],
    };
}

#[derive(Clone, Copy)]
struct QueuedMonitorInjection {
    ticket: MonitorInjectionTicket,
    slot: usize,
}

/// Static request/completion queues and fixed payload slots.
///
/// `outstanding` counts queued, active and completed-but-unread tickets.  It
/// cannot exceed `CAPACITY`, so every accepted request already owns space for
/// its terminal completion during stop or a channel-hop drain.
pub struct MonitorInjectionMailboxResources<
    M: RawMutex,
    const CAPACITY: usize,
    const FRAME_CAPACITY: usize,
> {
    requests: Channel<M, QueuedMonitorInjection, CAPACITY>,
    completions: Channel<M, MonitorInjectionCompletion, CAPACITY>,
    generation: AtomicU32,
    next_request: AtomicU32,
    outstanding: AtomicU32,
    publishers: AtomicU32,
    active_binding: BlockingMutex<M, RefCell<Option<MonitorInjectionChannelBinding>>>,
    slots: BlockingMutex<M, RefCell<[MonitorInjectionSlot<FRAME_CAPACITY>; CAPACITY]>>,
}

impl<M: RawMutex, const CAPACITY: usize, const FRAME_CAPACITY: usize>
    MonitorInjectionMailboxResources<M, CAPACITY, FRAME_CAPACITY>
{
    pub const fn new() -> Self {
        Self {
            requests: Channel::new(),
            completions: Channel::new(),
            generation: AtomicU32::new(0),
            next_request: AtomicU32::new(0),
            outstanding: AtomicU32::new(0),
            publishers: AtomicU32::new(0),
            active_binding: BlockingMutex::new(RefCell::new(None)),
            slots: BlockingMutex::new(RefCell::new(
                [const { MonitorInjectionSlot::EMPTY }; CAPACITY],
            )),
        }
    }

    pub const fn capacity() -> usize {
        CAPACITY
    }

    /// Maximum no-FCS MPDU bytes copied by one admitted request.
    pub const fn frame_capacity() -> usize {
        FRAME_CAPACITY
    }

    /// Start one fresh application/scheduler ownership epoch.
    pub fn begin_epoch(
        &mut self,
    ) -> Result<
        (
            MonitorInjectionHandle<'_, M, CAPACITY, FRAME_CAPACITY>,
            MonitorInjectionMailboxOwner<'_, M, CAPACITY, FRAME_CAPACITY>,
        ),
        MonitorInjectionMailboxEpochError,
    > {
        if CAPACITY == 0 {
            return Err(MonitorInjectionMailboxEpochError::ZeroCapacity);
        }
        if FRAME_CAPACITY == 0 {
            return Err(MonitorInjectionMailboxEpochError::ZeroFrameCapacity);
        }
        let current = self.generation.load(Ordering::Acquire);
        let epoch = current
            .checked_add(1)
            .ok_or(MonitorInjectionMailboxEpochError::GenerationExhausted)?;
        let closed_epoch = epoch
            .checked_add(1)
            .ok_or(MonitorInjectionMailboxEpochError::GenerationExhausted)?;

        let requests = self.requests.receiver();
        let completions = self.completions.receiver();
        while requests.try_receive().is_ok() {}
        while completions.try_receive().is_ok() {}
        for slot in self.slots.get_mut().get_mut() {
            slot.header = None;
        }
        *self.active_binding.get_mut().get_mut() = None;
        self.next_request.store(0, Ordering::Release);
        self.outstanding.store(0, Ordering::Release);
        self.publishers.store(0, Ordering::Release);
        self.generation.store(epoch, Ordering::Release);

        Ok((
            MonitorInjectionHandle {
                requests: self.requests.sender(),
                completions,
                generation: &self.generation,
                next_request: &self.next_request,
                outstanding: &self.outstanding,
                publishers: &self.publishers,
                active_binding: &self.active_binding,
                slots: &self.slots,
                epoch,
            },
            MonitorInjectionMailboxOwner {
                requests,
                completions: self.completions.sender(),
                generation: &self.generation,
                publishers: &self.publishers,
                active_binding: &self.active_binding,
                slots: &self.slots,
                epoch,
                closed_epoch,
                open: true,
                active: None,
            },
        ))
    }
}

impl<M: RawMutex, const CAPACITY: usize, const FRAME_CAPACITY: usize> Default
    for MonitorInjectionMailboxResources<M, CAPACITY, FRAME_CAPACITY>
{
    fn default() -> Self {
        Self::new()
    }
}

struct MonitorInjectionPublicationLease<'resources> {
    publishers: &'resources AtomicU32,
}

impl Drop for MonitorInjectionPublicationLease<'_> {
    fn drop(&mut self) {
        self.publishers.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Application-side non-blocking injection endpoint.
pub struct MonitorInjectionHandle<
    'resources,
    M: RawMutex,
    const CAPACITY: usize,
    const FRAME_CAPACITY: usize,
> {
    requests: Sender<'resources, M, QueuedMonitorInjection, CAPACITY>,
    completions: Receiver<'resources, M, MonitorInjectionCompletion, CAPACITY>,
    generation: &'resources AtomicU32,
    next_request: &'resources AtomicU32,
    outstanding: &'resources AtomicU32,
    publishers: &'resources AtomicU32,
    active_binding: &'resources BlockingMutex<M, RefCell<Option<MonitorInjectionChannelBinding>>>,
    slots: &'resources BlockingMutex<M, RefCell<[MonitorInjectionSlot<FRAME_CAPACITY>; CAPACITY]>>,
    epoch: u32,
}

impl<M: RawMutex, const CAPACITY: usize, const FRAME_CAPACITY: usize>
    MonitorInjectionHandle<'_, M, CAPACITY, FRAME_CAPACITY>
{
    pub const fn mailbox_epoch(&self) -> u32 {
        self.epoch
    }

    /// Currently published monitor dwell, if admission is open.
    pub fn active_binding(&self) -> Option<MonitorInjectionChannelBinding> {
        self.active_binding.lock(|binding| *binding.borrow())
    }

    /// Validate and copy one no-FCS request into caller-provisioned storage.
    pub fn try_send(
        &self,
        request: MonitorInjectionRequest<'_>,
    ) -> Result<MonitorInjectionTicket, MonitorInjectionBackpressure> {
        let requested_length = request.mpdu().len();
        if requested_length > FRAME_CAPACITY {
            return Err(MonitorInjectionBackpressure::FrameTooLong {
                requested: requested_length,
                capacity: FRAME_CAPACITY,
            });
        }
        if self.generation.load(Ordering::Acquire) != self.epoch {
            return Err(MonitorInjectionBackpressure::StaleMailboxEpoch);
        }

        self.publishers.fetch_add(1, Ordering::AcqRel);
        let _publication = MonitorInjectionPublicationLease {
            publishers: self.publishers,
        };
        if self.generation.load(Ordering::Acquire) != self.epoch {
            return Err(MonitorInjectionBackpressure::StaleMailboxEpoch);
        }
        self.require_active_binding(request.binding())?;

        let request_id = self.reserve_request_id()?;
        if self
            .outstanding
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                if usize::try_from(current).unwrap_or(usize::MAX) < CAPACITY {
                    current.checked_add(1)
                } else {
                    None
                }
            })
            .is_err()
        {
            return Err(MonitorInjectionBackpressure::QueueFull);
        }

        let ticket = MonitorInjectionTicket {
            mailbox_epoch: self.epoch,
            request: request_id,
        };
        let slot = self.slots.lock(|slots| {
            let mut slots = slots.borrow_mut();
            let (index, slot) = slots
                .iter_mut()
                .enumerate()
                .find(|(_, slot)| slot.header.is_none())?;
            let output = slot.bytes.split_at_mut_checked(requested_length)?.0;
            for (to, from) in output.iter_mut().zip(request.mpdu().bytes()) {
                *to = *from;
            }
            slot.header = Some(MonitorInjectionSlotHeader {
                ticket,
                binding: request.binding(),
                rate: request.rate(),
                length: requested_length,
            });
            Some(index)
        });
        let Some(slot) = slot else {
            self.outstanding.fetch_sub(1, Ordering::AcqRel);
            return Err(MonitorInjectionBackpressure::QueueFull);
        };
        match self
            .requests
            .try_send(QueuedMonitorInjection { ticket, slot })
        {
            Ok(()) => Ok(ticket),
            Err(TrySendError::Full(queued)) => {
                self.release_slot(queued);
                self.outstanding.fetch_sub(1, Ordering::AcqRel);
                Err(MonitorInjectionBackpressure::QueueFull)
            }
        }
    }

    fn reserve_request_id(&self) -> Result<u32, MonitorInjectionBackpressure> {
        let mut current = self.next_request.load(Ordering::Acquire);
        loop {
            let next = current
                .checked_add(1)
                .ok_or(MonitorInjectionBackpressure::RequestIdExhausted)?;
            match self.next_request.compare_exchange_weak(
                current,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Ok(next),
                Err(observed) => current = observed,
            }
        }
    }

    fn require_active_binding(
        &self,
        requested: MonitorInjectionChannelBinding,
    ) -> Result<(), MonitorInjectionBackpressure> {
        let active = self.active_binding();
        match active {
            Some(active) if active == requested => Ok(()),
            Some(active) => {
                Err(MonitorInjectionBackpressure::ChannelBindingMismatch { requested, active })
            }
            None => Err(MonitorInjectionBackpressure::NoActiveDwell),
        }
    }

    fn release_slot(&self, queued: QueuedMonitorInjection) {
        self.slots.lock(|slots| {
            let mut slots = slots.borrow_mut();
            if let Some(slot) = slots.get_mut(queued.slot)
                && slot
                    .header
                    .is_some_and(|header| header.ticket == queued.ticket)
            {
                slot.header = None;
            }
        });
    }

    pub fn try_receive(&self) -> Option<MonitorInjectionCompletion> {
        loop {
            let completion = self.completions.try_receive().ok()?;
            if completion.ticket.mailbox_epoch == self.epoch {
                self.outstanding.fetch_sub(1, Ordering::AcqRel);
                return Some(completion);
            }
        }
    }

    pub async fn receive(&self) -> MonitorInjectionCompletion {
        loop {
            let completion = self.completions.receive().await;
            if completion.ticket.mailbox_epoch == self.epoch {
                self.outstanding.fetch_sub(1, Ordering::AcqRel);
                return completion;
            }
        }
    }

    pub fn outstanding(&self) -> u32 {
        self.outstanding.load(Ordering::Acquire)
    }
}

/// Scheduler-side mailbox owner.  Its `active` lease enforces one physical
/// ordinary TX at a time and prevents a dwell from closing while TX is live.
pub struct MonitorInjectionMailboxOwner<
    'resources,
    M: RawMutex,
    const CAPACITY: usize,
    const FRAME_CAPACITY: usize,
> {
    requests: Receiver<'resources, M, QueuedMonitorInjection, CAPACITY>,
    completions: Sender<'resources, M, MonitorInjectionCompletion, CAPACITY>,
    generation: &'resources AtomicU32,
    publishers: &'resources AtomicU32,
    active_binding: &'resources BlockingMutex<M, RefCell<Option<MonitorInjectionChannelBinding>>>,
    slots: &'resources BlockingMutex<M, RefCell<[MonitorInjectionSlot<FRAME_CAPACITY>; CAPACITY]>>,
    epoch: u32,
    // Proved representable by begin_epoch before the binding can open.
    closed_epoch: u32,
    open: bool,
    active: Option<QueuedMonitorInjection>,
}

impl<M: RawMutex, const CAPACITY: usize, const FRAME_CAPACITY: usize>
    MonitorInjectionMailboxOwner<'_, M, CAPACITY, FRAME_CAPACITY>
{
    pub const fn mailbox_epoch(&self) -> u32 {
        self.epoch
    }

    pub const fn is_open(&self) -> bool {
        self.open
    }

    pub fn publishers_in_flight(&self) -> u32 {
        self.publishers.load(Ordering::Acquire)
    }

    pub fn has_pending(&self) -> bool {
        !self.requests.is_empty()
    }

    pub const fn active_ticket(&self) -> Option<MonitorInjectionTicket> {
        match self.active {
            Some(active) => Some(active.ticket),
            None => None,
        }
    }

    pub async fn ready(&self) {
        self.requests.ready_to_receive().await;
    }

    /// Publish authority derived from the actual monitor task/sink owner.
    /// A caller-supplied request binding is never accepted as the active
    /// binding merely because it has the same public value shape.
    pub fn begin_task_dwell<Rate, S: MonitorSink<Rate>>(
        &mut self,
        sink: &S,
    ) -> Result<MonitorInjectionChannelBinding, MonitorInjectionMailboxInvariantError> {
        let binding = sink
            .injection_channel_binding()
            .ok_or(MonitorInjectionMailboxInvariantError::MissingTaskDwellBinding)?;
        self.begin_bound_dwell(binding)?;
        Ok(binding)
    }

    fn begin_bound_dwell(
        &mut self,
        binding: MonitorInjectionChannelBinding,
    ) -> Result<(), MonitorInjectionMailboxInvariantError> {
        if !self.open {
            return Err(MonitorInjectionMailboxInvariantError::Closed);
        }
        if self.publishers_in_flight() != 0 {
            return Err(MonitorInjectionMailboxInvariantError::PublisherInFlight);
        }
        if self.active.is_some() {
            return Err(MonitorInjectionMailboxInvariantError::ActiveTransmit);
        }
        if self.has_pending() {
            return Err(MonitorInjectionMailboxInvariantError::PendingFromEarlierDwell);
        }
        self.active_binding.lock(|active| {
            let mut active = active.borrow_mut();
            if active.is_some() {
                return Err(MonitorInjectionMailboxInvariantError::DwellAlreadyActive);
            }
            *active = Some(binding);
            Ok(())
        })
    }

    /// Close admission first, then drain all queued requests for this dwell.
    /// An active physical TX must reach its IRQ/deadline terminal edge before
    /// this succeeds, which makes a caller's following retune safe.
    pub fn end_dwell(&mut self) -> Result<u32, MonitorInjectionMailboxInvariantError> {
        self.active_binding
            .lock(|active| *active.borrow_mut() = None);
        if self.publishers_in_flight() != 0 {
            return Err(MonitorInjectionMailboxInvariantError::PublisherInFlight);
        }
        if self.active.is_some() {
            return Err(MonitorInjectionMailboxInvariantError::ActiveTransmit);
        }
        self.cancel_pending(MonitorInjectionCancelReason::DwellEnded)
    }

    /// Reserve the ordered queue head for one physical transaction.
    pub fn try_take(
        &mut self,
    ) -> Result<Option<MonitorInjectionTicket>, MonitorInjectionMailboxInvariantError> {
        if self.active.is_some() {
            return Err(MonitorInjectionMailboxInvariantError::ActiveTransmit);
        }
        loop {
            let Ok(queued) = self.requests.try_receive() else {
                return Ok(None);
            };
            let binding = self.header(queued)?.binding;
            let active = self.active_binding.lock(|active| *active.borrow());
            if active != Some(binding) {
                if let Err(error) = self.publish_queued(
                    queued,
                    MonitorInjectionTerminal::Cancelled(
                        MonitorInjectionCancelReason::StaleChannelBinding,
                    ),
                ) {
                    self.active = Some(queued);
                    return Err(error);
                }
                continue;
            }
            self.active = Some(queued);
            return Ok(Some(queued.ticket));
        }
    }

    /// Borrow the exact retained bytes while keeping their slot affine.
    pub fn with_active_request<R>(
        &self,
        use_request: impl FnOnce(MonitorInjectionRequest<'_>) -> R,
    ) -> Result<R, MonitorInjectionMailboxInvariantError> {
        let active = self
            .active
            .ok_or(MonitorInjectionMailboxInvariantError::MissingPayloadSlot)?;
        self.slots.lock(|slots| {
            let slots = slots.borrow();
            let slot = slots
                .get(active.slot)
                .ok_or(MonitorInjectionMailboxInvariantError::MissingPayloadSlot)?;
            let header = slot
                .header
                .ok_or(MonitorInjectionMailboxInvariantError::MissingPayloadSlot)?;
            if header.ticket != active.ticket || header.length > FRAME_CAPACITY {
                return Err(MonitorInjectionMailboxInvariantError::MissingPayloadSlot);
            }
            let mpdu = MonitorInjectionMpdu::from_no_fcs(&slot.bytes[..header.length])
                .map_err(MonitorInjectionMailboxInvariantError::CorruptStoredFrame)?;
            Ok(use_request(MonitorInjectionRequest::new(
                header.binding,
                header.rate,
                mpdu,
            )))
        })
    }

    pub fn complete_active(
        &mut self,
        terminal: MonitorInjectionTerminal,
    ) -> Result<MonitorInjectionTicket, MonitorInjectionMailboxInvariantError> {
        let active = self
            .active
            .take()
            .ok_or(MonitorInjectionMailboxInvariantError::MissingPayloadSlot)?;
        if let Err(error) = self.publish_queued(active, terminal) {
            self.active = Some(active);
            return Err(error);
        }
        Ok(active.ticket)
    }

    pub fn close(&mut self) {
        if !self.open {
            return;
        }
        match self.generation.compare_exchange(
            self.epoch,
            self.closed_epoch,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            // Another invalidation already closed this epoch. begin_epoch
            // needs these resources exclusively and cannot overlap this
            // borrow, so both outcomes retire this same mailbox binding.
            Ok(_) | Err(_) => self.finish_close(),
        }
    }

    fn finish_close(&mut self) {
        self.active_binding
            .lock(|binding| *binding.borrow_mut() = None);
        self.open = false;
    }

    pub fn cancel_pending(
        &mut self,
        reason: MonitorInjectionCancelReason,
    ) -> Result<u32, MonitorInjectionMailboxInvariantError> {
        let mut cancelled = 0_u32;
        while let Ok(queued) = self.requests.try_receive() {
            if let Err(error) =
                self.publish_queued(queued, MonitorInjectionTerminal::Cancelled(reason))
            {
                self.active = Some(queued);
                return Err(error);
            }
            cancelled = cancelled.saturating_add(1);
        }
        Ok(cancelled)
    }

    pub fn shutdown(
        mut self,
    ) -> Result<MonitorInjectionMailboxShutdown, (Self, MonitorInjectionMailboxInvariantError)>
    {
        self.close();
        if self.publishers_in_flight() != 0 {
            return Err((
                self,
                MonitorInjectionMailboxInvariantError::PublisherInFlight,
            ));
        }
        if self.active.is_some() {
            return Err((self, MonitorInjectionMailboxInvariantError::ActiveTransmit));
        }
        let cancelled = match self.cancel_pending(MonitorInjectionCancelReason::MonitorStopped) {
            Ok(cancelled) => cancelled,
            Err(error) => return Err((self, error)),
        };
        Ok(MonitorInjectionMailboxShutdown {
            mailbox_epoch: self.epoch,
            cancelled,
        })
    }

    fn header(
        &self,
        queued: QueuedMonitorInjection,
    ) -> Result<MonitorInjectionSlotHeader, MonitorInjectionMailboxInvariantError> {
        self.slots.lock(|slots| {
            let slots = slots.borrow();
            let header = slots
                .get(queued.slot)
                .and_then(|slot| slot.header)
                .ok_or(MonitorInjectionMailboxInvariantError::MissingPayloadSlot)?;
            if header.ticket != queued.ticket {
                return Err(MonitorInjectionMailboxInvariantError::MissingPayloadSlot);
            }
            Ok(header)
        })
    }

    fn publish_queued(
        &self,
        queued: QueuedMonitorInjection,
        terminal: MonitorInjectionTerminal,
    ) -> Result<(), MonitorInjectionMailboxInvariantError> {
        self.slots.lock(|slots| {
            let mut slots = slots.borrow_mut();
            let slot = slots
                .get_mut(queued.slot)
                .ok_or(MonitorInjectionMailboxInvariantError::MissingPayloadSlot)?;
            let header = slot
                .header
                .filter(|header| header.ticket == queued.ticket)
                .ok_or(MonitorInjectionMailboxInvariantError::MissingPayloadSlot)?;
            self.completions
                .try_send(MonitorInjectionCompletion {
                    ticket: queued.ticket,
                    binding: header.binding,
                    terminal,
                })
                .map_err(|TrySendError::Full(_)| {
                    MonitorInjectionMailboxInvariantError::CompletionQueueFull
                })?;
            slot.header = None;
            Ok(())
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MonitorInjectionMailboxShutdown {
    pub mailbox_epoch: u32,
    pub cancelled: u32,
}

#[cfg(test)]
mod tests;
