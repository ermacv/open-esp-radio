//! Role-local monitor resources for the ESP32-S31 radio supervisor.
//!
//! This module does not own a second radio runner or interrupt handler. A
//! monitor epoch consumes these resources together with the role-neutral Wi-Fi
//! owner and returns both only after RX DMA and the shared IRQ route quiesce.

use core::sync::atomic::{AtomicU32, Ordering};

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;

use oer_radio::wifi::RadioSubsystemGeneration;

pub use oer_radio::wifi::{
    MONITOR_CHANNEL_SEQUENCE_CAPACITY, MonitorCapturePolicy, MonitorChannelPolicy,
    MonitorChannelSequence, MonitorChannelSequenceError, MonitorRequest,
};

use oer_esp32s31_ieee80211_runtime::roles::monitor::{
    MonitorControlResources, MonitorExecutionResources, MonitorRxRing,
    MonitorStoppedExecutionResources, MonitorStorage, MonitorTask, MonitorTaskBuildFailure,
};

use oer_esp32s31_ieee80211_esp_hal::mac_interrupt_epoch::EspHalMacInterruptRoute;

use oer_esp32s31_ieee80211_mac::rx::RxPhyInfo;

pub use oer_esp32s31_ieee80211_mac::rx::{
    RxBasebandFormat as Esp32s31MonitorBasebandFormat, RxPhyInfo as Esp32s31MonitorPhyInfo,
};

use crate::{
    resources::profile::{
        ESP32S31_DEFAULT_RX_BUFFER_SIZE as RX_BUFFER_SIZE,
        ESP32S31_DEFAULT_RX_BUFFER_STORAGE_SIZE as RX_STORAGE_SIZE,
        ESP32S31_DEFAULT_RX_DESCRIPTOR_COUNT as RX_DESCRIPTOR_COUNT,
    },
    supervisor::ProductionRxRing,
};

use oer_ieee80211_runtime::{
    MonitorCaptureFrame, MonitorCapturePool, MonitorCaptureReceiver, MonitorCaptureResources,
    MonitorCaptureSink,
};

use oer_ieee80211_softmac::{
    MonitorDropReason, MonitorFrame, MonitorInjectionChannelBinding, MonitorPublishOutcome,
    MonitorSink, WifiChannel,
};

use static_cell::{ConstStaticCell, StaticCell};

const CAPTURE_DEPTH: usize = 8;
const CAPTURE_SLOTS: usize = 8;
// Normalized monitor frames are views into one completed RX DMA segment. The
// segment cannot exceed the production RX buffer, so a larger retained slot
// only consumes internal SRAM without allowing an additional valid frame.
pub const ESP32S31_MONITOR_CAPTURE_CAPACITY: usize = RX_BUFFER_SIZE;
const CAPTURE_CAPACITY: usize = ESP32S31_MONITOR_CAPTURE_CAPACITY;

pub type ReceivedMonitorFrame =
    MonitorCaptureFrame<'static, RxPhyInfo, ESP32S31_MONITOR_CAPTURE_CAPACITY>;

type CapturePool = MonitorCapturePool<CAPTURE_CAPACITY, CAPTURE_SLOTS>;
pub(super) type CaptureResources = MonitorCaptureResources<
    'static,
    CriticalSectionRawMutex,
    RxPhyInfo,
    CAPTURE_DEPTH,
    CAPTURE_CAPACITY,
    CAPTURE_SLOTS,
>;
type RawCaptureSink = MonitorCaptureSink<
    'static,
    'static,
    CriticalSectionRawMutex,
    RxPhyInfo,
    CAPTURE_DEPTH,
    CAPTURE_CAPACITY,
    CAPTURE_SLOTS,
>;

/// Value-only capture accounting for the single production monitor stream.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MonitorCaptureStatistics {
    pub generation: u32,
    pub published_frames: u32,
    pub full_drops: u32,
    pub oversized_drops: u32,
    pub discarded_frames: u32,
}

struct MonitorCaptureCounters {
    generation: AtomicU32,
    published_frames: AtomicU32,
    full_drops: AtomicU32,
    oversized_drops: AtomicU32,
    discarded_frames: AtomicU32,
}

impl MonitorCaptureCounters {
    const fn new() -> Self {
        Self {
            generation: AtomicU32::new(0),
            published_frames: AtomicU32::new(0),
            full_drops: AtomicU32::new(0),
            oversized_drops: AtomicU32::new(0),
            discarded_frames: AtomicU32::new(0),
        }
    }

