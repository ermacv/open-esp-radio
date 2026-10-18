//! Production ownership of the ESP32-S31 Wi-Fi RX descriptor ring.
//!
//! DMA storage and the live descriptor frontier are kept in one finite radio
//! service. A completed single-buffer unit transfers its original allocation
//! through a bounded affine lease to the protocol and network consumers. The
//! descriptor stays CPU-owned until that lease returns; the radio owner then
//! rearms only the contiguous released prefix. At most 32 of 96 buffers may be
//! retained above the radio owner, leaving 64 descriptors in the radio domain.
//! A delayed masked-IRQ epoch can still consume that finite accepted list, so
//! this is capacity isolation rather than a minimum armed-credit guarantee.

#![forbid(unsafe_code)]

#[cfg(feature = "core0-rx-coarse-telemetry")]
use core::sync::atomic::{AtomicBool, Ordering};

use core::{future::Future, marker::PhantomData};

use embassy_sync::blocking_mutex::raw::RawMutex;

pub use oer_esp32s31_ieee80211::rx::storage::{
    ESP32S31_RX_BUFFER_SIZE, ESP32S31_RX_BUFFER_STORAGE_SIZE, ESP32S31_RX_DESCRIPTOR_COUNT,
    ESP32S31_RX_WALKER_ENABLE_SETTLE_US, ReceiveDmaBuffer, ReceiveDmaStorage, walker_enable_settle,
};

use oer_esp32s31_ieee80211_mac::rx::{
    RxDescriptorSnapshot, RxDma, RxRingError, RxRingHalted, RxRingLive, RxRingStopped,
    decode_rx_local_timestamp,
    pool::{
        RxStagePool, RxStageTransactionError, RxTimes, VENDOR_LARGE_RX_PAYLOAD_CAPACITY,
        VENDOR_LARGE_RX_SLOT_COUNT,
    },
};

use oer_ieee80211_mac::vif::{StaApRxRoute, StaApVif, classify_sta_ap_rx};

#[cfg(any(feature = "diagnostics", test))]
use crate::diagnostics::rx_pipeline::RxPipelineObserver;
#[cfg(test)]
use crate::diagnostics::rx_pipeline::{RxPipelineObservation, RxStageDiscard};

use crate::datapath::{
    DatapathRxProgress, DatapathRxWorkCounters,
    rx::{
        routed::{StaApStagedRxFrame, StaApStagedRxSender},
        staging::{StagedRxFrame, StagedRxReceiver, StagedRxSender, StagedRxTrySendError},
    },
    services::DatapathRxService,
};

#[cfg(feature = "core0-rx-coarse-telemetry")]
static INTERRUPT_DRIVEN_RECYCLED_APPEND: AtomicBool = AtomicBool::new(false);

/// Select the same-image diagnostic which completes a recycled-only RX turn
/// by restoring the level-triggered MAC interrupt instead of reposting an
/// unconditional software probe.
///
/// Terminal writeback, completed-frontier and exhausted-republication proofs
/// retain their cooperative continuation. This selector therefore removes
/// only the conservative append confirmation, not hardware lifecycle checks.
#[cfg(feature = "core0-rx-coarse-telemetry")]
pub fn configure_interrupt_driven_recycled_append_for_diagnostics(enabled: bool) {
    INTERRUPT_DRIVEN_RECYCLED_APPEND.store(enabled, Ordering::Relaxed);
}

#[cfg(feature = "core0-rx-coarse-telemetry")]
fn interrupt_driven_recycled_append_for_diagnostics() -> bool {
    INTERRUPT_DRIVEN_RECYCLED_APPEND.load(Ordering::Relaxed)
}

pub use oer_esp32s31_ieee80211::rx::transaction::{
    Admission as Esp32s31RxStageAdmissionPolicy, AdmitAll as FullRxStageAdmission,
    AdmitUnreserved as UnreservedRxStageAdmission, CompletedUnit as Esp32s31RxCompletedUnit,
    CreditPolicy as RxStageCreditPolicy, IngressClass as Esp32s31RxIngressClass,
    IngressRoute as Esp32s31RxIngressRoute, Observation as Esp32s31RxIngressObservation,
    Preview as Esp32s31RxCompletedUnitPreview, Unavailable as RxStageUnavailableDisposition,
};

/// Permanent publication strategy owned by the single physical RX producer.
///
/// Standalone STA/AP epochs publish an unclassified lease to their sole
/// protocol consumer. A same-channel STA+AP epoch instead publishes exactly
/// one ordered stream carrying the fact-only VIF route. Neither variant owns
/// protocol policy, and neither can manufacture or duplicate a staging lease.
pub enum StagedRxPublisher<
    'pool,
    'queue,
    M: RawMutex,
    const QUEUE_DEPTH: usize,
    const STAGE_CAPACITY: usize,
    const STAGE_SLOTS: usize,
