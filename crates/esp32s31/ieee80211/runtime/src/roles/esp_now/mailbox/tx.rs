#![expect(
    clippy::result_large_err,
    reason = "bounded TX admission returns the caller-owned 250-byte request"
)]

//! No-allocation application handoff for plaintext ESP-NOW v1/v2 transmit.
//!
//! The application owns copied payloads and observes one terminal completion
//! for every admitted ticket. The connected-station scheduler and the
//! standalone ESP-NOW role drain the same mailbox; neither manufactures a PHY
//! rate.

use core::{
    cell::RefCell,
    sync::atomic::{AtomicU32, Ordering},
};

use embassy_sync::{
    blocking_mutex::{Mutex as BlockingMutex, raw::RawMutex},
    channel::{Channel, Receiver, Sender, TrySendError},
};

use oer_esp32s31_ieee80211_sta::single_mpdu_tx::{
    SingleMpduEspNowTxError, SingleMpduTxError, SingleMpduTxOutcome,
};

use oer_ieee80211_mac::extensions::espressif::esp_now::{
    ESP_NOW_V1_MAX_PAYLOAD_LEN, ESP_NOW_V2_MAX_PAYLOAD_LEN, EspNowRandomValue, EspNowV1Payload,
    EspNowV1WireError, EspNowV2Payload, EspNowV2WireError,
};

use oer_ieee80211_softmac::EspNowPeerId;

/// One fully owned plaintext ESP-NOW v1 application request.
///
/// Construction copies at most 250 bytes. Peer identity and the caller-owned
/// four-byte random value remain attached to that exact payload while it is
/// queued, transmitted, rejected or cancelled.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EspNowOwnedV1Tx {
    peer: EspNowPeerId,
    random_value: EspNowRandomValue,
    payload_length: u8,
    payload: [u8; ESP_NOW_V1_MAX_PAYLOAD_LEN],
}

impl EspNowOwnedV1Tx {
    pub fn try_new(
        peer: EspNowPeerId,
        random_value: EspNowRandomValue,
        payload: &[u8],
    ) -> Result<Self, EspNowV1WireError> {
        let payload = EspNowV1Payload::new(payload)?;
        let mut owned = [0; ESP_NOW_V1_MAX_PAYLOAD_LEN];
        owned[..payload.len()].copy_from_slice(payload.bytes());
        Ok(Self {
            peer,
            random_value,
            payload_length: payload.len() as u8,
            payload: owned,
        })
    }

    pub const fn peer(&self) -> EspNowPeerId {
        self.peer
    }

    pub const fn random_value(&self) -> EspNowRandomValue {
        self.random_value
    }

    pub fn payload(&self) -> &[u8] {
        &self.payload[..usize::from(self.payload_length)]
    }
}

/// Generation-fenced identity of one admitted request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EspNowTxTicket {
    epoch: u32,
    request: u32,
}

impl EspNowTxTicket {
    pub const fn epoch(self) -> u32 {
        self.epoch
    }

