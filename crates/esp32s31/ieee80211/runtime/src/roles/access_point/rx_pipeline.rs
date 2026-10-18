#![expect(
    clippy::result_large_err,
    reason = "no-alloc AP RX shutdown returns the complete staged DMA owner"
)]

use core::future::Future;

use embassy_sync::blocking_mutex::raw::RawMutex;

use oer_esp32s31_ieee80211_mac::rx::{
    PUBLIC_HEADER_SIZE, RxDescriptorSnapshot, RxDma, RxRingHalted, RxRingLive, RxSegment,
    pool::{RxStagePool, RxStageTransactionError},
};

use oer_ieee80211_mac::security::LinkProtection;

#[cfg(any(feature = "diagnostics", test))]
use crate::diagnostics::rx_pipeline::RxPipelineObserver;

use crate::datapath::{
    DatapathRxProgress, DatapathRxWorkCounters,
    rx::{
        dma::{ReceiveDmaStorage, StagedRxEpoch},
        frontier::RxFrontierSchedulerSnapshot,
        staging::{StagedRxFrame, StagedRxQueue, StagedRxReceiver},
    },
};

#[doc(hidden)]
pub trait AccessPointStagedRxFrame {
    fn segment(&self) -> RxSegment<'_>;
}

impl<const CAPACITY: usize, const SLOTS: usize> AccessPointStagedRxFrame
    for StagedRxFrame<'_, CAPACITY, SLOTS>
{
    fn segment(&self) -> RxSegment<'_> {
        StagedRxFrame::segment(self)
    }
}

#[doc(hidden)]
pub trait AccessPointRxProtocolConsumer {
    /// Maximum number of descriptor-backed frame owners this consumer can
    /// retain independently of the physical DMA ring.
    const MAXIMUM_RETAINED_FRAMES: usize;

    fn try_receive(&mut self) -> Option<Self::Frame>;
    /// Consume frames which cannot require the ordinary-TX capability while
    /// the radio owner has an active transaction. Management and WPA2
    /// unprotected-data/EAPOL frames remain the exact ordered head; control,
    /// extension and ordinary protected data may be processed immediately.
    fn try_receive_during_tx(&mut self, security: LinkProtection) -> Option<Self::Frame>;
    type Frame: AccessPointStagedRxFrame;
    fn queued_frames(&self) -> usize;
    fn discard_queued(&mut self) -> usize;
}

#[doc(hidden)]
pub trait AccessPointRxProducerObservation<const COUNT: usize> {
    fn serviced_descriptors(&self) -> u64;
    /// Monotonic physical work completed by the AP DMA producer.
    ///
    /// The common DATAPATH continuation policy computes a before/after delta
    /// from these counters. Returning the role-trait default here would make
    /// every AP recycled-append continuation look empty and permanently pin
    /// adaptive coalescing to its 64-us bootstrap delay.
    fn work_counters(&self) -> DatapathRxWorkCounters;
    fn descriptor_snapshot(&self, index: usize) -> Option<RxDescriptorSnapshot>;
    fn scheduler_snapshot(&self) -> Option<RxFrontierSchedulerSnapshot>;
}

#[doc(hidden)]
pub trait AccessPointRxProducer<H, const COUNT: usize>:
    AccessPointRxProducerObservation<COUNT>
{
    fn start(
        &mut self,
        hardware: &mut H,
    ) -> impl Future<Output = Result<(), RxStageTransactionError>>;
    fn stage_completed(
        &mut self,
        hardware: &mut H,
    ) -> impl Future<Output = Result<DatapathRxProgress, RxStageTransactionError>>;
    fn stop(&mut self, hardware: &mut H) -> Result<(), RxStageTransactionError>;
}

/// DMA-only AP endpoint. It can publish staged owners but cannot parse or
/// consume them.
pub struct AccessPointReceiveProducer<
    'storage,
    'pool,
    'queue,
    D,
    M: RawMutex,
    const QUEUE_DEPTH: usize,
    const COUNT: usize,
    const STAGE_CAPACITY: usize,
    const STAGE_SLOTS: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> {
    inner: StagedRxEpoch<
        'storage,
        'pool,
        'queue,
        D,
        M,
        QUEUE_DEPTH,
        COUNT,
        STAGE_CAPACITY,
        STAGE_SLOTS,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
    >,
}

