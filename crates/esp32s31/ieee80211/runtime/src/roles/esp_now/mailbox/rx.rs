//! Owned, bounded ESP-NOW receive handoff for connected and standalone epochs.

use core::{
    cell::RefCell,
    sync::atomic::{AtomicU32, Ordering},
};
use oer_ieee80211_mac::sequence::SequenceNumber;

use embassy_sync::{
    blocking_mutex::{Mutex as BlockingMutex, raw::RawMutex},
    channel::{Channel, Receiver, Sender, TrySendError},
};

use oer_esp32s31_ieee80211_mac::rx::RxPhyInfo;

use oer_esp32s31_ieee80211_sta::standalone_esp_now_rx::{
    StandaloneEspNowRxEvent, StandaloneEspNowRxSink, StandaloneEspNowV2RxEvent,
};

use oer_ieee80211_mac::extensions::espressif::esp_now::{
    ESP_NOW_V2_MAX_PAYLOAD_LEN, EspNowDestination, EspNowRandomValue, EspNowUnicastAddress,
};

use oer_ieee80211_softmac::{EspNowOwnedReceivedV1, EspNowPeerId, EspNowReceivedV2, MacRxMetadata};

/// Owned datagram delivered after strict protocol admission and duplicate
/// suppression. `epoch` lets an application reject a handle retained beyond
/// its connected lifecycle even though old publishers are also fail-closed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EspNowOwnedRxEvent {
    pub epoch: u32,
    pub received: EspNowOwnedReceivedV1,
    pub metadata: MacRxMetadata<RxPhyInfo>,
}

/// Small metadata result paired with bytes copied into the caller's v2
/// buffer. The 1470-byte payload itself never crosses an Embassy channel.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EspNowV2RxEvent {
    pub epoch: u32,
    pub peer: EspNowPeerId,
    pub destination: EspNowDestination,
    pub source: EspNowUnicastAddress,
    pub random_value: EspNowRandomValue,
    pub sequence_number: SequenceNumber,
    pub retry: bool,
    pub payload_length: usize,
    pub metadata: MacRxMetadata<RxPhyInfo>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct EspNowV2RxLease {
    epoch: u32,
    slot: usize,
}

#[derive(Clone, Copy)]
struct EspNowV2RxHeader {
    event: EspNowV2RxEvent,
}

struct EspNowV2RxSlot {
    header: Option<EspNowV2RxHeader>,
    payload: [u8; ESP_NOW_V2_MAX_PAYLOAD_LEN],
}

impl EspNowV2RxSlot {
    const EMPTY: Self = Self {
        header: None,
        payload: [0; ESP_NOW_V2_MAX_PAYLOAD_LEN],
    };
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspNowV2RxMailboxError {
    BufferTooSmall { required: usize, available: usize },
    MissingPayloadSlot,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspNowRxMailboxEpochError {
    GenerationExhausted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspNowRxPublishOutcome {
    Published,
    Full,
    StaleEpoch,
}

/// Static channel and lifecycle counters. It contains no WPA2 key material and
/// shares neither storage nor capacity with the connected control mailbox.
pub struct EspNowRxMailboxResources<M: RawMutex, const CAPACITY: usize> {
    channel: Channel<M, EspNowOwnedRxEvent, CAPACITY>,
    v2_channel: Channel<M, EspNowV2RxLease, CAPACITY>,
    v2_slots: BlockingMutex<M, RefCell<[EspNowV2RxSlot; CAPACITY]>>,
    generation: AtomicU32,
    dropped: AtomicU32,
    stale_publications: AtomicU32,
}

impl<M: RawMutex, const CAPACITY: usize> EspNowRxMailboxResources<M, CAPACITY> {
    pub const fn new() -> Self {
        Self {
            channel: Channel::new(),
            v2_channel: Channel::new(),
            v2_slots: BlockingMutex::new(RefCell::new([const { EspNowV2RxSlot::EMPTY }; CAPACITY])),
            generation: AtomicU32::new(0),
            dropped: AtomicU32::new(0),
            stale_publications: AtomicU32::new(0),
        }
    }

    /// Start one mailbox epoch after the previous publisher and receiver have
    /// been returned or dropped. The unique resource borrow keeps two epochs
    /// from being created concurrently in safe code.
    ///
    /// Pending datagrams are discarded before the new generation becomes
    /// visible. A publisher retained in violation of the ownership contract
    /// observes a generation mismatch and cannot enqueue into the new epoch.
    pub fn begin_epoch(
        &mut self,
    ) -> Result<
        (
            EspNowRxPublisher<'_, M, CAPACITY>,
            EspNowRxReceiver<'_, M, CAPACITY>,
        ),
        EspNowRxMailboxEpochError,
    > {
        let receiver = self.channel.receiver();
        let v2_receiver = self.v2_channel.receiver();
        while receiver.try_receive().is_ok() {}
        while v2_receiver.try_receive().is_ok() {}
        for slot in self.v2_slots.get_mut().get_mut() {
            slot.header = None;
        }
        let resources: &Self = self;
        let epoch = self
            .generation
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |generation| {
                generation.checked_add(1)
            })
            .map_err(|_| EspNowRxMailboxEpochError::GenerationExhausted)?
            + 1;
        resources.dropped.store(0, Ordering::Release);
        resources.stale_publications.store(0, Ordering::Release);
        Ok((
            EspNowRxPublisher {
                sender: resources.channel.sender(),
                v2_sender: resources.v2_channel.sender(),
                v2_slots: &resources.v2_slots,
                generation: &resources.generation,
                epoch,
                dropped: &resources.dropped,
                stale_publications: &resources.stale_publications,
            },
            EspNowRxReceiver {
                receiver,
                v2_receiver,
                v2_slots: &resources.v2_slots,
                epoch,
                dropped: &resources.dropped,
                stale_publications: &resources.stale_publications,
            },
        ))
    }
}

impl<M: RawMutex, const CAPACITY: usize> Default for EspNowRxMailboxResources<M, CAPACITY> {
    fn default() -> Self {
        Self::new()
    }
}

/// Callback-side capability. It may publish only into the generation in which
/// it was created and cannot consume application events.
#[derive(Clone, Copy)]
pub struct EspNowRxPublisher<'resources, M: RawMutex, const CAPACITY: usize> {
    sender: Sender<'resources, M, EspNowOwnedRxEvent, CAPACITY>,
    v2_sender: Sender<'resources, M, EspNowV2RxLease, CAPACITY>,
    v2_slots: &'resources BlockingMutex<M, RefCell<[EspNowV2RxSlot; CAPACITY]>>,
    generation: &'resources AtomicU32,
    epoch: u32,
    dropped: &'resources AtomicU32,
    stale_publications: &'resources AtomicU32,
}

impl<M: RawMutex, const CAPACITY: usize> EspNowRxPublisher<'_, M, CAPACITY> {
    pub const fn epoch(&self) -> u32 {
        self.epoch
    }