> {
    Standalone(StagedRxSender<'queue, 'pool, M, QUEUE_DEPTH, STAGE_CAPACITY, STAGE_SLOTS>),
    StaAp {
        frames: StaApStagedRxSender<'pool, 'queue, M, QUEUE_DEPTH, STAGE_CAPACITY, STAGE_SLOTS>,
        ingress: oer_esp32s31_ieee80211_mac::rx::RxIngressConfig,
        addresses: oer_ieee80211_mac::vif::StaApRxAddresses,
    },
}

impl<
    'pool,
    'queue,
    M: RawMutex,
    const QUEUE_DEPTH: usize,
    const STAGE_CAPACITY: usize,
    const STAGE_SLOTS: usize,
> StagedRxPublisher<'pool, 'queue, M, QUEUE_DEPTH, STAGE_CAPACITY, STAGE_SLOTS>
{
    pub const fn standalone(
        frames: StagedRxSender<'queue, 'pool, M, QUEUE_DEPTH, STAGE_CAPACITY, STAGE_SLOTS>,
    ) -> Self {
        Self::Standalone(frames)
    }

    pub const fn sta_ap(
        frames: StaApStagedRxSender<'pool, 'queue, M, QUEUE_DEPTH, STAGE_CAPACITY, STAGE_SLOTS>,
        ingress: oer_esp32s31_ieee80211_mac::rx::RxIngressConfig,
        addresses: oer_ieee80211_mac::vif::StaApRxAddresses,
    ) -> Self {
        Self::StaAp {
            frames,
            ingress,
            addresses,
        }
    }

    fn free_capacity(&self) -> usize {
        match self {
            Self::Standalone(frames) => frames.free_capacity(),
            Self::StaAp { frames, .. } => frames.free_capacity(),
        }
    }

    fn len(&self) -> usize {
        match self {
            Self::Standalone(frames) => frames.len(),
            Self::StaAp { frames, .. } => frames.len(),
        }
    }

    fn try_resume_standalone_receiver(
        &self,
    ) -> Option<StagedRxReceiver<'queue, 'pool, M, QUEUE_DEPTH, STAGE_CAPACITY, STAGE_SLOTS>> {
        match self {
            Self::Standalone(frames) => Some(frames.resume_receiver()),
            Self::StaAp { .. } => None,
        }
    }

    fn preview(
        &self,
        unit: Esp32s31RxCompletedUnit,
        bytes: [u8; 24],
    ) -> Esp32s31RxCompletedUnitPreview {
        let frame_control = Some(u16::from_le_bytes([bytes[0], bytes[1]]));
        let class = Esp32s31RxIngressClass::of(frame_control);
        let route = match self {
            Self::Standalone(_) => Esp32s31RxIngressRoute::Standalone,
            Self::StaAp { addresses, .. } => match classify_sta_ap_rx(&bytes, *addresses) {
                StaApRxRoute::Interface(StaApVif::Station) => Esp32s31RxIngressRoute::Station,
                StaApRxRoute::Interface(StaApVif::AccessPoint) => {
                    Esp32s31RxIngressRoute::AccessPoint
                }
                StaApRxRoute::Foreign => Esp32s31RxIngressRoute::Foreign,
                StaApRxRoute::Ambiguous => Esp32s31RxIngressRoute::Ambiguous,
                StaApRxRoute::Malformed => Esp32s31RxIngressRoute::Malformed,
            },
        };
        Esp32s31RxCompletedUnitPreview {
            unit,
            frame_control,
            class,
            route,
        }
    }

    const fn unclassified_preview(
        &self,
        unit: Esp32s31RxCompletedUnit,
    ) -> Esp32s31RxCompletedUnitPreview {
        Esp32s31RxCompletedUnitPreview {
            unit,
            frame_control: None,
            class: Esp32s31RxIngressClass::Unclassified,
            route: match self {
                Self::Standalone(_) => Esp32s31RxIngressRoute::Standalone,
                Self::StaAp { .. } => Esp32s31RxIngressRoute::Malformed,
            },
        }
    }

    fn try_send(
        &self,
        frame: StagedRxFrame<'pool, STAGE_CAPACITY, STAGE_SLOTS>,
    ) -> Result<(), StagedRxFrame<'pool, STAGE_CAPACITY, STAGE_SLOTS>> {
        match self {
            Self::Standalone(frames) => frames.try_send(frame).map_err(|error| match error {
                StagedRxTrySendError(frame) => frame,
            }),
            Self::StaAp {
                frames,
                ingress,
                addresses,
            } => frames
                .try_send(StaApStagedRxFrame::classify(frame, *ingress, *addresses))
                .map_err(|error| error.0.into_frame()),
        }
    }
}

