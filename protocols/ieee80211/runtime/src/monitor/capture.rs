//! Bounded ownership handoff from monitor RX to an async capture consumer.
//!
//! The radio path copies a borrowed monitor view into independent caller-owned
//! storage and performs only a non-blocking channel publication. It therefore
//! never retains a DMA descriptor and never waits for a slow sniffer. The
//! capture pool is ordinary CPU memory and may be placed in external RAM when
//! the selected chip can access that memory; the Embassy channel stores only
//! leases and metadata, not frame-sized arrays.

use core::future::Future;

use embassy_sync::{
    blocking_mutex::raw::RawMutex,
    channel::{Channel, Receiver, Sender, TrySendError},
};
use oer_ieee80211_softmac::{
    MacRxMetadata, MonitorDropReason, MonitorFrame, MonitorInjectionChannelBinding,
    MonitorPublishOutcome, MonitorSink, WifiChannel,
    interface::{ChannelContextId, MonitorTapPoint},
};
use oer_memory::{RxHandoffPool, RxNetworkLease};

/// Metadata copied alongside one independently retained monitor frame.
#[derive(Clone, Copy, Debug)]
pub struct MonitorCaptureMetadata<Rate> {
    /// Supervisor generation which produced this frame.
    pub generation: u32,
    /// Exact tuned channel at the start of this completed RX/IRQ epoch.
    ///
    /// `None` is reserved for legacy sinks which publish without first
    /// accepting a channel-epoch edge. Production monitor tasks always stamp
    /// `Some` before starting RX.
    pub channel: Option<WifiChannel>,
    /// Monotonic channel-binding epoch within `generation`. A value may be
    /// skipped when a stopped retune supersedes a prepared channel before RX
    /// starts; retained frames never change their stamped value.
    pub channel_epoch: u32,
    /// False after the finite channel-epoch identity space is exhausted.
    /// Capture remains usable, but this metadata can no longer authorize TX.
    injection_binding_valid: bool,
    pub tap: MonitorTapPoint,
    pub channel_context: ChannelContextId,
    pub rx: MacRxMetadata<Rate>,
    /// Complete logical MPDU length reported by hardware.
    ///
    /// This can exceed `captured_length()` when hardware stripped an
    /// authenticated trailer before exposing the normalized DMA view.
    pub logical_length: usize,
}

/// Unique consumer ownership of one copied monitor frame.
///
/// Dropping this value returns its capture slot to the radio publisher.
pub struct MonitorCaptureFrame<'pool, Rate, const CAPACITY: usize> {
    lease: RxNetworkLease<'pool, CAPACITY>,
    metadata: MonitorCaptureMetadata<Rate>,
}

impl<Rate, const CAPACITY: usize> MonitorCaptureFrame<'_, Rate, CAPACITY> {
    pub fn bytes(&self) -> &[u8] {
        self.lease.frame()
    }

    pub const fn metadata(&self) -> &MonitorCaptureMetadata<Rate> {
        &self.metadata
    }

    pub fn captured_length(&self) -> usize {
        self.bytes().len()
    }

    pub fn is_complete(&self) -> bool {
        self.metadata.logical_length == self.captured_length()
    }

    /// Exact monitor dwell which produced this retained capture.
    ///
    /// A legacy sink which never accepted a channel-epoch edge returns
    /// `None` and therefore cannot authorize an injection.
    pub fn injection_channel_binding(&self) -> Option<MonitorInjectionChannelBinding> {
        if !self.metadata.injection_binding_valid {
            return None;
        }
        self.metadata.channel.and_then(|channel| {
            MonitorInjectionChannelBinding::new(
                self.metadata.generation,
                self.metadata.channel_epoch,
                channel,
            )
            .ok()
        })
    }
}

/// Caller-owned frame storage independent from the Wi-Fi DMA ring.
pub struct MonitorCapturePool<const CAPACITY: usize, const SLOTS: usize> {
    storage: RxHandoffPool<CAPACITY, SLOTS>,
}

impl<const CAPACITY: usize, const SLOTS: usize> MonitorCapturePool<CAPACITY, SLOTS> {
    pub const fn new() -> Self {
        const {
            #[cfg_attr(not(test), expect(clippy::panic, reason = "evaluated at build"))]
            const fn checked<const CAPACITY: usize, const SLOTS: usize>() {
                if CAPACITY == 0 {
                    panic!("monitor capture slots must not be empty");
                }
                if SLOTS == 0 {
                    panic!("monitor capture pool must contain a slot");
                }
                if SLOTS > u8::MAX as usize + 1 {
                    panic!("monitor capture pool exceeds the lease index domain");
                }
            }
            checked::<CAPACITY, SLOTS>();
        };
        Self {
            storage: RxHandoffPool::new(),
        }
    }