    pub fn try_publish(
        &self,
        received: oer_ieee80211_softmac::EspNowReceivedV1<'_>,
        metadata: MacRxMetadata<RxPhyInfo>,
    ) -> EspNowRxPublishOutcome {
        if self.generation.load(Ordering::Acquire) != self.epoch {
            saturating_increment(self.stale_publications);
            return EspNowRxPublishOutcome::StaleEpoch;
        }
        let event = EspNowOwnedRxEvent {
            epoch: self.epoch,
            received: EspNowOwnedReceivedV1::copy_from(received),
            metadata,
        };
        match self.sender.try_send(event) {
            Ok(()) => EspNowRxPublishOutcome::Published,
            Err(TrySendError::Full(_)) => {
                saturating_increment(self.dropped);
                EspNowRxPublishOutcome::Full
            }
        }
    }

    pub fn try_publish_v2(
        &self,
        received: EspNowReceivedV2<'_>,
        metadata: MacRxMetadata<RxPhyInfo>,
    ) -> EspNowRxPublishOutcome {
        if self.generation.load(Ordering::Acquire) != self.epoch {
            saturating_increment(self.stale_publications);
            return EspNowRxPublishOutcome::StaleEpoch;
        }
        let frame = received.frame();
        let slot = self.v2_slots.lock(|slots| {
            let mut slots = slots.borrow_mut();
            let index = slots.iter().position(|slot| slot.header.is_none())?;
            let payload_length = frame
                .action()
                .copy_payload(&mut slots[index].payload)
                .ok()?;
            slots[index].header = Some(EspNowV2RxHeader {
                event: EspNowV2RxEvent {
                    epoch: self.epoch,
                    peer: received.peer(),
                    destination: frame.destination(),
                    source: frame.source(),
                    random_value: frame.action().random_value(),
                    sequence_number: frame.sequence_number(),
                    retry: frame.retry(),
                    payload_length,
                    metadata,
                },
            });
            Some(index)
        });
        let Some(slot) = slot else {
            saturating_increment(self.dropped);
            return EspNowRxPublishOutcome::Full;
        };
        match self.v2_sender.try_send(EspNowV2RxLease {
            epoch: self.epoch,
            slot,
        }) {
            Ok(()) => EspNowRxPublishOutcome::Published,
            Err(TrySendError::Full(_)) => {
                self.v2_slots.lock(|slots| {
                    slots.borrow_mut()[slot].header = None;
                });
                saturating_increment(self.dropped);
                EspNowRxPublishOutcome::Full
            }
        }
    }
}

impl<M: RawMutex, const CAPACITY: usize> StandaloneEspNowRxSink
    for EspNowRxPublisher<'_, M, CAPACITY>
{
    fn publish(&mut self, event: StandaloneEspNowRxEvent<'_>) {
        let _ = self.try_publish(event.received, event.metadata);
    }

    fn supports_v2(&self) -> bool {
        true
    }

    fn publish_v2(&mut self, event: StandaloneEspNowV2RxEvent<'_>) {
        let _ = self.try_publish_v2(event.received, event.metadata);
    }
}