    fn begin_epoch(&self, generation: u32) {
        self.published_frames.store(0, Ordering::Relaxed);
        self.full_drops.store(0, Ordering::Relaxed);
        self.oversized_drops.store(0, Ordering::Relaxed);
        self.discarded_frames.store(0, Ordering::Relaxed);
        self.generation.store(generation, Ordering::Release);
    }

    fn snapshot(&self) -> MonitorCaptureStatistics {
        MonitorCaptureStatistics {
            generation: self.generation.load(Ordering::Acquire),
            published_frames: self.published_frames.load(Ordering::Relaxed),
            full_drops: self.full_drops.load(Ordering::Relaxed),
            oversized_drops: self.oversized_drops.load(Ordering::Relaxed),
            discarded_frames: self.discarded_frames.load(Ordering::Relaxed),
        }
    }
}

static CAPTURE_COUNTERS: MonitorCaptureCounters = MonitorCaptureCounters::new();

pub(super) fn record_discarded_monitor_frames(discarded: usize) {
    CAPTURE_COUNTERS.discarded_frames.fetch_add(
        u32::try_from(discarded).unwrap_or(u32::MAX),
        Ordering::Relaxed,
    );
}

/// Transparent sink wrapper: accounting stays in one static instead of adding
/// a pointer to every monitor owner carried through the radio async state.
/// Keeping this handle the same size as `RawCaptureSink` avoids large
/// by-value owner states becoming transient executor-stack allocations. With
/// the counters pointer embedded, the ESP32-S31 release build's radio poll
/// frame grew from `0x2dd0` to `0x66d90` bytes and overwrote static station
/// memory before crossing the RAM boundary.
pub(super) struct CaptureSink {
    inner: RawCaptureSink,
}

const _: () = assert!(
    core::mem::size_of::<CaptureSink>() == core::mem::size_of::<RawCaptureSink>(),
    "monitor accounting must not enlarge the role owner carried by the radio future",
);

impl CaptureSink {
    fn configure(&mut self, generation: u32, snapshot_length: Option<usize>) {
        self.inner.configure(generation, snapshot_length);
    }
}

impl MonitorSink<RxPhyInfo> for CaptureSink {
    fn begin_channel_epoch(&mut self, channel: WifiChannel) {
        self.inner.begin_channel_epoch(channel);
    }

    fn end_channel_epoch(&mut self) {
        self.inner.end_channel_epoch();
    }

    fn injection_channel_binding(&self) -> Option<MonitorInjectionChannelBinding> {
        self.inner.injection_channel_binding()
    }

    fn try_publish(&mut self, frame: MonitorFrame<'_, RxPhyInfo>) -> MonitorPublishOutcome {
        let outcome = self.inner.try_publish(frame);
        let counter = match outcome {
            MonitorPublishOutcome::Published => &CAPTURE_COUNTERS.published_frames,
            MonitorPublishOutcome::Dropped(MonitorDropReason::Full) => &CAPTURE_COUNTERS.full_drops,
            MonitorPublishOutcome::Dropped(MonitorDropReason::TooLong) => {
                &CAPTURE_COUNTERS.oversized_drops
            }
            MonitorPublishOutcome::Dropped(MonitorDropReason::Filtered) => return outcome,
        };
        counter.fetch_add(1, Ordering::Relaxed);
        outcome
    }
}
type CaptureReceiver = MonitorCaptureReceiver<
    'static,
    'static,
    CriticalSectionRawMutex,
    RxPhyInfo,
    CAPTURE_DEPTH,
    CAPTURE_CAPACITY,
>;

pub(super) type MonitorTaskResources = MonitorExecutionResources<
    'static,
    CriticalSectionRawMutex,
    CaptureSink,
    RX_DESCRIPTOR_COUNT,
    RX_BUFFER_SIZE,
    RX_STORAGE_SIZE,
>;
pub(super) type MonitorMemory =
    MonitorStorage<RX_DESCRIPTOR_COUNT, RX_BUFFER_SIZE, RX_STORAGE_SIZE>;
pub(super) type MonitorStoppedResources = MonitorStoppedExecutionResources<
    'static,
    CriticalSectionRawMutex,
    CaptureSink,
    RX_DESCRIPTOR_COUNT,
    RX_BUFFER_SIZE,
    RX_STORAGE_SIZE,
