//! Bounded application handoff for standalone-monitor frame injection.
//!
//! This module owns no radio registers or DMA descriptor.  It fences every
//! accepted no-FCS MPDU to one exact monitor dwell, moves its bytes by value
//! through a fixed lock-free request queue, and reserves completion capacity
//! before admitting a request.  A chip role may bind the scheduler-side owner
//! only after it can prove a physical monitor TX route.
//! Epoch creation reserves the next invalidation value before changing storage.
//! Neither mailbox epochs nor request IDs wrap: exhaustion refuses admission,
//! preserving already queued bytes and completion capacity.
//!
//! Nothing here waits for a lock. The application endpoint and the scheduler
//! share only atomics, the two SPSC queues and the active dwell's binding in a
//! [`ClaimCell`]: a caller that finds the binding lent to the other side at
//! that instant is refused with a typed result and retries later.

use core::sync::atomic::{AtomicU32, Ordering};

use oer_ieee80211_port::{
    MonitorInjectionChannelBinding, MonitorInjectionRate, MonitorInjectionRequest, MonitorSink,
};
use oer_ieee80211_upper_mac::monitor::{MonitorInjectionFrameError, stored_injection_request};
use oer_memory::{
    AffineSpscTrySendError, ClaimCell, WakingSpscQueue, WakingSpscReceiver, WakingSpscSender,
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
    /// The scheduler is publishing or ending a dwell at this instant.
    DwellChanging,
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
    /// The application is reading the dwell's binding at this instant; the
    /// mailbox did not change.
    BindingBusy,
}

/// One admitted request: its identity, dwell, rate and no-FCS bytes, moved by
/// value from the application to the scheduler.
struct QueuedMonitorInjection<const FRAME_CAPACITY: usize> {
    ticket: MonitorInjectionTicket,
    binding: MonitorInjectionChannelBinding,
    rate: MonitorInjectionRate,
    length: usize,
    bytes: [u8; FRAME_CAPACITY],
}

/// A request whose terminal found the completion queue full, returned whole.
type Unpublished<const FRAME_CAPACITY: usize> = (
    QueuedMonitorInjection<FRAME_CAPACITY>,
    MonitorInjectionMailboxInvariantError,
);

/// Static request/completion queues and the active dwell's binding.
///
/// `outstanding` counts queued, active and completed-but-unread tickets.  It
/// cannot exceed `CAPACITY`, so every accepted request already owns space for
/// its terminal completion during stop or a channel-hop drain.
pub struct MonitorInjectionMailboxResources<const CAPACITY: usize, const FRAME_CAPACITY: usize> {
    requests: WakingSpscQueue<QueuedMonitorInjection<FRAME_CAPACITY>, CAPACITY>,
    completions: WakingSpscQueue<MonitorInjectionCompletion, CAPACITY>,
    generation: AtomicU32,
    next_request: AtomicU32,
    outstanding: AtomicU32,
    publishers: AtomicU32,
    active_binding: ClaimCell<Option<MonitorInjectionChannelBinding>>,
}