/// Application-side capability for one connected epoch.
pub struct EspNowRxReceiver<'resources, M: RawMutex, const CAPACITY: usize> {
    receiver: Receiver<'resources, M, EspNowOwnedRxEvent, CAPACITY>,
    v2_receiver: Receiver<'resources, M, EspNowV2RxLease, CAPACITY>,
    v2_slots: &'resources BlockingMutex<M, RefCell<[EspNowV2RxSlot; CAPACITY]>>,
    epoch: u32,
    dropped: &'resources AtomicU32,
    stale_publications: &'resources AtomicU32,
}

impl<M: RawMutex, const CAPACITY: usize> EspNowRxReceiver<'_, M, CAPACITY> {
    pub const fn epoch(&self) -> u32 {
        self.epoch
    }

    pub fn try_receive(&self) -> Option<EspNowOwnedRxEvent> {
        loop {
            let event = self.receiver.try_receive().ok()?;
            if event.epoch == self.epoch {
                return Some(event);
            }
        }
    }

    pub async fn receive(&self) -> EspNowOwnedRxEvent {
        loop {
            let event = self.receiver.receive().await;
            if event.epoch == self.epoch {
                return event;
            }
        }
    }

    pub fn try_receive_v2(
        &self,
        output: &mut [u8],
    ) -> Result<Option<EspNowV2RxEvent>, EspNowV2RxMailboxError> {
        self.require_v2_output(output)?;
        loop {
            let Some(lease) = self.v2_receiver.try_receive().ok() else {
                return Ok(None);
            };
            if lease.epoch == self.epoch {
                return self.copy_v2(lease, output).map(Some);
            }
            self.release_v2(lease);
        }
    }

    pub async fn receive_v2(
        &self,
        output: &mut [u8],
    ) -> Result<EspNowV2RxEvent, EspNowV2RxMailboxError> {
        self.require_v2_output(output)?;
        loop {
            let lease = self.v2_receiver.receive().await;
            if lease.epoch == self.epoch {
                return self.copy_v2(lease, output);
            }
            self.release_v2(lease);
        }
    }

    fn require_v2_output(&self, output: &[u8]) -> Result<(), EspNowV2RxMailboxError> {
        if output.len() < ESP_NOW_V2_MAX_PAYLOAD_LEN {
            return Err(EspNowV2RxMailboxError::BufferTooSmall {
                required: ESP_NOW_V2_MAX_PAYLOAD_LEN,
                available: output.len(),
            });
        }
        Ok(())
    }

    fn copy_v2(
        &self,
        lease: EspNowV2RxLease,
        output: &mut [u8],
    ) -> Result<EspNowV2RxEvent, EspNowV2RxMailboxError> {
        self.v2_slots.lock(|slots| {
            let mut slots = slots.borrow_mut();
            let Some(slot) = slots.get_mut(lease.slot) else {
                return Err(EspNowV2RxMailboxError::MissingPayloadSlot);
            };
            let Some(header) = slot.header.take() else {
                return Err(EspNowV2RxMailboxError::MissingPayloadSlot);
            };
            if header.event.epoch != lease.epoch {
                return Err(EspNowV2RxMailboxError::MissingPayloadSlot);
            }
            output[..header.event.payload_length]
                .copy_from_slice(&slot.payload[..header.event.payload_length]);
            Ok(header.event)
        })
    }

    fn release_v2(&self, lease: EspNowV2RxLease) {
        self.v2_slots.lock(|slots| {
            if let Some(slot) = slots.borrow_mut().get_mut(lease.slot)
                && slot
                    .header
                    .is_some_and(|header| header.event.epoch == lease.epoch)
            {
                slot.header = None;
            }
        });
    }

    pub fn len(&self) -> usize {
        self.receiver.len() + self.v2_receiver.len()
    }

    pub fn is_empty(&self) -> bool {
        self.receiver.is_empty() && self.v2_receiver.is_empty()
    }

    pub fn dropped(&self) -> u32 {
        self.dropped.load(Ordering::Acquire)
    }

    pub fn stale_publications(&self) -> u32 {
        self.stale_publications.load(Ordering::Acquire)
    }

    /// Drain every application event before the lifecycle reuses the static
    /// channel for another association.
    pub fn shutdown(self) -> EspNowRxMailboxShutdown {
        let mut discarded = 0_u32;
        while self.receiver.try_receive().is_ok() {
            discarded = discarded.saturating_add(1);
        }
        while let Ok(lease) = self.v2_receiver.try_receive() {
            self.release_v2(lease);
            discarded = discarded.saturating_add(1);
        }
        EspNowRxMailboxShutdown {
            epoch: self.epoch,
            discarded,
            dropped: self.dropped(),
            stale_publications: self.stale_publications(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EspNowRxMailboxShutdown {
    pub epoch: u32,
    pub discarded: u32,
    pub dropped: u32,
    pub stale_publications: u32,
}

fn saturating_increment(value: &AtomicU32) {
    let _ = value.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
        Some(current.saturating_add(1))
    });
}