>;
pub(super) type ProductionMonitorTask = MonitorTask<
    'static,
    oer_esp32s31_ieee80211_esp_hal::EspHalWifiPlatform,
    crate::supervisor::WifiMembership,
    EspHalMacInterruptRoute,
    CriticalSectionRawMutex,
    CaptureSink,
    RX_DESCRIPTOR_COUNT,
    RX_BUFFER_SIZE,
    RX_STORAGE_SIZE,
>;
pub(super) type ProductionMonitorBuildFailure = MonitorTaskBuildFailure<
    'static,
    oer_esp32s31_ieee80211_esp_hal::EspHalWifiPlatform,
    crate::supervisor::WifiMembership,
    EspHalMacInterruptRoute,
    CriticalSectionRawMutex,
    CaptureSink,
    RX_DESCRIPTOR_COUNT,
    RX_BUFFER_SIZE,
    RX_STORAGE_SIZE,
>;

static CAPTURE_POOL: ConstStaticCell<CapturePool> = ConstStaticCell::new(CapturePool::new());
static CAPTURE_RESOURCES: StaticCell<CaptureResources> = StaticCell::new();
static MONITOR_CONTROL: ConstStaticCell<MonitorControlResources<CriticalSectionRawMutex>> =
    ConstStaticCell::new(MonitorControlResources::new());

/// Application-owned monitor capture stream. Frames carry their supervisor
/// generation, so a lease retained across a role transition is unambiguous.
// CAPABILITY: wifi-interfaces-and-operating-modes-promiscuous-mode
pub struct MonitorFrames {
    receiver: CaptureReceiver,
}

impl MonitorFrames {
    pub fn try_receive(&self) -> Option<ReceivedMonitorFrame> {
        self.receiver.try_receive()
    }

    pub async fn receive(&self) -> ReceivedMonitorFrame {
        self.receiver.receive().await
    }

    /// Snapshot capture-pool publication and loss for the current generation.
    pub fn statistics(&self) -> MonitorCaptureStatistics {
        CAPTURE_COUNTERS.snapshot()
    }
}

pub(super) struct ProductionMonitorResources {
    memory: MonitorMemory,
    sink: CaptureSink,
    control: &'static MonitorControlResources<CriticalSectionRawMutex>,
}

impl ProductionMonitorResources {
    pub(super) fn bind(
        mut self,
        generation: RadioSubsystemGeneration,
        snapshot_length: Option<u16>,
        rx_ring: Option<ProductionRxRing>,
    ) -> MonitorTaskResources {
        CAPTURE_COUNTERS.begin_epoch(generation.value());
        self.sink
            .configure(generation.value(), snapshot_length.map(usize::from));
        MonitorExecutionResources::new(
            self.memory,
            rx_ring.map(|ring| match ring {
                ProductionRxRing::Halted(ring) => MonitorRxRing::Halted(ring),
                ProductionRxRing::Live(ring) => MonitorRxRing::Live(ring),
            }),
            self.sink,
            self.control,
        )
    }

    pub(super) fn from_stopped(resources: MonitorStoppedResources) -> (Self, ProductionRxRing) {
        let parts = resources.into_parts();
        (
            Self {
                memory: parts.memory,
                sink: parts.sink,
                control: parts.control,
            },
            ProductionRxRing::Live(parts.live_ring),
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum MonitorResourcesError {
    InUse,
}

pub(super) struct MonitorProductResources {
    pub(super) role: ProductionMonitorResources,
    pub(super) capture: &'static CaptureResources,
    pub(super) frames: MonitorFrames,
}

pub(super) fn initialize_monitor_resources(
    memory: MonitorMemory,
) -> Result<MonitorProductResources, MonitorResourcesError> {
    let capture = CAPTURE_RESOURCES
        .try_init_with(|| CaptureResources::new(CAPTURE_POOL.take()))
        .ok_or(MonitorResourcesError::InUse)?;
    let (sink, receiver) = capture.split();
    Ok(MonitorProductResources {
        role: ProductionMonitorResources {
            memory,
            sink: CaptureSink { inner: sink },
            control: MONITOR_CONTROL.take(),
        },
        capture,
        frames: MonitorFrames { receiver },
    })
}