    pub const fn request(self) -> u32 {
        self.request
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct EspNowV2TxLease {
    slot: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
// Both variants remain inline so the bounded no-alloc mailbox can move a
// complete request by value without another lifetime or storage owner.
#[allow(clippy::large_enum_variant)]
pub(crate) enum EspNowQueuedRequest {
    V1(EspNowOwnedV1Tx),
    V2(EspNowV2TxLease),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct EspNowQueuedTx {
    pub(crate) ticket: EspNowTxTicket,
    pub(crate) peer: EspNowPeerId,
    pub(crate) request: EspNowQueuedRequest,
}

#[derive(Clone, Copy)]
struct EspNowV2TxHeader {
    ticket: EspNowTxTicket,
    peer: EspNowPeerId,
    random_value: EspNowRandomValue,
    payload_length: u16,
}

struct EspNowV2TxSlot {
    header: Option<EspNowV2TxHeader>,
    payload: [u8; ESP_NOW_V2_MAX_PAYLOAD_LEN],
}

impl EspNowV2TxSlot {
    const EMPTY: Self = Self {
        header: None,
        payload: [0; ESP_NOW_V2_MAX_PAYLOAD_LEN],
    };
}

/// Borrowed view of an application request while its preallocated slot is
/// synchronously copied into the ordinary TX arena.
pub struct EspNowV2TxRequest<'payload> {
    peer: EspNowPeerId,
    random_value: EspNowRandomValue,
    payload: &'payload [u8],
}

impl EspNowV2TxRequest<'_> {
    pub const fn peer(&self) -> EspNowPeerId {
        self.peer
    }

    pub const fn random_value(&self) -> EspNowRandomValue {
        self.random_value
    }

    pub const fn payload(&self) -> &[u8] {
        self.payload
    }
}

/// Why an admitted request ended without starting a new transmission.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspNowTxCancelReason {
    StationStopped,
    ConnectionEnded,
    OwnerShutdown,
    StaleEpoch,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspNowTxRuntimeFailure {
    MissingOrdinaryTxOutcome,
    MissingV2PayloadSlot,
    OffChannel(EspNowOffChannelFailureStage),
    TxLifecycle(SingleMpduTxError),
}

/// Durable terminal stage for an off-channel request whose detailed hardware
/// error remains with the quarantined standalone owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspNowOffChannelFailureStage {
    QuiesceHomeInterrupts,
    StopHomeReceive,
    SwitchToPeer,
    ActivateTransmitInterrupts,
    QuiesceTransmitInterrupts,
    SwitchHome,
    PrepareHomeReceive,
    StartHomeReceive,
    ActivateHomeInterrupts,
}

/// Exactly one terminal result for an admitted ticket.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspNowTxTerminal {
    /// The ordinary transaction reached a terminal hardware result. Inspect
    /// `is_success()` or the embedded normalized status to distinguish ACK,
    /// timeout, collision and hardware-failure outcomes.
    Completed(SingleMpduTxOutcome),
    /// Peer/wire admission or the typed chip PHY/publication boundary rejected
    /// the request before a live hardware transaction was created.
    Rejected(SingleMpduEspNowTxError),
    Cancelled(EspNowTxCancelReason),
    RuntimeFailure(EspNowTxRuntimeFailure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EspNowTxCompletion {
    pub ticket: EspNowTxTicket,
    pub peer: EspNowPeerId,
    pub terminal: EspNowTxTerminal,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspNowTxMailboxEpochError {
    ZeroCapacity,
    GenerationExhausted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspNowTxBackpressure {
    StaleEpoch,
    QueueFull,
    RequestIdExhausted,
}

/// Failed non-blocking admission, retaining the complete application request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EspNowTxTrySendError {
    pub reason: EspNowTxBackpressure,
    pub request: EspNowOwnedV1Tx,
}

/// Failed v2 admission. The caller's borrowed payload is never consumed;
/// bytes become mailbox-owned only after this call succeeds.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspNowV2TxTrySendError {
    Wire(EspNowV2WireError),
    Backpressure(EspNowTxBackpressure),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspNowTxMailboxInvariantError {
    CompletionQueueFull,
    PublisherInFlight,
    MissingV2PayloadSlot,
}

/// Static request/completion queues, v2 payload slots and reconnect generation.
///
/// Both channels have the same capacity. Admission reserves one completion
/// slot before publishing a request, so stop/reconnect can synchronously emit
/// a terminal cancellation for every accepted ticket without allocation or a
/// completion-overflow policy.
pub struct EspNowTxMailboxResources<M: RawMutex, const CAPACITY: usize> {
    requests: Channel<M, EspNowQueuedTx, CAPACITY>,
    completions: Channel<M, EspNowTxCompletion, CAPACITY>,
    generation: AtomicU32,
    next_request: AtomicU32,
    outstanding: AtomicU32,
    publishers: AtomicU32,
    v2_slots: BlockingMutex<M, RefCell<[EspNowV2TxSlot; CAPACITY]>>,
}

impl<M: RawMutex, const CAPACITY: usize> EspNowTxMailboxResources<M, CAPACITY> {
    pub const fn new() -> Self {
        Self {
            requests: Channel::new(),
            completions: Channel::new(),
            generation: AtomicU32::new(0),
            next_request: AtomicU32::new(0),
            outstanding: AtomicU32::new(0),
            publishers: AtomicU32::new(0),
            v2_slots: BlockingMutex::new(RefCell::new([const { EspNowV2TxSlot::EMPTY }; CAPACITY])),
        }
    }

    /// Start a fresh application/scheduler epoch. The unique mutable borrow
    /// prevents overlapping epochs in safe code; the generation additionally
    /// fences an endpoint retained across an unsafe/static lifecycle bug.
    pub fn begin_epoch(
        &mut self,
    ) -> Result<
        (
            EspNowTxHandle<'_, M, CAPACITY>,
            EspNowTxMailboxOwner<'_, M, CAPACITY>,
        ),
        EspNowTxMailboxEpochError,
    > {
        if CAPACITY == 0 {
            return Err(EspNowTxMailboxEpochError::ZeroCapacity);
        }
        let current = self.generation.load(Ordering::Acquire);
        if current >= u32::MAX - 1 {
            return Err(EspNowTxMailboxEpochError::GenerationExhausted);
        }

        let request_receiver = self.requests.receiver();
        let completion_receiver = self.completions.receiver();
        while request_receiver.try_receive().is_ok() {}
        while completion_receiver.try_receive().is_ok() {}
        for slot in self.v2_slots.get_mut().get_mut() {
            slot.header = None;
        }
        let epoch = current + 1;
        self.next_request.store(0, Ordering::Release);
        self.outstanding.store(0, Ordering::Release);
        self.publishers.store(0, Ordering::Release);
        self.generation.store(epoch, Ordering::Release);

        Ok((
            EspNowTxHandle {
                requests: self.requests.sender(),
                completions: completion_receiver,
                generation: &self.generation,
                next_request: &self.next_request,
                outstanding: &self.outstanding,
                publishers: &self.publishers,
                v2_slots: &self.v2_slots,
                epoch,
            },
            EspNowTxMailboxOwner {
                requests: request_receiver,
                completions: self.completions.sender(),
                generation: &self.generation,
                publishers: &self.publishers,
                v2_slots: &self.v2_slots,
                epoch,
                open: true,
            },
        ))
    }
}

impl<M: RawMutex, const CAPACITY: usize> Default for EspNowTxMailboxResources<M, CAPACITY> {
    fn default() -> Self {
        Self::new()
    }
}

/// Application capability for one connected ESP-NOW TX epoch.
///
/// Both `try_send` and `try_send_v2` are intentionally non-blocking: queue
/// saturation is visible as typed backpressure while an application drains
/// terminal completions.
pub struct EspNowTxHandle<'resources, M: RawMutex, const CAPACITY: usize> {
    requests: Sender<'resources, M, EspNowQueuedTx, CAPACITY>,
    completions: Receiver<'resources, M, EspNowTxCompletion, CAPACITY>,
    generation: &'resources AtomicU32,
    next_request: &'resources AtomicU32,
    outstanding: &'resources AtomicU32,
    publishers: &'resources AtomicU32,
    v2_slots: &'resources BlockingMutex<M, RefCell<[EspNowV2TxSlot; CAPACITY]>>,
    epoch: u32,
}

struct EspNowTxPublicationLease<'resources> {
    publishers: &'resources AtomicU32,
}

impl Drop for EspNowTxPublicationLease<'_> {
    fn drop(&mut self) {
        self.publishers.fetch_sub(1, Ordering::AcqRel);
    }
}