/// Complete production RX owner for one descriptor-ring epoch.
///
/// The default ring state is live. Pause and resume consume this owner and
/// change only `R`; storage, leases, admission and progress remain intact.
/// RX service is available only on the live specialization.
pub struct StagedRxProducer<
    'storage,
    'pool,
    'queue,
    D,
    M: RawMutex,
    const QUEUE_DEPTH: usize = VENDOR_LARGE_RX_SLOT_COUNT,
    const COUNT: usize = ESP32S31_RX_DESCRIPTOR_COUNT,
    const STAGE_CAPACITY: usize = VENDOR_LARGE_RX_PAYLOAD_CAPACITY,
    const STAGE_SLOTS: usize = VENDOR_LARGE_RX_SLOT_COUNT,
    const DMA_BUFFER_SIZE: usize = ESP32S31_RX_BUFFER_SIZE,
    const DMA_STORAGE_SIZE: usize = ESP32S31_RX_BUFFER_STORAGE_SIZE,
    P = FullRxStageAdmission,
    R = RxRingLive<'storage, COUNT>,
> {
    ring: R,
    ring_lifetime: PhantomData<&'storage ()>,
    storage: &'static ReceiveDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    pool: &'pool RxStagePool<STAGE_SLOTS, STAGE_CAPACITY>,
    frames: StagedRxPublisher<'pool, 'queue, M, QUEUE_DEPTH, STAGE_CAPACITY, STAGE_SLOTS>,
    delay: D,
    #[cfg(any(feature = "diagnostics", test))]
    pipeline_observer: Option<&'pool dyn RxPipelineObserver>,
    admission: P,
    serviced_descriptors: u64,
    serviced_units: u64,
    serviced_bytes: u64,
}

/// Connected RX resources after the DMA walker is confirmed stopped.
///
/// This owner deliberately retains the queue sender, staging pool and reload
/// delay together with the halted descriptor storage. A later station epoch
/// can therefore reconstruct the same production RX service without stealing
/// static resources or retaining any frontier from the previous peer.
pub struct StoppedReceive<
    'storage,
    'pool,
    'queue,
    D,
    M: RawMutex,
    const QUEUE_DEPTH: usize = VENDOR_LARGE_RX_SLOT_COUNT,
    const COUNT: usize = ESP32S31_RX_DESCRIPTOR_COUNT,
    const STAGE_CAPACITY: usize = VENDOR_LARGE_RX_PAYLOAD_CAPACITY,
    const STAGE_SLOTS: usize = VENDOR_LARGE_RX_SLOT_COUNT,
    const DMA_BUFFER_SIZE: usize = ESP32S31_RX_BUFFER_SIZE,
    const DMA_STORAGE_SIZE: usize = ESP32S31_RX_BUFFER_STORAGE_SIZE,
> {
    ring: RxRingHalted<'storage, COUNT>,
    storage: &'static ReceiveDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    pool: &'pool RxStagePool<STAGE_SLOTS, STAGE_CAPACITY>,
    frames: StagedRxPublisher<'pool, 'queue, M, QUEUE_DEPTH, STAGE_CAPACITY, STAGE_SLOTS>,
    delay: D,
    #[cfg(any(feature = "diagnostics", test))]
    pipeline_observer: Option<&'pool dyn RxPipelineObserver>,
}

/// Peer-independent resources retained while a halted ring is used by a
/// finite Authentication/Association/WPA2 receive epoch.
///
/// Splitting these resources from [`RxRingHalted`] lets the station lifecycle
/// pass the descriptor frontier through its pre-connected type state without
/// discarding the production staging pool, queue sender, observation delay or
/// telemetry binding. Reassembly consumes both owners, so it cannot create a
/// second connected RX service for the same static storage.
pub struct RxEpochResources<
    'storage,
    'pool,
    'queue,
    D,
    M: RawMutex,
    const QUEUE_DEPTH: usize = VENDOR_LARGE_RX_SLOT_COUNT,
    const COUNT: usize = ESP32S31_RX_DESCRIPTOR_COUNT,
    const STAGE_CAPACITY: usize = VENDOR_LARGE_RX_PAYLOAD_CAPACITY,
    const STAGE_SLOTS: usize = VENDOR_LARGE_RX_SLOT_COUNT,
    const DMA_BUFFER_SIZE: usize = ESP32S31_RX_BUFFER_SIZE,
    const DMA_STORAGE_SIZE: usize = ESP32S31_RX_BUFFER_STORAGE_SIZE,