/// Protocol-only AP endpoint. It owns staged frame leases but has no DMA,
/// descriptor, PAC, or interrupt capability.
pub struct AccessPointRxConsumer<
    'pool,
    'queue,
    M: RawMutex,
    const QUEUE_DEPTH: usize,
    const STAGE_CAPACITY: usize,
    const STAGE_SLOTS: usize,
> {
    frames: StagedRxReceiver<'queue, 'pool, M, QUEUE_DEPTH, STAGE_CAPACITY, STAGE_SLOTS>,
    deferred: Option<StagedRxFrame<'pool, STAGE_CAPACITY, STAGE_SLOTS>>,
}

pub(super) fn can_process_ap_frame_during_tx(
    segment: RxSegment<'_>,
    security: LinkProtection,
) -> bool {
    let Some(frame_control) = segment
        .buffer
        .get(PUBLIC_HEADER_SIZE..PUBLIC_HEADER_SIZE + 2)
        .map(|bytes| u16::from_le_bytes([bytes[0], bytes[1]]))
    else {
        return false;
    };
    match frame_control & 0x000c {
        // Management processing may publish an authentication, association,
        // BlockAck-action or teardown response.
        0x0000 => false,
        // Protected data is ordinary authorized ingress. Open-network data is
        // also ordinary ingress; WPA2 unprotected data may be EAPOL and keeps
        // the hardware/TX capability until the idle boundary.
        0x0008 => security == LinkProtection::Open || frame_control & 0x4000 != 0,
        // Control and extension frames are observation/ignore-only in the AP
        // protocol owner and cannot manufacture a TX transaction.
        _ => true,
    }
}

impl<
    'storage,
    'pool,
    'queue,
    D,
    M: RawMutex,
    const QUEUE_DEPTH: usize,
    const COUNT: usize,
    const STAGE_CAPACITY: usize,
    const STAGE_SLOTS: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
>
    AccessPointReceiveProducer<
        'storage,
        'pool,
        'queue,
        D,
        M,
        QUEUE_DEPTH,
        COUNT,
        STAGE_CAPACITY,
        STAGE_SLOTS,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
    >