impl<M: RawMutex, const CAPACITY: usize> EspNowTxHandle<'_, M, CAPACITY> {
    pub const fn epoch(&self) -> u32 {
        self.epoch
    }

    pub fn try_send(
        &self,
        request: EspNowOwnedV1Tx,
    ) -> Result<EspNowTxTicket, EspNowTxTrySendError> {
        if self.generation.load(Ordering::Acquire) != self.epoch {
            return Err(EspNowTxTrySendError {
                reason: EspNowTxBackpressure::StaleEpoch,
                request,
            });
        }
        self.publishers.fetch_add(1, Ordering::AcqRel);
        let _publication = EspNowTxPublicationLease {
            publishers: self.publishers,
        };
        // Closing an epoch changes the generation before its final drain. A
        // sender which raced that edge either fails here or remains counted
        // until its synchronous channel publication has finished.
        if self.generation.load(Ordering::Acquire) != self.epoch {
            return Err(EspNowTxTrySendError {
                reason: EspNowTxBackpressure::StaleEpoch,
                request,
            });
        }
        let request_id = self
            .next_request
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                current.checked_add(1)
            })
            .map(|previous| previous + 1)
            .map_err(|_| EspNowTxTrySendError {
                reason: EspNowTxBackpressure::RequestIdExhausted,
                request,
            })?;
        if self
            .outstanding
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                (usize::try_from(current).unwrap_or(usize::MAX) < CAPACITY).then_some(current + 1)
            })
            .is_err()
        {
            return Err(EspNowTxTrySendError {
                reason: EspNowTxBackpressure::QueueFull,
                request,
            });
        }
        let ticket = EspNowTxTicket {
            epoch: self.epoch,
            request: request_id,
        };
        match self.requests.try_send(EspNowQueuedTx {
            ticket,
            peer: request.peer(),
            request: EspNowQueuedRequest::V1(request),
        }) {
            Ok(()) => Ok(ticket),
            Err(TrySendError::Full(queued)) => {
                self.outstanding.fetch_sub(1, Ordering::AcqRel);
                Err(EspNowTxTrySendError {
                    reason: EspNowTxBackpressure::QueueFull,
                    request: match queued.request {
                        EspNowQueuedRequest::V1(request) => request,
                        EspNowQueuedRequest::V2(_) => {
                            unreachable!("v1 admission cannot publish a v2 lease")
                        }
                    },
                })
            }
        }
    }

    /// Copy and admit one v2 request into a preallocated payload slot.
    ///
    /// The 1470-byte storage never enters an Embassy channel or an async
    /// future; only its generation-fenced slot lease is queued.
    pub fn try_send_v2(
        &self,
        peer: EspNowPeerId,
        random_value: EspNowRandomValue,
        payload: &[u8],
    ) -> Result<EspNowTxTicket, EspNowV2TxTrySendError> {
        let payload = EspNowV2Payload::new(payload).map_err(EspNowV2TxTrySendError::Wire)?;
        if self.generation.load(Ordering::Acquire) != self.epoch {
            return Err(EspNowV2TxTrySendError::Backpressure(
                EspNowTxBackpressure::StaleEpoch,
            ));
        }
        self.publishers.fetch_add(1, Ordering::AcqRel);
        let _publication = EspNowTxPublicationLease {
            publishers: self.publishers,
        };
        if self.generation.load(Ordering::Acquire) != self.epoch {
            return Err(EspNowV2TxTrySendError::Backpressure(
                EspNowTxBackpressure::StaleEpoch,
            ));
        }
        let request_id = self
            .next_request
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                current.checked_add(1)
            })
            .map(|previous| previous + 1)
            .map_err(|_| {
                EspNowV2TxTrySendError::Backpressure(EspNowTxBackpressure::RequestIdExhausted)
            })?;
        if self
            .outstanding
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                (usize::try_from(current).unwrap_or(usize::MAX) < CAPACITY).then_some(current + 1)
            })
            .is_err()
        {
            return Err(EspNowV2TxTrySendError::Backpressure(
                EspNowTxBackpressure::QueueFull,
            ));
        }
        let ticket = EspNowTxTicket {
            epoch: self.epoch,
            request: request_id,
        };
        let slot = self.v2_slots.lock(|slots| {
            let mut slots = slots.borrow_mut();
            let index = slots.iter().position(|slot| slot.header.is_none())?;
            slots[index].payload[..payload.len()].copy_from_slice(payload.bytes());
            slots[index].header = Some(EspNowV2TxHeader {
                ticket,
                peer,
                random_value,
                payload_length: payload.len() as u16,
            });
            Some(index)
        });
        let Some(slot) = slot else {
            self.outstanding.fetch_sub(1, Ordering::AcqRel);
            return Err(EspNowV2TxTrySendError::Backpressure(
                EspNowTxBackpressure::QueueFull,
            ));
        };
        let queued = EspNowQueuedTx {
            ticket,
            peer,
            request: EspNowQueuedRequest::V2(EspNowV2TxLease { slot }),
        };
        match self.requests.try_send(queued) {
            Ok(()) => Ok(ticket),
            Err(TrySendError::Full(queued)) => {
                self.v2_slots.lock(|slots| {
                    slots.borrow_mut()[slot].header = None;
                });
                self.outstanding.fetch_sub(1, Ordering::AcqRel);
                let _ = queued;
                Err(EspNowV2TxTrySendError::Backpressure(
                    EspNowTxBackpressure::QueueFull,
                ))
            }
        }
    }

    pub fn try_receive(&self) -> Option<EspNowTxCompletion> {
        loop {
            let completion = self.completions.try_receive().ok()?;
            if completion.ticket.epoch == self.epoch {
                self.outstanding.fetch_sub(1, Ordering::AcqRel);
                return Some(completion);
            }
        }
    }

    pub async fn receive(&self) -> EspNowTxCompletion {
        loop {
            let completion = self.completions.receive().await;
            if completion.ticket.epoch == self.epoch {
                self.outstanding.fetch_sub(1, Ordering::AcqRel);
                return completion;
            }
        }
    }

    pub fn outstanding(&self) -> u32 {
        self.outstanding.load(Ordering::Acquire)
    }

    pub fn completion_len(&self) -> usize {
        self.completions.len()
    }
}