impl<const CAPACITY: usize, const FRAME_CAPACITY: usize>
    MonitorInjectionMailboxResources<CAPACITY, FRAME_CAPACITY>
{
    pub const fn new() -> Self {
        Self {
            requests: WakingSpscQueue::new(),
            completions: WakingSpscQueue::new(),
            generation: AtomicU32::new(0),
            next_request: AtomicU32::new(0),
            outstanding: AtomicU32::new(0),
            publishers: AtomicU32::new(0),
            active_binding: ClaimCell::new(),
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
            MonitorInjectionHandle<'_, CAPACITY, FRAME_CAPACITY>,
            MonitorInjectionMailboxOwner<'_, CAPACITY, FRAME_CAPACITY>,
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

        let (request_sender, request_receiver) = self.requests.split();
        let (completion_sender, completion_receiver) = self.completions.split();
        while request_receiver.try_receive().is_some() {}
        while completion_receiver.try_receive().is_some() {}
        // The exclusive borrow ended every lend of the previous epoch; a fresh
        // cell is empty, so its first install is never refused.
        self.active_binding = ClaimCell::new();
        let _installed = self.active_binding.install(None);
        self.next_request.store(0, Ordering::Release);
        self.outstanding.store(0, Ordering::Release);
        self.publishers.store(0, Ordering::Release);
        self.generation.store(epoch, Ordering::Release);

        Ok((
            MonitorInjectionHandle {
                requests: request_sender,
                completions: completion_receiver,
                generation: &self.generation,
                next_request: &self.next_request,
                outstanding: &self.outstanding,
                publishers: &self.publishers,
                active_binding: &self.active_binding,
                epoch,
            },
            MonitorInjectionMailboxOwner {
                requests: request_receiver,
                completions: completion_sender,
                generation: &self.generation,
                publishers: &self.publishers,
                active_binding: &self.active_binding,
                epoch,
                closed_epoch,
                open: true,
                active: None,
            },
        ))
    }
}

impl<const CAPACITY: usize, const FRAME_CAPACITY: usize> Default
    for MonitorInjectionMailboxResources<CAPACITY, FRAME_CAPACITY>
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
pub struct MonitorInjectionHandle<'resources, const CAPACITY: usize, const FRAME_CAPACITY: usize>
{
    requests: WakingSpscSender<'resources, QueuedMonitorInjection<FRAME_CAPACITY>, CAPACITY>,
    completions: WakingSpscReceiver<'resources, MonitorInjectionCompletion, CAPACITY>,
    generation: &'resources AtomicU32,
    next_request: &'resources AtomicU32,
    outstanding: &'resources AtomicU32,
    publishers: &'resources AtomicU32,
    active_binding: &'resources ClaimCell<Option<MonitorInjectionChannelBinding>>,
    epoch: u32,
}

impl<const CAPACITY: usize, const FRAME_CAPACITY: usize>
    MonitorInjectionHandle<'_, CAPACITY, FRAME_CAPACITY>
{
    pub const fn mailbox_epoch(&self) -> u32 {
        self.epoch
    }

    /// Currently published monitor dwell, if admission is open, or
    /// [`MonitorInjectionBackpressure::DwellChanging`] while the scheduler
    /// changes it at this instant.
    pub fn active_binding(
        &self,
    ) -> Result<Option<MonitorInjectionChannelBinding>, MonitorInjectionBackpressure> {
        self.active_binding
            .lend(|active| *active)
            .map_err(|_| MonitorInjectionBackpressure::DwellChanging)
    }

    /// Validate and copy one no-FCS request into the request queue.
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
        let mut bytes = [0; FRAME_CAPACITY];
        for (to, from) in bytes.iter_mut().zip(request.mpdu().bytes()) {
            *to = *from;
        }
        match self.requests.try_send(QueuedMonitorInjection {
            ticket,
            binding: request.binding(),
            rate: request.rate(),
            length: requested_length,
            bytes,
        }) {
            Ok(()) => Ok(ticket),
            Err(AffineSpscTrySendError(_queued)) => {
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
        match self.active_binding()? {
            Some(active) if active == requested => Ok(()),
            Some(active) => {
                Err(MonitorInjectionBackpressure::ChannelBindingMismatch { requested, active })
            }
            None => Err(MonitorInjectionBackpressure::NoActiveDwell),
        }
    }

    pub fn try_receive(&self) -> Option<MonitorInjectionCompletion> {
        loop {
            let completion = self.completions.try_receive()?;
            if completion.ticket.mailbox_epoch == self.epoch {
                self.outstanding.fetch_sub(1, Ordering::AcqRel);
                return Some(completion);
            }
        }
    }

    /// Wait for the next completion of this epoch. Cancellation leaves it
    /// queued.
    pub async fn receive(&mut self) -> MonitorInjectionCompletion {
        loop {
            self.completions.ready().await;
            if let Some(completion) = self.try_receive() {
                return completion;
            }
        }
    }

    pub fn outstanding(&self) -> u32 {
        self.outstanding.load(Ordering::Acquire)
    }
}

/// Scheduler-side mailbox owner.  Its `active` request enforces one physical
/// ordinary TX at a time and prevents a dwell from closing while TX is live.
pub struct MonitorInjectionMailboxOwner<
    'resources,
    const CAPACITY: usize,
    const FRAME_CAPACITY: usize,
> {
    requests: WakingSpscReceiver<'resources, QueuedMonitorInjection<FRAME_CAPACITY>, CAPACITY>,
    completions: WakingSpscSender<'resources, MonitorInjectionCompletion, CAPACITY>,
    generation: &'resources AtomicU32,
    publishers: &'resources AtomicU32,
    active_binding: &'resources ClaimCell<Option<MonitorInjectionChannelBinding>>,
    epoch: u32,
    // Proved representable by begin_epoch before the binding can open.
    closed_epoch: u32,
    open: bool,
    active: Option<QueuedMonitorInjection<FRAME_CAPACITY>>,
}

impl<const CAPACITY: usize, const FRAME_CAPACITY: usize>
    MonitorInjectionMailboxOwner<'_, CAPACITY, FRAME_CAPACITY>
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
        match &self.active {
            Some(active) => Some(active.ticket),
            None => None,
        }
    }

    /// Wait until a request is queued. Cancellation consumes nothing.
    pub async fn ready(&mut self) {
        self.requests.ready().await;
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
        self.active_binding
            .lend(|active| {
                if active.is_some() {
                    return Err(MonitorInjectionMailboxInvariantError::DwellAlreadyActive);
                }
                *active = Some(binding);
                Ok(())
            })
            .map_err(|_| MonitorInjectionMailboxInvariantError::BindingBusy)?
    }

    fn set_binding(
        &self,
        binding: Option<MonitorInjectionChannelBinding>,
    ) -> Result<(), MonitorInjectionMailboxInvariantError> {
        self.active_binding
            .lend(|active| *active = binding)
            .map_err(|_| MonitorInjectionMailboxInvariantError::BindingBusy)
    }

    /// Close admission first, then drain all queued requests for this dwell.
    /// An active physical TX must reach its IRQ/deadline terminal edge before
    /// this succeeds, which makes a caller's following retune safe.
    pub fn end_dwell(&mut self) -> Result<u32, MonitorInjectionMailboxInvariantError> {
        self.set_binding(None)?;
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
        let active = self
            .active_binding
            .lend(|active| *active)
            .map_err(|_| MonitorInjectionMailboxInvariantError::BindingBusy)?;
        loop {
            let Some(queued) = self.requests.try_receive() else {
                return Ok(None);
            };
            if active != Some(queued.binding) {
                if let Err((queued, error)) = self.publish_queued(
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
            let ticket = queued.ticket;
            self.active = Some(queued);
            return Ok(Some(ticket));
        }
    }

    /// Borrow the exact retained bytes of the active request.
    pub fn with_active_request<R>(
        &self,
        use_request: impl FnOnce(MonitorInjectionRequest<'_>) -> R,
    ) -> Result<R, MonitorInjectionMailboxInvariantError> {
        let active = self
            .active
            .as_ref()
            .ok_or(MonitorInjectionMailboxInvariantError::MissingPayloadSlot)?;
        let frame = active
            .bytes
            .get(..active.length)
            .ok_or(MonitorInjectionMailboxInvariantError::MissingPayloadSlot)?;
        let request = stored_injection_request(active.binding, active.rate, frame)
            .map_err(MonitorInjectionMailboxInvariantError::CorruptStoredFrame)?;
        Ok(use_request(request))
    }

    pub fn complete_active(
        &mut self,
        terminal: MonitorInjectionTerminal,
    ) -> Result<MonitorInjectionTicket, MonitorInjectionMailboxInvariantError> {
        let active = self
            .active
            .take()
            .ok_or(MonitorInjectionMailboxInvariantError::MissingPayloadSlot)?;
        let ticket = active.ticket;
        if let Err((active, error)) = self.publish_queued(active, terminal) {
            self.active = Some(active);
            return Err(error);
        }
        Ok(ticket)
    }

    /// Invalidate this epoch and close its dwell. A binding lent to the
    /// application at this instant refuses the close; calling it again
    /// completes it.
    pub fn close(&mut self) -> Result<(), MonitorInjectionMailboxInvariantError> {
        if !self.open {
            return Ok(());
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

    fn finish_close(&mut self) -> Result<(), MonitorInjectionMailboxInvariantError> {
        self.set_binding(None)?;
        self.open = false;
        Ok(())
    }

    pub fn cancel_pending(
        &mut self,
        reason: MonitorInjectionCancelReason,
    ) -> Result<u32, MonitorInjectionMailboxInvariantError> {
        let mut cancelled = 0_u32;
        while let Some(queued) = self.requests.try_receive() {
            if let Err((queued, error)) =
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
        if let Err(error) = self.close() {
            return Err((self, error));
        }
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

    /// Publish the request's terminal; a full completion queue returns the
    /// request unchanged.
    fn publish_queued(
        &self,
        queued: QueuedMonitorInjection<FRAME_CAPACITY>,
        terminal: MonitorInjectionTerminal,
    ) -> Result<(), Unpublished<FRAME_CAPACITY>> {
        match self.completions.try_send(MonitorInjectionCompletion {
            ticket: queued.ticket,
            binding: queued.binding,
            terminal,
        }) {
            Ok(()) => Ok(()),
            Err(AffineSpscTrySendError(_)) => Err((
                queued,
                MonitorInjectionMailboxInvariantError::CompletionQueueFull,
            )),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MonitorInjectionMailboxShutdown {
    pub mailbox_epoch: u32,
    pub cancelled: u32,
}

#[cfg(test)]
mod tests;