    /// Payload bytes reserved by this pool, excluding ownership bookkeeping.
    pub const fn payload_storage_bytes() -> usize {
        const {
            #[cfg_attr(not(test), expect(clippy::panic, reason = "evaluated at build"))]
            const fn checked<const CAPACITY: usize, const SLOTS: usize>() -> usize {
                match CAPACITY.checked_mul(SLOTS) {
                    Some(bytes) => bytes,
                    None => panic!("capture payload storage fits the target address space"),
                }
            }
            checked::<CAPACITY, SLOTS>()
        }
    }

    pub const fn slot_capacity() -> usize {
        CAPACITY
    }

    pub const fn slot_count() -> usize {
        SLOTS
    }

    pub fn claimed_slots(&self) -> usize {
        self.storage.claimed_slots()
    }

    fn try_capture<'pool, Rate>(
        &'pool self,
        frame: MonitorFrame<'_, Rate>,
        generation: u32,
        channel: Option<WifiChannel>,
        channel_epoch: u32,
        injection_binding_valid: bool,
        snapshot_length: Option<usize>,
    ) -> Result<MonitorCaptureFrame<'pool, Rate, CAPACITY>, MonitorDropReason> {
        let captured_length = snapshot_length
            .unwrap_or(frame.bytes.len())
            .min(frame.bytes.len());
        if captured_length > CAPACITY {
            return Err(MonitorDropReason::TooLong);
        }
        let mut radio = self
            .storage
            .try_claim_radio()
            .ok_or(MonitorDropReason::Full)?;
        radio
            .frame_prefix_mut(captured_length)
            .iter_mut()
            .zip(frame.bytes)
            .for_each(|(to, from)| *to = *from);
        Ok(MonitorCaptureFrame {
            lease: radio.into_network(captured_length),
            metadata: MonitorCaptureMetadata {
                generation,
                channel,
                channel_epoch,
                injection_binding_valid,
                tap: frame.tap,
                channel_context: frame.channel_context,
                rx: frame.metadata,
                logical_length: frame.logical_length,
            },
        })
    }
}

impl<const CAPACITY: usize, const SLOTS: usize> Default for MonitorCapturePool<CAPACITY, SLOTS> {
    fn default() -> Self {
        Self::new()
    }
}

/// Embassy queue and capture-pool binding for one monitor epoch.
///
/// `DEPTH` is a retained-frame limit, never a processing batch size. It may
/// not exceed the number of independently owned capture slots.
pub struct MonitorCaptureResources<
    'pool,
    M: RawMutex,
    Rate,
    const DEPTH: usize,
    const CAPACITY: usize,
    const SLOTS: usize,
> {
    pool: &'pool MonitorCapturePool<CAPACITY, SLOTS>,
    frames: Channel<M, MonitorCaptureFrame<'pool, Rate, CAPACITY>, DEPTH>,
}

impl<'pool, M: RawMutex, Rate, const DEPTH: usize, const CAPACITY: usize, const SLOTS: usize>
    MonitorCaptureResources<'pool, M, Rate, DEPTH, CAPACITY, SLOTS>
{
    pub const fn new(pool: &'pool MonitorCapturePool<CAPACITY, SLOTS>) -> Self {
        const {
            #[cfg_attr(not(test), expect(clippy::panic, reason = "evaluated at build"))]
            const fn checked<const DEPTH: usize, const SLOTS: usize>() {
                if DEPTH == 0 {
                    panic!("monitor capture queue must not be empty");
                }
                if DEPTH > SLOTS {
                    panic!("monitor capture queue cannot outgrow its ownership pool");
                }
            }
            checked::<DEPTH, SLOTS>();
        };
        Self {
            pool,
            frames: Channel::new(),
        }
    }

    /// Split one monitor epoch into non-blocking radio and async consumer
    /// capabilities. Endpoints can be reconstructed only after the previous
    /// epoch has stopped and its retained frames have been dropped.
    pub fn split(
        &self,
    ) -> (
        MonitorCaptureSink<'_, 'pool, M, Rate, DEPTH, CAPACITY, SLOTS>,
        MonitorCaptureReceiver<'_, 'pool, M, Rate, DEPTH, CAPACITY>,
    ) {
        (
            MonitorCaptureSink {
                pool: self.pool,
                sender: self.frames.sender(),
                generation: 0,
                channel: None,
                channel_epoch: 0,
                injection_binding_valid: true,
                snapshot_length: None,
            },
            MonitorCaptureReceiver {
                receiver: self.frames.receiver(),
            },
        )
    }

    /// Drop captures which were queued by a completed monitor generation.
    pub fn discard_queued(&self) -> usize {
        let mut discarded = 0_usize;
        while let Ok(frame) = self.frames.try_receive() {
            drop(frame);
            discarded = discarded.saturating_add(1);
        }
        discarded
    }
}