where
    D: oer_time::Timer,
{
    pub fn from_halted(
        ring: RxRingHalted<'storage, COUNT>,
        storage: &'static ReceiveDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
        pool: &'pool RxStagePool<STAGE_SLOTS, STAGE_CAPACITY>,
        queue: &'queue StagedRxQueue<'pool, M, QUEUE_DEPTH, STAGE_CAPACITY, STAGE_SLOTS>,
        delay: D,
    ) -> (
        Self,
        AccessPointRxConsumer<'pool, 'queue, M, QUEUE_DEPTH, STAGE_CAPACITY, STAGE_SLOTS>,
    ) {
        let (sender, frames) = queue.split();
        (
            Self {
                inner: StagedRxEpoch::from_halted(ring, storage, pool, sender, delay),
            },
            AccessPointRxConsumer {
                frames,
                deferred: None,
            },
        )
    }

    pub fn from_live(
        ring: RxRingLive<'storage, COUNT>,
        storage: &'static ReceiveDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
        pool: &'pool RxStagePool<STAGE_SLOTS, STAGE_CAPACITY>,
        queue: &'queue StagedRxQueue<'pool, M, QUEUE_DEPTH, STAGE_CAPACITY, STAGE_SLOTS>,
        delay: D,
    ) -> (
        Self,
        AccessPointRxConsumer<'pool, 'queue, M, QUEUE_DEPTH, STAGE_CAPACITY, STAGE_SLOTS>,
    ) {
        let (sender, frames) = queue.split();
        (
            Self {
                inner: StagedRxEpoch::from_live(ring, storage, pool, sender, delay),
            },
            AccessPointRxConsumer {
                frames,
                deferred: None,
            },
        )
    }

    /// Adopt the standalone producer retained by a previous logical role and
    /// resume only its protocol consumer for this AP epoch.
    pub fn from_live_resources(
        ring: RxRingLive<'storage, COUNT>,
        resources: crate::datapath::rx::dma::RxEpochResources<
            'storage,
            'pool,
            'queue,
            D,
            M,
            QUEUE_DEPTH,
            COUNT,
            STAGE_CAPACITY,
            STAGE_SLOTS,
            DMA_BUFFER_SIZE,
            DMA_STORAGE_SIZE,
        >,
    ) -> (
        Self,
        AccessPointRxConsumer<'pool, 'queue, M, QUEUE_DEPTH, STAGE_CAPACITY, STAGE_SLOTS>,
    ) {
        let frames = resources
            .try_resume_standalone_receiver()
            .expect("AP role handoff requires a retained standalone RX publisher");
        (
            Self {
                inner: StagedRxEpoch::from_live_resources(ring, resources),
            },
            AccessPointRxConsumer {
                frames,
                deferred: None,
            },
        )
    }

    /// Adopt a retained standalone producer while its physical ring is
    /// halted. Starting the AP epoch promotes this same owner to live.
    pub fn from_halted_resources(
        ring: RxRingHalted<'storage, COUNT>,
        resources: crate::datapath::rx::dma::RxEpochResources<
            'storage,
            'pool,
            'queue,
            D,
            M,
            QUEUE_DEPTH,
            COUNT,
            STAGE_CAPACITY,
            STAGE_SLOTS,
            DMA_BUFFER_SIZE,
            DMA_STORAGE_SIZE,
        >,
    ) -> (
        Self,
        AccessPointRxConsumer<'pool, 'queue, M, QUEUE_DEPTH, STAGE_CAPACITY, STAGE_SLOTS>,
    ) {
        let frames = resources
            .try_resume_standalone_receiver()
            .expect("AP role handoff requires a retained standalone RX publisher");
        (
            Self {
                inner: StagedRxEpoch::from_halted_resources(ring, resources),
            },
            AccessPointRxConsumer {
                frames,
                deferred: None,
            },
        )
    }

    #[cfg(any(feature = "diagnostics", test))]
    pub fn from_live_with_pipeline_observer(
        ring: RxRingLive<'storage, COUNT>,
        storage: &'static ReceiveDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
        pool: &'pool RxStagePool<STAGE_SLOTS, STAGE_CAPACITY>,
        queue: &'queue StagedRxQueue<'pool, M, QUEUE_DEPTH, STAGE_CAPACITY, STAGE_SLOTS>,
        delay: D,
        observer: &'pool dyn RxPipelineObserver,
    ) -> (
        Self,
        AccessPointRxConsumer<'pool, 'queue, M, QUEUE_DEPTH, STAGE_CAPACITY, STAGE_SLOTS>,
    ) {
        let (sender, frames) = queue.split();
        (
            Self {
                inner: StagedRxEpoch::from_live_with_pipeline_observer(
                    ring, storage, pool, sender, delay, observer,
                ),
            },
            AccessPointRxConsumer {
                frames,
                deferred: None,
            },
        )
    }

    /// Attach value-only pipeline observations without exposing descriptor
    /// ownership to the AP protocol layer.
    #[cfg(any(feature = "diagnostics", test))]
    pub fn from_halted_with_pipeline_observer(
        ring: RxRingHalted<'storage, COUNT>,
        storage: &'static ReceiveDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
        pool: &'pool RxStagePool<STAGE_SLOTS, STAGE_CAPACITY>,
        queue: &'queue StagedRxQueue<'pool, M, QUEUE_DEPTH, STAGE_CAPACITY, STAGE_SLOTS>,
        delay: D,
        observer: &'pool dyn RxPipelineObserver,
    ) -> (
        Self,
        AccessPointRxConsumer<'pool, 'queue, M, QUEUE_DEPTH, STAGE_CAPACITY, STAGE_SLOTS>,
    ) {
        let (sender, frames) = queue.split();
        (
            Self {
                inner: StagedRxEpoch::from_halted_with_pipeline_observer(
                    ring, storage, pool, sender, delay, observer,
                ),
            },
            AccessPointRxConsumer {
                frames,
                deferred: None,
            },
        )
    }

    pub fn try_into_halted(self) -> Result<RxRingHalted<'storage, COUNT>, Self> {
        let Self { inner } = self;
        match inner.try_into_halted() {
            Ok(ring) => Ok(ring),
            Err(inner) => Err(Self { inner }),
        }
    }

    pub fn try_into_live(self) -> Result<RxRingLive<'storage, COUNT>, Self> {
        let Self { inner } = self;
        match inner.try_into_live_ring() {
            Ok(ring) => Ok(ring),
            Err(inner) => Err(Self { inner }),
        }
    }

    /// Return a live ring together with the same standalone producer retained
    /// across the logical AP epoch.
    #[allow(clippy::type_complexity, clippy::result_large_err)]
    pub fn try_into_live_epoch_parts(
        self,
    ) -> Result<
        (
            RxRingLive<'storage, COUNT>,
            crate::datapath::rx::dma::RxEpochResources<
                'storage,
                'pool,
                'queue,
                D,
                M,
                QUEUE_DEPTH,
                COUNT,
                STAGE_CAPACITY,
                STAGE_SLOTS,
                DMA_BUFFER_SIZE,
                DMA_STORAGE_SIZE,
            >,
        ),
        Self,
    > {
        let Self { inner } = self;
        inner
            .try_into_live_epoch_parts()
            .map_err(|inner| Self { inner })
    }
}