/// Scheduler-side endpoint. It can consume accepted requests and publish
/// terminal results, but cannot create application tickets.
pub struct EspNowTxMailboxOwner<'resources, M: RawMutex, const CAPACITY: usize> {
    requests: Receiver<'resources, M, EspNowQueuedTx, CAPACITY>,
    completions: Sender<'resources, M, EspNowTxCompletion, CAPACITY>,
    generation: &'resources AtomicU32,
    publishers: &'resources AtomicU32,
    v2_slots: &'resources BlockingMutex<M, RefCell<[EspNowV2TxSlot; CAPACITY]>>,
    epoch: u32,
    open: bool,
}

impl<M: RawMutex, const CAPACITY: usize> EspNowTxMailboxOwner<'_, M, CAPACITY> {
    pub const fn epoch(&self) -> u32 {
        self.epoch
    }

    pub const fn is_open(&self) -> bool {
        self.open
    }

    pub fn has_pending(&self) -> bool {
        !self.requests.is_empty()
    }

    pub fn publishers_in_flight(&self) -> u32 {
        self.publishers.load(Ordering::Acquire)
    }

    pub async fn ready(&self) {
        self.requests.ready_to_receive().await;
    }

    pub(crate) fn try_take(&self) -> Option<EspNowQueuedTx> {
        self.requests.try_receive().ok()
    }

    pub(crate) fn with_v2_request<R>(
        &self,
        queued: &EspNowQueuedTx,
        use_request: impl FnOnce(EspNowV2TxRequest<'_>) -> R,
    ) -> Result<R, EspNowTxMailboxInvariantError> {
        let EspNowQueuedRequest::V2(lease) = queued.request else {
            return Err(EspNowTxMailboxInvariantError::MissingV2PayloadSlot);
        };
        self.v2_slots.lock(|slots| {
            let slots = slots.borrow();
            let Some(slot) = slots.get(lease.slot) else {
                return Err(EspNowTxMailboxInvariantError::MissingV2PayloadSlot);
            };
            let Some(header) = slot.header else {
                return Err(EspNowTxMailboxInvariantError::MissingV2PayloadSlot);
            };
            if header.ticket != queued.ticket || header.peer != queued.peer {
                return Err(EspNowTxMailboxInvariantError::MissingV2PayloadSlot);
            }
            Ok(use_request(EspNowV2TxRequest {
                peer: header.peer,
                random_value: header.random_value,
                payload: &slot.payload[..usize::from(header.payload_length)],
            }))
        })
    }

    fn release_v2_slot(&self, queued: &EspNowQueuedTx) {
        let EspNowQueuedRequest::V2(lease) = queued.request else {
            return;
        };
        self.v2_slots.lock(|slots| {
            let mut slots = slots.borrow_mut();
            if slots
                .get(lease.slot)
                .and_then(|slot| slot.header)
                .is_some_and(|header| header.ticket == queued.ticket)
            {
                slots[lease.slot].header = None;
            }
        });
    }

    pub(crate) fn close(&mut self) {
        if !self.open {
            return;
        }
        let _ = self.generation.compare_exchange(
            self.epoch,
            self.epoch + 1,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
        self.open = false;
    }

    pub(crate) fn publish(
        &self,
        queued: EspNowQueuedTx,
        terminal: EspNowTxTerminal,
    ) -> Result<(), EspNowTxMailboxInvariantError> {
        self.release_v2_slot(&queued);
        self.completions
            .try_send(EspNowTxCompletion {
                ticket: queued.ticket,
                peer: queued.peer,
                terminal,
            })
            .map_err(|TrySendError::Full(_)| EspNowTxMailboxInvariantError::CompletionQueueFull)
    }

    pub(crate) fn cancel_pending(
        &self,
        reason: EspNowTxCancelReason,
    ) -> Result<u32, EspNowTxMailboxInvariantError> {
        let mut cancelled = 0_u32;
        while let Some(queued) = self.try_take() {
            let reason = if queued.ticket.epoch == self.epoch {
                reason
            } else {
                EspNowTxCancelReason::StaleEpoch
            };
            self.publish(queued, EspNowTxTerminal::Cancelled(reason))?;
            cancelled = cancelled.saturating_add(1);
        }
        Ok(cancelled)
    }

    pub(crate) fn shutdown(
        mut self,
        reason: EspNowTxCancelReason,
    ) -> Result<EspNowTxMailboxShutdown, EspNowTxMailboxInvariantError> {
        self.close();
        if self.publishers_in_flight() != 0 {
            return Err(EspNowTxMailboxInvariantError::PublisherInFlight);
        }
        Ok(EspNowTxMailboxShutdown {
            epoch: self.epoch,
            cancelled: self.cancel_pending(reason)?,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EspNowTxMailboxShutdown {
    pub epoch: u32,
    pub cancelled: u32,
}