> {
    ring_lifetime: PhantomData<&'storage ()>,
    storage: &'static ReceiveDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    pool: &'pool RxStagePool<STAGE_SLOTS, STAGE_CAPACITY>,
    frames: StagedRxPublisher<'pool, 'queue, M, QUEUE_DEPTH, STAGE_CAPACITY, STAGE_SLOTS>,
    delay: D,
    #[cfg(any(feature = "diagnostics", test))]
    pipeline_observer: Option<&'pool dyn RxPipelineObserver>,
}

/// Connected RX resources after descriptor rebuild but before walker enable.
///
/// Keeping this state distinct preserves the platform settle edge and returns
/// the complete owner if walker activation fails. A reconnecting station can
/// retry or reset without stealing descriptor, buffer, queue or pool storage.
pub struct PreparedRx<
    'storage,
    'pool,
    'queue,
    D,
    M: RawMutex,
    const QUEUE_DEPTH: usize = VENDOR_LARGE_RX_SLOT_COUNT,
    const COUNT: usize = ESP32S31_RX_DESCRIPTOR_COUNT,
    const STAGE_CAPACITY: usize = VENDOR_LARGE_RX_PAYLOAD_CAPACITY,
    const STAGE_SLOTS: usize = VENDOR_LARGE_RX_SLOT_COUNT,
    const DMA_BUFFER_SIZE: usize = ESP32S31_RX_BUFFER_SIZE,
    const DMA_STORAGE_SIZE: usize = ESP32S31_RX_BUFFER_STORAGE_SIZE,
> {
    ring: RxRingStopped<'storage, COUNT>,
    storage: &'static ReceiveDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    pool: &'pool RxStagePool<STAGE_SLOTS, STAGE_CAPACITY>,
    frames: StagedRxPublisher<'pool, 'queue, M, QUEUE_DEPTH, STAGE_CAPACITY, STAGE_SLOTS>,
    delay: D,
    #[cfg(any(feature = "diagnostics", test))]
    pipeline_observer: Option<&'pool dyn RxPipelineObserver>,
}

mod epoch;
mod lifecycle;
mod service;

pub use epoch::StagedRxEpoch;

#[cfg(test)]
mod tests;

/// The staged publisher as the RX transaction sees it: each frame is
/// timestamped on `clock` at its first executor-visible handoff.
pub(crate) struct ClockedRxPublisher<
    'a,
    'pool,
    'queue,
    M: RawMutex,
    D,
    const QUEUE_DEPTH: usize,
    const STAGE_CAPACITY: usize,
    const STAGE_SLOTS: usize,
> {
    pub(crate) frames:
        &'a StagedRxPublisher<'pool, 'queue, M, QUEUE_DEPTH, STAGE_CAPACITY, STAGE_SLOTS>,
    pub(crate) clock: &'a D,
}

impl<
    'pool,
    'queue,
    M: RawMutex,
    D: oer_time::Clock,
    const QUEUE_DEPTH: usize,
    const STAGE_CAPACITY: usize,
    const STAGE_SLOTS: usize,
> oer_esp32s31_ieee80211::rx::transaction::Publisher<'pool, STAGE_CAPACITY, STAGE_SLOTS>
    for ClockedRxPublisher<'_, 'pool, 'queue, M, D, QUEUE_DEPTH, STAGE_CAPACITY, STAGE_SLOTS>
{
    const DEPTH: usize = QUEUE_DEPTH;

    #[inline(always)]
    fn free_capacity(&self) -> usize {
        self.frames.free_capacity()
    }
    #[inline(always)]
    fn preview(
        &self,
        unit: Esp32s31RxCompletedUnit,
        bytes: [u8; 24],
    ) -> Esp32s31RxCompletedUnitPreview {
        self.frames.preview(unit, bytes)
    }
    #[inline(always)]
    fn unclassified_preview(
        &self,
        unit: Esp32s31RxCompletedUnit,
    ) -> Esp32s31RxCompletedUnitPreview {
        self.frames.unclassified_preview(unit)
    }
    #[inline(always)]
    fn try_send(
        &self,
        mut frame: StagedRxFrame<'pool, STAGE_CAPACITY, STAGE_SLOTS>,
    ) -> Result<(), StagedRxFrame<'pool, STAGE_CAPACITY, STAGE_SLOTS>> {
        // Timestamp the first executor-visible handoff, before either the
        // standalone or same-channel routing queue can delay protocol parsing.
        // Retried publication preserves this first sample in the affine frame.
        let handoff = self.clock.now();
        let buffer = frame.segment().buffer;
        #[cfg(feature = "rx-clock-probe")]
        super::clock_probe::observe(buffer, handoff);
        // The receive timestamp travels as the raw value the MAC wrote; a
        // consumer that needs the reception time converts it.
        frame.mark_handoff(RxTimes {
            handoff,
            stamp: decode_rx_local_timestamp(buffer),
        });
        self.frames.try_send(frame)
    }
}