impl<
    'storage,
    'pool,
    'queue,
    D,
    M: RawMutex,
    const QUEUE_DEPTH: usize,
    const COUNT: usize,
    const STAGE_CAPACITY: usize,
    const STAGE_SLOTS: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> AccessPointRxProducerObservation<COUNT>
    for AccessPointReceiveProducer<
        'storage,
        'pool,
        'queue,
        D,
        M,
        QUEUE_DEPTH,
        COUNT,
        STAGE_CAPACITY,
        STAGE_SLOTS,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
    >
where
    D: oer_time::Timer,
{
    fn serviced_descriptors(&self) -> u64 {
        self.inner.serviced_descriptors()
    }

    fn work_counters(&self) -> DatapathRxWorkCounters {
        self.inner.work_counters()
    }

    fn descriptor_snapshot(&self, index: usize) -> Option<RxDescriptorSnapshot> {
        self.inner.descriptor_snapshot(index)
    }

    fn scheduler_snapshot(&self) -> Option<RxFrontierSchedulerSnapshot> {
        self.inner.scheduler_snapshot()
    }
}

impl<
    'storage,
    'pool,
    'queue,
    D,
    M: RawMutex,
    H: RxDma,
    const QUEUE_DEPTH: usize,
    const COUNT: usize,
    const STAGE_CAPACITY: usize,
    const STAGE_SLOTS: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> AccessPointRxProducer<H, COUNT>
    for AccessPointReceiveProducer<
        'storage,
        'pool,
        'queue,
        D,
        M,
        QUEUE_DEPTH,
        COUNT,
        STAGE_CAPACITY,
        STAGE_SLOTS,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
    >
where
    D: oer_time::Timer,
{
    async fn start(&mut self, hardware: &mut H) -> Result<(), RxStageTransactionError> {
        self.inner.start(hardware).await
    }

    async fn stage_completed(
        &mut self,
        hardware: &mut H,
    ) -> Result<DatapathRxProgress, RxStageTransactionError> {
        self.inner.service(hardware).await
    }

    fn stop(&mut self, _hardware: &mut H) -> Result<(), RxStageTransactionError> {
        self.inner.park_for_role_handoff()
    }
}

impl<
    'pool,
    'queue,
    M: RawMutex,
    const QUEUE_DEPTH: usize,
    const STAGE_CAPACITY: usize,
    const STAGE_SLOTS: usize,
> AccessPointRxProtocolConsumer
    for AccessPointRxConsumer<'pool, 'queue, M, QUEUE_DEPTH, STAGE_CAPACITY, STAGE_SLOTS>
{
    const MAXIMUM_RETAINED_FRAMES: usize = STAGE_SLOTS;

    type Frame = StagedRxFrame<'pool, STAGE_CAPACITY, STAGE_SLOTS>;

    #[inline(always)]
    fn try_receive(&mut self) -> Option<Self::Frame> {
        self.deferred
            .take()
            .or_else(|| self.frames.try_receive().ok())
    }

    #[inline(always)]
    fn try_receive_during_tx(&mut self, security: LinkProtection) -> Option<Self::Frame> {
        if self.deferred.is_some() {
            return None;
        }
        let frame = self.frames.try_receive().ok()?;
        if can_process_ap_frame_during_tx(frame.segment(), security) {
            Some(frame)
        } else {
            self.deferred = Some(frame);
            None
        }
    }

    fn queued_frames(&self) -> usize {
        self.frames.len() + usize::from(self.deferred.is_some())
    }

    fn discard_queued(&mut self) -> usize {
        let mut discarded = usize::from(self.deferred.take().is_some());
        while let Ok(frame) = self.frames.try_receive() {
            drop(frame);
            discarded = discarded.saturating_add(1);
        }
        discarded
    }
}

#[cfg(test)]
mod classification_tests;