/// Radio-side best-effort capture endpoint.
pub struct MonitorCaptureSink<
    'queue,
    'pool,
    M: RawMutex,
    Rate,
    const DEPTH: usize,
    const CAPACITY: usize,
    const SLOTS: usize,
> {
    pool: &'pool MonitorCapturePool<CAPACITY, SLOTS>,
    sender: Sender<'queue, M, MonitorCaptureFrame<'pool, Rate, CAPACITY>, DEPTH>,
    generation: u32,
    channel: Option<WifiChannel>,
    channel_epoch: u32,
    injection_binding_valid: bool,
    snapshot_length: Option<usize>,
}

impl<M: RawMutex, Rate, const DEPTH: usize, const CAPACITY: usize, const SLOTS: usize>
    MonitorCaptureSink<'_, '_, M, Rate, DEPTH, CAPACITY, SLOTS>
{
    /// Bind a reusable sink to one supervisor generation and capture policy.
    pub fn configure(&mut self, generation: u32, snapshot_length: Option<usize>) {
        self.generation = generation;
        self.channel = None;
        self.channel_epoch = 0;
        self.injection_binding_valid = true;
        self.snapshot_length = snapshot_length;
    }
}

impl<M: RawMutex, Rate, const DEPTH: usize, const CAPACITY: usize, const SLOTS: usize>
    MonitorSink<Rate> for MonitorCaptureSink<'_, '_, M, Rate, DEPTH, CAPACITY, SLOTS>
{
    fn begin_channel_epoch(&mut self, channel: WifiChannel) {
        self.channel = Some(channel);
        if self.injection_binding_valid {
            match self.channel_epoch.checked_add(1) {
                Some(epoch) => self.channel_epoch = epoch,
                None => self.injection_binding_valid = false,
            }
        }
    }

    fn end_channel_epoch(&mut self) {
        self.channel = None;
    }

    fn injection_channel_binding(&self) -> Option<MonitorInjectionChannelBinding> {
        if !self.injection_binding_valid {
            return None;
        }
        self.channel.and_then(|channel| {
            MonitorInjectionChannelBinding::new(self.generation, self.channel_epoch, channel).ok()
        })
    }

    fn try_publish(&mut self, frame: MonitorFrame<'_, Rate>) -> MonitorPublishOutcome {
        // Avoid an otherwise pointless frame-sized copy after a slow consumer
        // has already filled the queue. A concurrent receive may make this a
        // conservative best-effort drop, which is valid for a monitor tap.
        if self.sender.is_full() {
            return MonitorPublishOutcome::Dropped(MonitorDropReason::Full);
        }
        let captured = match self.pool.try_capture(
            frame,
            self.generation,
            self.channel,
            self.channel_epoch,
            self.injection_binding_valid,
            self.snapshot_length,
        ) {
            Ok(captured) => captured,
            Err(reason) => return MonitorPublishOutcome::Dropped(reason),
        };
        match self.sender.try_send(captured) {
            Ok(()) => MonitorPublishOutcome::Published,
            Err(TrySendError::Full(_captured)) => {
                // Dropping the returned frame restores the pool credit.
                MonitorPublishOutcome::Dropped(MonitorDropReason::Full)
            }
        }
    }
}

/// Consumer-side async capture endpoint.
pub struct MonitorCaptureReceiver<
    'queue,
    'pool,
    M: RawMutex,
    Rate,
    const DEPTH: usize,
    const CAPACITY: usize,
> {
    receiver: Receiver<'queue, M, MonitorCaptureFrame<'pool, Rate, CAPACITY>, DEPTH>,
}

impl<'queue, 'pool, M: RawMutex, Rate, const DEPTH: usize, const CAPACITY: usize>
    MonitorCaptureReceiver<'queue, 'pool, M, Rate, DEPTH, CAPACITY>
{
    pub fn try_receive(&self) -> Option<MonitorCaptureFrame<'pool, Rate, CAPACITY>> {
        self.receiver.try_receive().ok()
    }

    pub fn receive(&self) -> impl Future<Output = MonitorCaptureFrame<'pool, Rate, CAPACITY>> + '_ {
        self.receiver.receive()
    }

    pub fn len(&self) -> usize {
        self.receiver.len()
    }

    pub fn is_empty(&self) -> bool {
        self.receiver.is_empty()
    }

    /// Drop every queued capture and return its pool credits.
    ///
    /// A monitor lifecycle calls this after stopping the RX owner and before
    /// reusing the same resources for another epoch.
    pub fn discard_queued(&self) -> usize {
        let mut discarded = 0_usize;
        while let Ok(frame) = self.receiver.try_receive() {
            drop(frame);
            discarded = discarded.saturating_add(1);
        }
        discarded
    }
}

#[cfg(test)]
mod tests;
