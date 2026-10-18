#![expect(
    clippy::large_enum_variant,
    reason = "no-alloc connected outcomes retain concrete reusable or faulted owner frontiers"
)]

//! Connected Embassy composition for the standalone station application.
//!
//! This module composes station resources and network bindings with the
//! reusable PAC/DMA/IRQ and 802.11 protocol transitions. [`execution`] owns
//! the same-Core0 child task and exact runner return; the station composition
//! retains the subsequent quiescence, teardown and quarantine obligations.
//! The crate-level interrupt binding owns the shared MAC/power ISR publication.

#[cfg(feature = "diagnostics")]
use core::sync::atomic::{AtomicU32, Ordering};

use crate::resources::profile::{
    ESP32S31_DEFAULT_CONTROL_QUEUE_DEPTH as CONTROL_QUEUE_DEPTH,
    ESP32S31_DEFAULT_NETWORK_FRAME_CAPACITY as NETWORK_FRAME_CAPACITY,
    ESP32S31_DEFAULT_NETWORK_TX_QUEUE_DEPTH as NETWORK_TX_QUEUE_DEPTH,
    ESP32S31_DEFAULT_NETWORK_TX_TRAILER as NETWORK_TX_TRAILER,
    ESP32S31_DEFAULT_RX_REORDER_WINDOW as RX_REORDER_WINDOW,
    ESP32S31_DEFAULT_RX_STAGE_CAPACITY as RX_STAGE_CAPACITY,
    ESP32S31_DEFAULT_RX_STAGE_SLOT_COUNT as RX_STAGE_SLOT_COUNT,
    ESP32S31_DEFAULT_TX_AMPDU_FRAME_COUNT as TX_AMPDU_FRAME_COUNT,
};

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;

use embassy_time::Timer;
use oer_time_embassy::EmbassyClock;

use oer_esp32s31_hal::{ieee80211::arena::RadioOwnerArena, owner::RadioRuntimeOwner};

use oer_esp32s31_ieee80211::cooperative_hardware::CooperativeRadioHardware;
#[cfg(feature = "diagnostics")]
use oer_esp32s31_ieee80211_dma::descriptor::{rx_done, rx_rearm_word};
#[cfg(feature = "diagnostics")]
use oer_esp32s31_ieee80211_runtime::roles::station::rx_protocol::ConnectedRxProtocolSink;
#[cfg(feature = "diagnostics")]
use oer_esp32s31_ieee80211_sta::connected_rx::{
    ConnectedRxEvent, ConnectedRxSink as MacConnectedRxSink,
};

use oer_esp32s31_ieee80211_runtime::roles::radio_channel::WifiReconnectPolicy;
use oer_esp32s31_ieee80211_runtime::roles::station::epoch::StoppedStaRx;
use oer_esp32s31_ieee80211_runtime::roles::station::power::{
    StationPowerLink, StationRf, finish_station_power, run_station_power_agent,
};
use oer_esp32s31_ieee80211_runtime::{
    datapath::{
        DatapathRunner, DatapathServices,
        network::DatapathNetwork,
        rx::{
            dma::{RxEpochResources, StagedRxProducer, StoppedReceive},
            frontier::{ReceiveFrontier, RxFrontierError},
            reorder::{
                RX_REORDER_BACKING_SLOT_COUNT, RxReorderCommandResources, RxReorderFrameStorage,
            },
            staging::StagedRxQueue,
        },
    },
    roles::station::connected::{
        ConnectedControlPublisher, ConnectedControlResources,
        ConnectedControlShutdown as Esp32s31ConnectedControlShutdown,
        ConnectedDriverAssemblyFailure, ConnectedEpochResources, ConnectedEpochStartFailure,
        ConnectedEpochStarted, ConnectedNetworkStartedParts, ConnectedProtocolStopped,
        ConnectedReceiveProtocol, ConnectedReceiveStorage, ConnectedServiceResources,
        ConnectedStaBlockAckPolicy, ConnectedStaCcmpReplayFailure, ConnectedStaCompositionFailure,
        ConnectedStaConfig, ConnectedStaConfigError, ConnectedStaControlResources,
        ConnectedStaGroupSecurity, ConnectedStaNetworkTxDomain, ConnectedStaPort,
        ConnectedStaRateConfig, ConnectedStaRxParked, ConnectedStaRxPolicy,
        ConnectedStaRxProtocolResources, ConnectedStaRxService, ConnectedStaSecurityStopReport,
        ConnectedStaTeardownFailure, ConnectedStaTxHandoffFailure, ConnectedStaTxPolicy,
        ConnectedStaTxResources, ConnectedStationExit, ConnectedTx, ConnectedWpa2Security,
        ConnectionServices, ConnectionTeardownFailure, DisconnectedStaEpoch,
        EmbassyNetConnectedRxSink, InitialConnectedEpochResources, ReconnectedStaEpoch,
        StaTxEpochExt, StartedNetworkConnection, StationCommand, StationCommandReceiver,
        activate_esp32s31_connected_epoch, complete_esp32s31_connected_datapath_exit,
        prepare_esp32s31_connected_service, quiesce_completed_esp32s31_connected_epoch,
        start_esp32s31_initial_connected_epoch, start_esp32s31_reconnected_connected_epoch,
    },
};

use oer_esp32s31_ieee80211_esp_hal::{
    EspHalWifiPlatform,
    mac_interrupt_epoch::{
        prepare_active_connected_sta_without_power_save,
        prepare_active_connected_sta_without_power_save_with_access,
    },
};

use oer_esp32s31_ieee80211_mac::{
    crypto::{StaGroupCcmpKeyMaterial, StaGroupCcmpSlot, StaPairwiseCcmpSlot},
    init::MacRuntimeStopHardware,
    rx::{RxIngressConfig, RxRingError, pool::RxStagePool},
    tx::{HeEdcaTxopLimit, HtGuardInterval, HtMcs, LegacyRate, ampdu::HtAmpduTxError},
};

use oer_esp32s31_ieee80211_sta::{
    attempt::{StaAttemptSecurity, StaAttemptSecurityMaterial, StaInstalledSecurity},
    connected_rx::{StaCcmpRxReplayResource, StaCcmpRxReplayStartFailure},
    single_mpdu_tx::ConnectedTxSecurity,
};

use oer_ieee80211_mac::station::StaTxSequenceCounters;
use oer_ieee80211_sta::station::{StaFault, StaFaultCause, StaFaultPhase, StaFaultReason};

use oer_ieee80211_runtime::{await_stack_boundary, station_network::RunningStationNetwork};

use static_cell::{ConstStaticCell, StaticCell};

mod execution;

pub(crate) use execution::ConnectedDatapathMailbox;

pub(super) use execution::initialize_connected_datapath_mailbox;

#[cfg(feature = "diagnostics")]
use crate::diagnostics::{ConnectedRxObservation, ConnectedRxObserver};

use crate::{
    interrupts::{IRQ_RUNTIME, MacInterruptEpoch},
    radio_resources::{
        NETWORK_TX_HEADROOM, NetworkRunner, RadioAmpduStorage, RadioTxBacking, RunningWifiNetwork,
        TX_AMPDU_BUFFER_SIZE, WifiNetworkResources,
    },
    supervisor::{
        ControlTx, ProductionRxRing, ProductionStationBoardResources, ProductionStationRuntime,
        RX_BUFFER_SIZE, RX_DESCRIPTOR_COUNT, RxStorage, StationPowerMode, TxStorage,
        production_station_runtime,
    },
};
pub(super) type ControlResources =
    ConnectedControlResources<CriticalSectionRawMutex, CONTROL_QUEUE_DEPTH>;
type ControlPublisher =
    ConnectedControlPublisher<'static, CriticalSectionRawMutex, CONTROL_QUEUE_DEPTH>;
type NetworkRxPublisher = <NetworkRunner as DatapathNetwork>::RxPublisher;
type EmbassyConnectedRxSink =
    EmbassyNetConnectedRxSink<'static, NetworkRxPublisher, ControlPublisher>;
#[cfg(feature = "diagnostics")]
type ConnectedRxSink = ObservedConnectedRxSink<EmbassyConnectedRxSink>;
#[cfg(not(feature = "diagnostics"))]
type ConnectedRxSink = EmbassyConnectedRxSink;
type ConnectedRxProtocol = ConnectedReceiveProtocol<
    'static,
    'static,
    'static,
    'static,
    CriticalSectionRawMutex,
    ConnectedRxSink,
    EmbassyClock,
    RX_STAGE_SLOT_COUNT,
    RX_STAGE_CAPACITY,
    RX_STAGE_SLOT_COUNT,
    RX_REORDER_BACKING_SLOT_COUNT,
>;
pub(super) type ConnectedRxProtocolStorage = ConnectedReceiveStorage<
    'static,
    RX_STAGE_CAPACITY,
    RX_STAGE_SLOT_COUNT,
    RX_REORDER_BACKING_SLOT_COUNT,
>;
type ConnectedRxProtocolStoppedOwner = ConnectedProtocolStopped<
    'static,
    'static,
    RX_STAGE_CAPACITY,
    RX_STAGE_SLOT_COUNT,
    RX_REORDER_BACKING_SLOT_COUNT,
>;
type ConnectedLiveRx =
    ConnectedRx<oer_esp32s31_ieee80211_dma::rx_ring::RxRingLive<'static, RX_DESCRIPTOR_COUNT>>;
type ConnectedStoppedRx = StoppedReceive<
    'static,
    'static,
    'static,
    EmbassyClock,
    CriticalSectionRawMutex,
    RX_STAGE_SLOT_COUNT,
    RX_DESCRIPTOR_COUNT,
    RX_STAGE_CAPACITY,
    RX_STAGE_SLOT_COUNT,
    RX_BUFFER_SIZE,
    { RX_BUFFER_SIZE + 4 },
>;
type ConnectedRx<R> = StagedRxProducer<
    'static,
    'static,
    'static,
    EmbassyClock,
    CriticalSectionRawMutex,
    RX_STAGE_SLOT_COUNT,
    RX_DESCRIPTOR_COUNT,
    RX_STAGE_CAPACITY,
    RX_STAGE_SLOT_COUNT,
    RX_BUFFER_SIZE,
    { RX_BUFFER_SIZE + 4 },
    oer_esp32s31_ieee80211_runtime::datapath::rx::dma::FullRxStageAdmission,
    R,
>;
type ConnectedRxService = ConnectedStaRxService<ConnectedLiveRx, ConnectedRxProtocol>;
type ConnectedParkedRxService =
    ConnectedStaRxParked<ConnectedLiveRx, ConnectedRxProtocolStoppedOwner>;
pub(super) type ProductionAccessPointRxProducer =
    oer_esp32s31_ieee80211_runtime::roles::access_point::AccessPointReceiveProducer<
        'static,
        'static,
        'static,
        EmbassyClock,
        CriticalSectionRawMutex,
        RX_STAGE_SLOT_COUNT,
        RX_DESCRIPTOR_COUNT,
        RX_STAGE_CAPACITY,
        RX_STAGE_SLOT_COUNT,
        RX_BUFFER_SIZE,
        { RX_BUFFER_SIZE + 4 },
    >;
pub(super) type ProductionAccessPointRxConsumer =
    oer_esp32s31_ieee80211_runtime::roles::access_point::AccessPointRxConsumer<
        'static,
        'static,
        CriticalSectionRawMutex,
        RX_STAGE_SLOT_COUNT,
        RX_STAGE_CAPACITY,
        RX_STAGE_SLOT_COUNT,
    >;

pub(super) fn access_point_rx_pipeline(
    ring: ProductionRxRing,
    storage: &'static RxStorage,
    retained: Option<ConnectedRxEpochResources>,
    #[cfg(feature = "diagnostics")] pipeline_observer: Option<
        &'static dyn oer_esp32s31_ieee80211_runtime::diagnostics::rx_pipeline::RxPipelineObserver,
    >,
) -> (
    ProductionAccessPointRxProducer,
    ProductionAccessPointRxConsumer,
) {
    if let Some(retained) = retained {
        #[cfg(feature = "diagnostics")]
        let retained = match pipeline_observer {
            Some(observer) => retained.with_pipeline_observer(observer),
            None => retained,
        };
        return match ring {
            ProductionRxRing::Halted(ring) => oer_esp32s31_ieee80211_runtime::roles::access_point::AccessPointReceiveProducer::from_halted_resources(
                ring,
                retained,
            ),
            ProductionRxRing::Live(ring) => oer_esp32s31_ieee80211_runtime::roles::access_point::AccessPointReceiveProducer::from_live_resources(
                ring,
                retained,
            ),
        };
    }
    #[cfg(feature = "diagnostics")]
    if let Some(observer) = pipeline_observer {
        return match ring {
            ProductionRxRing::Halted(ring) => oer_esp32s31_ieee80211_runtime::roles::access_point::AccessPointReceiveProducer::from_halted_with_pipeline_observer(
                ring,
                storage,
                &RX_STAGE_POOL,
                &STAGED_RX_QUEUE,
                EmbassyClock,
                observer,
            ),
            ProductionRxRing::Live(ring) => oer_esp32s31_ieee80211_runtime::roles::access_point::AccessPointReceiveProducer::from_live_with_pipeline_observer(
                ring,
                storage,
                &RX_STAGE_POOL,
                &STAGED_RX_QUEUE,
                EmbassyClock,
                observer,
            ),
        };
    }
    match ring {
        ProductionRxRing::Halted(ring) => {
            oer_esp32s31_ieee80211_runtime::roles::access_point::AccessPointReceiveProducer::from_halted(
                ring,
                storage,
                &RX_STAGE_POOL,
                &STAGED_RX_QUEUE,
                EmbassyClock,
            )
        }
        ProductionRxRing::Live(ring) => {
            oer_esp32s31_ieee80211_runtime::roles::access_point::AccessPointReceiveProducer::from_live(
                ring,
                storage,
                &RX_STAGE_POOL,
                &STAGED_RX_QUEUE,
                EmbassyClock,
            )
        }
    }
}

#[cfg(feature = "diagnostics")]
fn log_rx_ring_topology(label: &str, rx: &ConnectedLiveRx) {
    let topology = rx.ring().topology_snapshot();
    let reload = rx.ring().reload_repair_evidence();
    let mut armed_mask = [0_u64; 2];
    let mut completed_mask = [0_u64; 2];
    let mut invalid_mask = [0_u64; 2];
    for index in 0..RX_DESCRIPTOR_COUNT {
        let word = index / u64::BITS as usize;
        let bit = 1_u64 << (index % u64::BITS as usize);
        let Some(descriptor) = rx.ring().descriptor_snapshot(index) else {
            invalid_mask[word] |= bit;
            continue;
        };
        if rx_done(descriptor.word0) {
            completed_mask[word] |= bit;
        } else if rx_rearm_word(descriptor.word0) == Some(descriptor.word0) {
            armed_mask[word] |= bit;
        } else {
            invalid_mask[word] |= bit;
        }
    }
    diagnostics_event!(
        "open-radio: connected RX reload label={} observations={} upper_only={} base_repairs={} last_next_low={:#07x} last_last={:?} last_head={:?}",
        label,
        reload.observations,
        reload.unknown_upper_with_zero_address,
        reload.base_repairs,
        reload.last_next_low,
        reload.last_last_low,
        reload.last_repair_head,
    );
    diagnostics_event!(
        "open-radio: connected RX topology label={} valid={} base={:#010x} start={} head={:#010x} head_next={:#010x} tail={} tail_address={:#010x} visited={} terminals={} armed_mask={:016x}:{:016x} completed_mask={:016x}:{:016x} invalid_mask={:016x}:{:016x}",
        label,
        topology.valid,
        topology.descriptor_base,
        topology.start_index,
        topology.head_address,
        topology.head_next_address,
        topology.tail_index,
        topology.tail_address,
        topology.visited_descriptors,
        topology.terminal_descriptors,
        armed_mask[1],
        armed_mask[0],
        completed_mask[1],
        completed_mask[0],
        invalid_mask[1],
        invalid_mask[0],
    );
    diagnostics_event!(
        "open-radio: connected RX buffers label={} detached={} released={}",
        label,
        rx.storage().detached_buffer_count(),
        rx.storage().released_buffer_count(),
    );
    if !topology.valid {
        for index in 0..RX_DESCRIPTOR_COUNT {
            if let Some(descriptor) = rx.ring().descriptor_snapshot(index) {
                diagnostics_event!(
                    "open-radio: connected RX descriptor label={} index={} address={:#010x} word0={:#010x} buffer={:#010x} next={:#010x}",
                    label,
                    descriptor.index,
                    descriptor.address,
                    descriptor.word0,
                    descriptor.buffer_address,
                    descriptor.next_address,
                );
            }
        }
    }
}

pub type ConnectedHardware = CooperativeRadioHardware<'static>;
pub(crate) enum ConnectedParkedRx {
    /// Logical role handoff kept the descriptor walker running.
    Live(ConnectedLiveRx),
    /// Whole-radio shutdown stopped the descriptor walker before RF close.
    Halted(ConnectedStoppedRx),
}

impl ConnectedParkedRx {
    pub(super) const fn from_live(rx: ConnectedLiveRx) -> Self {
        Self::Live(rx)
    }

    pub(super) const fn from_halted(rx: ConnectedStoppedRx) -> Self {
        Self::Halted(rx)
    }

    pub(super) fn into_physical_parts(self) -> (ProductionRxRing, ConnectedRxEpochResources) {
        match self {
            Self::Live(rx) => {
                let (ring, resources) = rx
                    .try_into_live_epoch_parts()
                    .unwrap_or_else(|_| panic!("parked station RX retained a staging lease"));
                (ProductionRxRing::Live(ring), resources)
            }
            Self::Halted(rx) => {
                let (ring, resources) = rx.into_epoch_parts();
                (ProductionRxRing::Halted(ring), resources)
            }
        }
    }
}

impl StoppedStaRx for ConnectedParkedRx {
    type Preconnected = ReceiveFrontier<'static, RX_DESCRIPTOR_COUNT, RX_BUFFER_SIZE>;
    type Persistent = ConnectedRxEpochResources;

    fn split_for_reconnect(self) -> (Self::Preconnected, Self::Persistent) {
        match self {
            Self::Live(rx) => {
                let (ring, resources) = rx
                    .try_into_live_epoch_parts()
                    .unwrap_or_else(|_| panic!("parked station RX retained a staging lease"));
                (ReceiveFrontier::from_live(ring), resources)
            }
            Self::Halted(rx) => {
                let (ring, resources) = rx.into_epoch_parts();
                (ReceiveFrontier::from_halted(ring), resources)
            }
        }
    }
}
pub(super) type ConnectedRxEpochResources = RxEpochResources<
    'static,
    'static,
    'static,
    EmbassyClock,
    CriticalSectionRawMutex,
    RX_STAGE_SLOT_COUNT,
    RX_DESCRIPTOR_COUNT,
    RX_STAGE_CAPACITY,
    RX_STAGE_SLOT_COUNT,
    RX_BUFFER_SIZE,
    { RX_BUFFER_SIZE + 4 },
>;
type ConnectedLiveTx = ConnectedTx<
    'static,
    'static,
    oer_esp32s31_ieee80211_runtime::datapath::PinnedTxFrame<
        'static,
        CriticalSectionRawMutex,
        NETWORK_FRAME_CAPACITY,
        NETWORK_TX_HEADROOM,
        NETWORK_TX_TRAILER,
        NETWORK_TX_QUEUE_DEPTH,
    >,
    oer_esp32s31_phy::PhyTxTargetPowerProfile,
    fn() -> u32,
    super::MacTimer,
    TX_AMPDU_FRAME_COUNT,
    TX_AMPDU_BUFFER_SIZE,
    { crate::resources::profile::ESP32S31_DEFAULT_TX_BUFFER_SIZE },
>;
type ConnectedDriverServices<R = ConnectedLiveRx, H = ConnectedHardware> = ConnectionServices<
    'static,
    CriticalSectionRawMutex,
    H,
    R,
    ConnectedRxProtocol,
    ConnectedLiveTx,
    CONTROL_QUEUE_DEPTH,
>;
type ConnectedSoftwareTxFrame = <NetworkRunner as DatapathNetwork>::TxFrame;
type ConnectedPhysicalTxFrame = <NetworkRunner as DatapathNetwork>::PhysicalTxFrame;
type ConnectedDatapathError = <ConnectedDriverServices as DatapathServices<
    ConnectedSoftwareTxFrame,
    ConnectedPhysicalTxFrame,
>>::Error;
type ConnectedDatapathRunner<B = ConnectedDriverServices> = DatapathRunner<
    'static,
    CriticalSectionRawMutex,
    NetworkRunner,
    B,
    NetworkRxPublisher,
    EmbassyClock,
>;

type ConnectedServicesMapper = fn(ConnectedDriverServices) -> ConnectedDriverServices;
type ConnectedProtocolAssemblyResources = ConnectedStaRxProtocolResources<
    'static,
    'static,
    'static,
    'static,
    CriticalSectionRawMutex,
    ConnectedRxSink,
    EmbassyClock,
    RX_STAGE_SLOT_COUNT,
    RX_STAGE_CAPACITY,
    RX_STAGE_SLOT_COUNT,
    RX_REORDER_BACKING_SLOT_COUNT,
>;
type ConnectedControlAssemblyResources =
    ConnectedStaControlResources<'static, CriticalSectionRawMutex, CONTROL_QUEUE_DEPTH>;
type ConnectedTxAssemblyFailure = ConnectedStaTxHandoffFailure<
    'static,
    'static,
    RadioTxBacking,
    oer_esp32s31_phy::PhyTxTargetPowerProfile,
    fn() -> u32,
    super::MacTimer,
    TX_AMPDU_FRAME_COUNT,
    TX_AMPDU_BUFFER_SIZE,
    { crate::resources::profile::ESP32S31_DEFAULT_TX_BUFFER_SIZE },
>;
type ConnectedAssemblyComposition = ConnectedStaCompositionFailure<
    ConnectedHardware,
    ConnectedLiveRx,
    ConnectedProtocolAssemblyResources,
    ConnectedControlAssemblyResources,
    ConnectedTxAssemblyFailure,
>;
type ConnectedAssemblyFailure =
    oer_esp32s31_ieee80211_runtime::roles::station::connected::ConnectedDriverAssemblyFailure<
        NetworkRunner,
        ConnectedAssemblyComposition,
        ConnectedServicesMapper,
    >;
type ConnectedDriverStarted = ConnectedEpochStarted<
    ConnectedHardware,
    ConnectedLiveRx,
    RadioAmpduStorage,
    ConnectedControlStatics,
>;
type ReturnedConnectedTxResources = oer_esp32s31_ieee80211_sta::control_tx::WifiTxResources<
    'static,
    oer_esp32s31_phy::PhyTxTargetPowerProfile,
    fn() -> u32,
    super::MacTimer,
    { crate::resources::profile::ESP32S31_DEFAULT_TX_BUFFER_SIZE },
>;
type ConnectedDriverTeardownFailure = ConnectionTeardownFailure<
    'static,
    CriticalSectionRawMutex,
    ConnectedHardware,
    ConnectedRxService,
    ConnectedParkedRxService,
    ConnectedLiveTx,
    CONTROL_QUEUE_DEPTH,
    RxRingError,
>;
// A failed connected-driver teardown is a terminal quarantine frontier: the
// supervisor retains it forever and can never begin another role epoch. Keep
// that complete no-alloc owner graph at a stable address so wrapping the fault
// for the terminal actor does not copy several kilobytes through each enum
// layer on the task stack.
static CONNECTED_DRIVER_TEARDOWN_FAULT: StaticCell<ConnectedDriverTeardownFailure> =
    StaticCell::new();
pub type ConnectedReconnectedEpoch = ReconnectedStaEpoch<
    ConnectedHardware,
    ReceiveFrontier<'static, RX_DESCRIPTOR_COUNT, RX_BUFFER_SIZE>,
    ConnectedRxEpochResources,
    RadioAmpduStorage,
    ConnectedControlStatics,
>;
pub type ConnectedDisconnectedEpoch = DisconnectedStaEpoch<
    RunningStationNetwork<(), NetworkRunner>,
    ConnectedHardware,
    ConnectedParkedRx,
    RadioAmpduStorage,
    ConnectedControlStatics,
>;
// This SRAM object stores only the 32 affine handoff records. Each admitted
// frame retains its original buffer from the 96-entry DMA ring. The bounded
// handoff therefore leaves at least 64 descriptor credits with the hardware.
oer_memory::zeroed_static! {
    /// Placement: latency-critical RX staging stays in internal SRAM.
    pub(super) static RX_STAGE_POOL: RxStagePool<RX_STAGE_SLOT_COUNT, RX_STAGE_CAPACITY> =
        zeroed in ".critical.bss.open_radio_rx_stage";
}

/// Handoff slots kept from zero-copy network ownership: one BlockAck-16
/// window of staging credits, so a complete in-flight A-MPDU burst still
/// stages while the network holds the rest. The critical control/management
/// credit is separate, inside the staging admission's `CreditPolicy`.
#[cfg(feature = "owned-network")]
const RX_ZERO_COPY_RESERVE: usize = RX_REORDER_WINDOW;

#[cfg(feature = "owned-network")]
const _: () = assert!(
    RX_STAGE_CAPACITY == oer_esp32s31_ieee80211_mac::rx::pool::VENDOR_LARGE_RX_PAYLOAD_CAPACITY
        && RX_STAGE_SLOT_COUNT == oer_esp32s31_ieee80211_mac::rx::pool::VENDOR_LARGE_RX_SLOT_COUNT,
    "the zero-copy origin adopts the vendor large RX staging profile"
);

/// Zero-copy origin of [`RX_STAGE_POOL`], shared by the station and AP
/// endpoints. Its atomics are touched per received frame, so it stays in
/// internal SRAM.
#[cfg(feature = "owned-network")]
#[allow(
    unsafe_code,
    reason = "the linker must retain per-frame zero-copy accounting in internal SRAM"
)]
#[unsafe(link_section = ".critical.data.open_radio_rx_zero_copy")]
pub(super) static RX_ZERO_COPY_ORIGIN:
    oer_esp32s31_ieee80211_runtime::datapath::owned::RxZeroCopyOrigin =
    oer_esp32s31_ieee80211_runtime::datapath::owned::RxZeroCopyOrigin::new(
        &RX_ZERO_COPY_ORIGIN,
        RX_STAGE_POOL.external_handoff_pool(),
        RX_ZERO_COPY_RESERVE,
    );
pub(super) static STAGED_RX_QUEUE: StagedRxQueue<
    'static,
    CriticalSectionRawMutex,
    RX_STAGE_SLOT_COUNT,
    RX_STAGE_CAPACITY,
    RX_STAGE_SLOT_COUNT,
> = StagedRxQueue::new();
pub(super) static STA_AP_STAGED_RX_QUEUE:
    oer_esp32s31_ieee80211_runtime::datapath::rx::routed::StaApStagedRxQueue<
        'static,
        CriticalSectionRawMutex,
        RX_STAGE_SLOT_COUNT,
        RX_STAGE_CAPACITY,
        RX_STAGE_SLOT_COUNT,
    > = oer_esp32s31_ieee80211_runtime::datapath::rx::routed::StaApStagedRxQueue::new();
/// The connected station's link to its power agent.
static STATION_POWER_LINK: StationPowerLink<CriticalSectionRawMutex> = StationPowerLink::new();

pub(super) static RX_REORDER_COMMANDS: RxReorderCommandResources<CriticalSectionRawMutex> =
    RxReorderCommandResources::new();
pub(super) static RX_REORDER_STORAGE: RxReorderFrameStorage<
    RX_STAGE_CAPACITY,
    RX_REORDER_BACKING_SLOT_COUNT,
> = RxReorderFrameStorage::new();
// Reorder machines and retained RX lease tokens are long-lived protocol
// state. Keeping their arena static prevents this multi-kilobyte owner table
// from being copied through the connected async state machine's stack frame.
static RX_PROTOCOL_RUNTIME: ConstStaticCell<ConnectedRxProtocolStorage> =
    ConstStaticCell::new(ConnectedReceiveStorage::new());
static CONTROL_RESOURCES: ConstStaticCell<ControlResources> =
    ConstStaticCell::new(ControlResources::new());
/// The slot each association's connected control is placed in.
type ProductionControlSlot =
    oer_esp32s31_ieee80211_runtime::roles::station::control_slot::ConnectedControlStorage<
        'static,
        CriticalSectionRawMutex,
        CONTROL_QUEUE_DEPTH,
    >;
// The connected control (BlockAck sessions, power and link-monitor state)
// lives here for the whole association: the connected services, their
// teardown and every fault hold a pointer to it, so its size is not copied
// through the connected epoch's poll frame or its teardown. It is
// task-context state, not interrupt-shared: it stays in the PSRAM tier where
// the task stacks and futures that held it by value lived, at no SRAM cost.
//
// A `.bss` section starts zeroed, and an empty slot is not all-zero bytes
// (its `None` is a niche value): a const-initialized cell would start as a
// "placed" control of zeros, which the first placement drops. The cell is
// uninitialized storage instead, and the one-time claim writes the empty
// slot.
oer_memory::zeroed_static! {
    /// Placement: the connected control slot is in the PSRAM ownership tier.
    static CONNECTED_CONTROL_SLOT: oer_memory::zeroed::ZeroedStatic<
        core::mem::MaybeUninit<ProductionControlSlot>,
    > = zeroed in ".psram.bss.open_radio_station_connected_control";
}

/// The connected control's statics: its event queue and the slot each
/// association's control is placed in. Before a control is placed, and once
/// teardown released it, both travel together; meanwhile the slot is lent to
/// the connected services.
pub(crate) struct ConnectedControlStatics {
    pub(super) queue: &'static ControlResources,
    pub(super) slot: &'static mut ProductionControlSlot,
}
pub(super) static STA_CCMP_RX_REPLAY: StaCcmpRxReplayResource = StaCcmpRxReplayResource::new();
#[cfg(feature = "diagnostics")]
static DIAGNOSTIC_LINK_BANDWIDTH_MHZ: AtomicU32 = AtomicU32::new(0);
#[cfg(feature = "diagnostics")]
static DIAGNOSTIC_DATA_RATE_KBPS: AtomicU32 = AtomicU32::new(0);
#[cfg(feature = "diagnostics")]
static DIAGNOSTIC_AGGREGATE_RATE_KBPS: AtomicU32 = AtomicU32::new(0);
static REGISTER_ARENA: ConstStaticCell<RadioOwnerArena> =
    ConstStaticCell::new(RadioOwnerArena::new());
// Ethernet scratch belongs to the driver datapath. Application socket buffers
// are deliberately not allocated by this product integration.
static ETHERNET_FRAME: ConstStaticCell<[u8; RX_STAGE_CAPACITY]> =
    ConstStaticCell::new([0; RX_STAGE_CAPACITY]);
// One station protocol turn may close a reorder gap and synchronously release
// the current MPDU plus a complete negotiated BlockAck window. Each released
// MPDU can itself be an A-MSDU, and the deferred publication image adds one
// two-byte record prefix per decoded Ethernet subframe. This PSRAM-owned arena
// therefore covers the full typed release frontier rather than only one DMA
// unit; network backpressure can retain that finite batch without poisoning
// the affine paired owner graph with a synthetic `BatchCapacity` fault.
const STA_AP_STATION_RX_BATCH_CAPACITY: usize = (RX_REORDER_WINDOW + 1) * RX_STAGE_CAPACITY * 2;
static STA_AP_STATION_RX_BATCH: ConstStaticCell<[u8; STA_AP_STATION_RX_BATCH_CAPACITY]> =
    ConstStaticCell::new([0; STA_AP_STATION_RX_BATCH_CAPACITY]);

/// Hardware frontier accepted by one connected epoch.
pub type ConnectedStationEpoch = ConnectedEpochResources<
    RadioRuntimeOwner,
    ReceiveFrontier<'static, RX_DESCRIPTOR_COUNT, RX_BUFFER_SIZE>,
    ConnectedReconnectedEpoch,
>;
type ConnectedRxFrontier = ReceiveFrontier<'static, RX_DESCRIPTOR_COUNT, RX_BUFFER_SIZE>;
type InitialConnectedResources = InitialConnectedEpochResources<
    'static,
    ConnectedRxEpochResources,
    RadioAmpduStorage,
    ConnectedControlStatics,
>;
type ConnectedEpochStartFault = ConnectedEpochStartFailure<
    InitialConnectedResources,
    ConnectedHardware,
    ConnectedRxFrontier,
    ConnectedRxEpochResources,
    RadioAmpduStorage,
    ConnectedControlStatics,
    RxFrontierError,
>;

/// Initial-only connected resources materialized before the supervisor can
/// activate any station IRQ or DMA epoch.
pub(crate) struct InitialConnectedStaticResources {
    registers: &'static RadioOwnerArena,
    aggregate: Option<RadioAmpduStorage>,
    control: ConnectedControlStatics,
}

/// Read-only, value-returning diagnostics view of the connected register
/// arena. It cannot mutate registers, outlive a synchronous borrow, or keep
/// the PAC owner published during role shutdown.
#[cfg(feature = "diagnostics")]
#[derive(Clone, Copy)]
pub struct DiagnosticSnapshot {
    registers: &'static RadioOwnerArena,
}

#[cfg(feature = "diagnostics")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DiagnosticTxVector {
    pub bandwidth_mhz: u16,
    pub data_rate_kbps: u32,
    pub aggregate_rate_kbps: u32,
}

/// Application-safe subset of hardware RX counters used by diagnostics.
/// No register block or PAC capability crosses this boundary.
#[cfg(feature = "diagnostics")]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DiagnosticRxStatistics {
    pub mpdu_count: u16,
    pub data_success: u16,
    pub fcs_error: u16,
    pub abort: u16,
    pub abort_fcs_pass: u16,
    pub power_drop_error: u16,
    pub he_sig_b_error: u16,
    pub same_bm_error: u16,
    pub signal_field: u16,
    pub end: u16,
    pub other_unicast: u16,
    pub buffer_full: u16,
    pub fifo_overflow: u16,
    pub tkip_error: u16,
    pub bt_block_error: u16,
    pub frequency_hop_error: u16,
    pub last_unmatched_error: u16,
    pub ack_interrupt: u16,
    pub rts_interrupt: u16,
    pub brx_agc_error: u16,
    pub brx_error: u16,
    pub nrx_error: u16,
    pub nrx_abort: u16,
    pub nrx_agc_exit: u16,
    pub nrx_baseband_off: u16,
    pub nrx_fdm_watchdog: u16,
    pub nrx_restart: u16,
    pub nrx_service: u16,
    pub nrx_tx_over: u16,
    pub nrx_unsupported: u16,
    pub nrx_he_format: u16,
    pub nrx_ht_sig: u16,
    pub nrx_he_unsupported: u16,
    pub nrx_he_sig_a_crc: u16,
    pub rx_hang: u8,
    pub tx_hang: u8,
    pub rx_tx_hang: u32,
    pub rx_tx_panic: u32,
}

#[cfg(feature = "diagnostics")]
impl DiagnosticRxStatistics {
    pub(crate) fn from_deltas(
        primary: oer_esp32s31_hal::ieee80211::mac::MacRxPrimaryStatisticsDelta,
        decode: oer_esp32s31_hal::ieee80211::mac::MacRxDecodeErrorStatisticsDelta,
        hang: oer_esp32s31_hal::ieee80211::mac::MacRxHangStatisticsDelta,
    ) -> Self {
        Self {
            mpdu_count: primary.mpdu_count,
            data_success: primary.data_success,
            fcs_error: primary.fcs_error,
            abort: primary.abort,
            abort_fcs_pass: primary.abort_fcs_pass,
            power_drop_error: primary.power_drop_error,
            he_sig_b_error: primary.he_sig_b_error,
            same_bm_error: primary.same_bm_error,
            signal_field: primary.signal_field,
            end: primary.end,
            other_unicast: primary.other_unicast,
            buffer_full: primary.buffer_full,
            fifo_overflow: primary.fifo_overflow,
            tkip_error: primary.tkip_error,
            bt_block_error: primary.bt_block_error,
            frequency_hop_error: primary.frequency_hop_error,
            last_unmatched_error: primary.last_unmatched_error,
            ack_interrupt: primary.ack_interrupt,
            rts_interrupt: primary.rts_interrupt,
            brx_agc_error: decode.brx_agc,
            brx_error: decode.brx,
            nrx_error: decode.nrx,
            nrx_abort: decode.nrx_abort,
            nrx_agc_exit: decode.nrx_agc_exit,
            nrx_baseband_off: decode.nrx_baseband_off,
            nrx_fdm_watchdog: decode.nrx_fdm_watchdog,
            nrx_restart: decode.nrx_restart,
            nrx_service: decode.nrx_service,
            nrx_tx_over: decode.nrx_tx_over,
            nrx_unsupported: decode.nrx_unsupported,
            nrx_he_format: decode.nrx_he_format,
            nrx_ht_sig: decode.nrx_ht_sig,
            nrx_he_unsupported: decode.nrx_he_unsupported,
            nrx_he_sig_a_crc: decode.nrx_he_sig_a_crc,
            rx_hang: hang.rx,
            tx_hang: hang.tx,
            rx_tx_hang: hang.rx_tx_hang,
            rx_tx_panic: hang.rx_tx_panic,
        }
    }
}

#[cfg(feature = "diagnostics")]
pub(crate) struct ObservedConnectedRxSink<S> {
    inner: S,
    observer: &'static dyn ConnectedRxObserver,
}

#[cfg(feature = "diagnostics")]
impl<S: MacConnectedRxSink> MacConnectedRxSink for ObservedConnectedRxSink<S> {
    fn wants_power_save_data(&self) -> bool {
        self.inner.wants_power_save_data()
    }

    fn publish(&mut self, event: ConnectedRxEvent<'_>) {
        let include_phy = match event {
            ConnectedRxEvent::Ethernet { frame, .. } => self.observer.requests_phy(frame.into()),
            _ => false,
        };
        self.observer
            .observe(ConnectedRxObservation::decode(event, include_phy));
        self.inner.publish(event);
    }
}

#[cfg(feature = "diagnostics")]
impl<S, const CAPACITY: usize, const SLOTS: usize> ConnectedRxProtocolSink<CAPACITY, SLOTS>
    for ObservedConnectedRxSink<S>
where
    S: ConnectedRxProtocolSink<CAPACITY, SLOTS>,
{
    fn staged_rx_admission(
        &self,
    ) -> oer_esp32s31_ieee80211_runtime::roles::station::rx_protocol::StagedRxAdmission {
        self.inner.staged_rx_admission()
    }

    fn wait_ready(&mut self) -> impl core::future::Future<Output = ()> + '_ {
        self.inner.wait_ready()
    }

    fn wait_staged_ready(&mut self) -> impl core::future::Future<Output = ()> + '_ {
        self.inner.wait_staged_ready()
    }

    fn publish_staged(
        &mut self,
        frame: oer_esp32s31_ieee80211_runtime::datapath::rx::staging::StagedRxFrame<
            '_,
            CAPACITY,
            SLOTS,
        >,
        ethernet: oer_esp32s31_ieee80211_runtime::datapath::rx::staging::StagedEthernetPublication,
    ) -> oer_esp32s31_ieee80211_runtime::datapath::rx::staging::StagedRxDisposition {
        {
            let raw = frame.segment().buffer;
            let payload =
                &raw[ethernet.payload_offset..ethernet.payload_offset + ethernet.payload_length];
            let event = ConnectedRxEvent::Ethernet {
                frame: oer_ieee80211_mac::data::EthernetFrameParts {
                    destination: ethernet.destination,
                    source: ethernet.source,
                    ether_type: ethernet.ether_type,
                    payload,
                },
                raw,
                amsdu: false,
                metadata: ethernet.metadata,
            };
            let include_phy = self.observer.requests_phy(event_frame(event).into());
            self.observer
                .observe(ConnectedRxObservation::decode(event, include_phy));
        }
        self.inner.publish_staged(frame, ethernet)
    }
}

#[cfg(feature = "diagnostics")]
fn event_frame(event: ConnectedRxEvent<'_>) -> oer_ieee80211_mac::data::EthernetFrameParts<'_> {
    match event {
        ConnectedRxEvent::Ethernet { frame, .. } => frame,
        _ => unreachable!("the staged observation is always an Ethernet event"),
    }
}

#[cfg(feature = "diagnostics")]
impl DiagnosticSnapshot {
    /// Snapshot the associated-STA receive filters and BSSID identity while
    /// the connected epoch owns the register arena.
    pub fn sta_receive_policy(
        self,
    ) -> Option<oer_esp32s31_hal::ieee80211::mac::MacStaReceivePolicySnapshot> {
        self.registers.try_station_receive_policy_snapshot().ok()
    }

    /// Snapshot hardware RX counters only while a connected epoch owns the
    /// register arena. `None` means the role is stopped/transitioning or a
    /// driver transaction currently has the bounded borrow.
    pub fn rx_statistics(self) -> Option<DiagnosticRxStatistics> {
        let statistics = self.registers.try_receive_statistics_snapshot().ok()?;
        let primary = statistics.primary;
        let decode = statistics.decode_errors;
        let hang = statistics.hang;
        Some(DiagnosticRxStatistics {
            mpdu_count: primary.mpdu_count,
            data_success: primary.data_success,
            fcs_error: primary.fcs_error,
            abort: primary.abort,
            abort_fcs_pass: primary.abort_fcs_pass,
            power_drop_error: primary.power_drop_error,
            he_sig_b_error: primary.he_sig_b_error,
            same_bm_error: primary.same_bm_error,
            signal_field: primary.signal_field,
            end: primary.end,
            other_unicast: primary.other_unicast,
            buffer_full: primary.buffer_full,
            fifo_overflow: primary.fifo_overflow,
            tkip_error: primary.tkip_error,
            bt_block_error: primary.bt_block_error,
            frequency_hop_error: primary.frequency_hop_error,
            last_unmatched_error: primary.last_unmatched_error,
            ack_interrupt: primary.ack_interrupt,
            rts_interrupt: primary.rts_interrupt,
            brx_agc_error: decode.brx_agc,
            brx_error: decode.brx,
            nrx_error: decode.nrx,
            nrx_abort: decode.nrx_abort,
            nrx_agc_exit: decode.nrx_agc_exit,
            nrx_baseband_off: decode.nrx_baseband_off,
            nrx_fdm_watchdog: decode.nrx_fdm_watchdog,
            nrx_restart: decode.nrx_restart,
            nrx_service: decode.nrx_service,
            nrx_tx_over: decode.nrx_tx_over,
            nrx_unsupported: decode.nrx_unsupported,
            nrx_he_format: decode.nrx_he_format,
            nrx_ht_sig: decode.nrx_ht_sig,
            nrx_he_unsupported: decode.nrx_he_unsupported,
            nrx_he_sig_a_crc: decode.nrx_he_sig_a_crc,
            rx_hang: hang.rx,
            tx_hang: hang.tx,
            rx_tx_hang: hang.rx_tx_hang,
            rx_tx_panic: hang.rx_tx_panic,
        })
    }

    /// Wrapping count of real `RX_SUCCESS` publications from the shared hard
    /// interrupt path. Handoff/capacity wakes are deliberately excluded.
    pub fn rx_interrupt_posts(self) -> u32 {
        IRQ_RUNTIME.rx_post_count()
    }

    /// Whether the PAC observed a MAC cause not owned by the dispatcher.
    pub fn had_unhandled_interrupt(self) -> bool {
        IRQ_RUNTIME.observed_unhandled()
    }

    /// Current associated-link TX vector. `None` means no connected epoch
    /// owns the datapath.
    pub fn tx_vector(self) -> Option<DiagnosticTxVector> {
        let bandwidth_mhz = DIAGNOSTIC_LINK_BANDWIDTH_MHZ.load(Ordering::Acquire);
        (bandwidth_mhz != 0).then(|| DiagnosticTxVector {
            bandwidth_mhz: bandwidth_mhz as u16,
            data_rate_kbps: DIAGNOSTIC_DATA_RATE_KBPS.load(Ordering::Relaxed),
            aggregate_rate_kbps: DIAGNOSTIC_AGGREGATE_RATE_KBPS.load(Ordering::Relaxed),
        })
    }
}

#[cfg(feature = "diagnostics")]
fn log_sta_receive_policy(
    edge: &str,
    policy: oer_esp32s31_hal::ieee80211::mac::MacStaReceivePolicySnapshot,
) {
    let bssid = policy.bssid;
    diagnostics_event!(
        "open-radio: sta-rx-policy edge={} bssid={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} aid={} spacing={} check={} ap={} rx={} beacon={:02x}",
        edge,
        bssid[0],
        bssid[1],
        bssid[2],
        bssid[3],
        bssid[4],
        bssid[5],
        policy.association_id,
        policy.minimum_mpdu_start_spacing,
        policy.bssid_address_check_enabled,
        policy.interface_is_soft_ap,
        policy.interface_rx_policy_enabled,
        policy.beacon_filter_control,
    );
}

pub(super) struct InitialConnectedInitialization {
    pub(super) resources: InitialConnectedStaticResources,
    #[cfg(feature = "diagnostics")]
    pub(super) diagnostics: DiagnosticSnapshot,
}

impl InitialConnectedStaticResources {
    pub(super) fn take_aggregate(&mut self) -> RadioAmpduStorage {
        self.aggregate
            .take()
            .expect("one STA or AP role exclusively owns the aggregate arena")
    }

    pub(super) fn restore_aggregate(&mut self, aggregate: RadioAmpduStorage) {
        assert!(
            self.aggregate.replace(aggregate).is_none(),
            "aggregate arena cannot be restored over a live role owner"
        );
    }

    pub(super) fn with_rx(
        mut self,
        rx: ConnectedRxEpochResources,
    ) -> InitialConnectedEpochResources<
        'static,
        ConnectedRxEpochResources,
        RadioAmpduStorage,
        ConnectedControlStatics,
    > {
        InitialConnectedEpochResources::new(self.registers, rx, self.take_aggregate(), self.control)
    }
}

pub(super) type ConnectedNetworkStarted<'state, 'security> = StartedNetworkConnection<
    'security,
    ProductionStationRuntime<'state>,
    ConnectedStationEpoch,
    (),
    NetworkRunner,
    (),
>;

/// Normal outcome returned after all connected owners are quiescent.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectedStationOutcome {
    Disconnected(oer_esp32s31_ieee80211_sta::connected_control::ConnectedDisconnectReason),
    ReconnectRequested,
    StationStopped(StationCommand),
    HardwareFailure,
}

/// Exact input for the next finite join attempt.
pub struct ConnectedStationReturn<'state, 'security> {
    pub disconnected: ConnectedDisconnectedEpoch,
    pub runtime: ProductionStationRuntime<'state>,
    pub security: StaAttemptSecurity<'security>,
    pub outcome: ConnectedStationOutcome,
}

pub(crate) struct ConnectedDriverAssemblyFault {
    _role:
        oer_esp32s31_ieee80211::runtime::WifiRoleOwner<EspHalWifiPlatform, super::WifiMembership>,
    _interrupt: MacInterruptEpoch,
    _dma: oer_esp32s31_ieee80211_runtime::roles::station::StationDmaResources<
        'static,
        RxStorage,
        RX_DESCRIPTOR_COUNT,
    >,
    _tx_storage: &'static mut TxStorage,
    _scan_table: &'static mut oer_ieee80211_mac::scan::ScanTable,
    _stack: (),
    _initial_network_task: Option<()>,
    _control_resources: &'static ControlResources,
    _group_security: ConnectedStaGroupSecurity,
    _material: StaAttemptSecurityMaterial,
    _interface: oer_ieee80211_softmac::interface::BoundVirtualInterface,
    _failure: ConnectedAssemblyFailure,
}

/// Exact WPA2 owners retained when the shared replay arena or connected plan
/// rejects publication before driver composition.
pub(crate) enum ConnectedStationReplaySetupFailure {
    Start {
        _failure: StaCcmpRxReplayStartFailure,
        _pairwise: StaPairwiseCcmpSlot,
        _group: StaGroupCcmpSlot,
        _group_material: StaGroupCcmpKeyMaterial,
    },
    Plan {
        _failure: ConnectedStaCcmpReplayFailure,
        _tx_security: ConnectedTxSecurity,
        _group_security: ConnectedStaGroupSecurity,
    },
}

/// Non-reusable connected owner retained at the exact failed transition.
/// No variant exposes the ordinary disconnected owner required for retry.
pub enum ConnectedStationFault<'state, 'security> {
    /// The association ended with the station's RF asleep.
    RfAsleep {
        _returned: ConnectedStationReturn<'state, 'security>,
    },
    InvalidConnectedPolicy {
        _resources: ConnectedStationResources<'state, 'security>,
        _error: ConnectedStaConfigError,
    },
    DriverAssembly {
        _fault: ConnectedDriverAssemblyFault,
    },
    OrdinaryTxUnavailable {
        _runtime: ProductionStationRuntime<'state>,
        _started: ConnectedDriverStarted,
        _stack: (),
        _network: NetworkRunner,
        _initial_network_task: Option<()>,
        _plan: oer_esp32s31_ieee80211_runtime::roles::station::connected::ConnectedStaPlan,
        _installed_security: StaInstalledSecurity,
        _security: StaAttemptSecurity<'security>,
        _error: oer_esp32s31_ieee80211_sta::tx_epoch::StaTxEpochError,
    },
    SecurityOwnershipMismatch {
        _runtime: ProductionStationRuntime<'state>,
        _started: ConnectedDriverStarted,
        _stack: (),
        _network: NetworkRunner,
        _initial_network_task: Option<()>,
        _plan: oer_esp32s31_ieee80211_runtime::roles::station::connected::ConnectedStaPlan,
        _installed_security: StaInstalledSecurity,
        _sequences: StaTxSequenceCounters,
        _material: StaAttemptSecurityMaterial,
        _control_tx: ControlTx,
    },
    ReplaySetup {
        _runtime: ProductionStationRuntime<'state>,
        _started: ConnectedDriverStarted,
        _stack: (),
        _network: NetworkRunner,
        _initial_network_task: Option<()>,
        _plan: oer_esp32s31_ieee80211_runtime::roles::station::connected::ConnectedStaPlan,
        _failure: ConnectedStationReplaySetupFailure,
        _sequences: StaTxSequenceCounters,
        _material: StaAttemptSecurityMaterial,
        _control_tx: ControlTx,
    },
    InitialStaticResourcesUnavailable {
        _runtime: ProductionStationRuntime<'state>,
        _hardware: RadioRuntimeOwner,
        _receive: ConnectedRxFrontier,
        _stack: (),
        _network: NetworkRunner,
        _initial_network_task: Option<()>,
        _plan: oer_esp32s31_ieee80211_runtime::roles::station::connected::ConnectedStaPlan,
        _installed_security: StaInstalledSecurity,
        _security: StaAttemptSecurity<'security>,
    },
    InterruptActivation {
        _started: ConnectedNetworkStarted<'state, 'security>,
    },
    EpochStart {
        _runtime: ProductionStationRuntime<'state>,
        _failure: ConnectedEpochStartFault,
        _stack: (),
        _network: NetworkRunner,
        _initial_network_task: Option<()>,
        _plan: oer_esp32s31_ieee80211_runtime::roles::station::connected::ConnectedStaPlan,
        _installed_security: StaInstalledSecurity,
        _security: StaAttemptSecurity<'security>,
    },
    DriverTeardown {
        _role: oer_esp32s31_ieee80211::runtime::WifiRoleOwner<
            EspHalWifiPlatform,
            super::WifiMembership,
        >,
        _interrupt: MacInterruptEpoch,
        _dma: oer_esp32s31_ieee80211_runtime::roles::station::StationDmaResources<
            'static,
            RxStorage,
            RX_DESCRIPTOR_COUNT,
        >,
        _tx_storage: &'state mut TxStorage,
        _scan_table: &'state mut oer_ieee80211_mac::scan::ScanTable,
        _interface: oer_ieee80211_softmac::interface::BoundVirtualInterface,
        _sta_ap_rx_batch: &'static mut [u8],
        _initial_connected: Option<InitialConnectedStaticResources>,
        #[cfg(feature = "diagnostics")]
        _diagnostics: Option<crate::DiagnosticObservers>,
        _network: RunningWifiNetwork,
        _control_resources: &'static ControlResources,
        _outcome: ConnectedStationOutcome,
        _interrupt_drain: oer_esp32s31_ieee80211_runtime::datapath::irq::MacInterruptEpochDrain,
        _error: &'static mut ConnectedDriverTeardownFailure,
        _material: StaAttemptSecurityMaterial,
    },
    SecurityTeardownMismatch {
        _runtime: ProductionStationRuntime<'state>,
        _network: RunningWifiNetwork,
        _control_resources: ConnectedControlStatics,
        _outcome: ConnectedStationOutcome,
        _interrupt_drain: oer_esp32s31_ieee80211_runtime::datapath::irq::MacInterruptEpochDrain,
        _hardware: ConnectedHardware,
        _parked_rx: ConnectedParkedRx,
        _tx_resources: ReturnedConnectedTxResources,
        _aggregate: RadioAmpduStorage,
        _control_observation: Esp32s31ConnectedControlShutdown,
        _security_stop: ConnectedStaSecurityStopReport,
        _sequences: StaTxSequenceCounters,
        _material: StaAttemptSecurityMaterial,
    },
    TxRestore {
        _runtime: ProductionStationRuntime<'state>,
        _network: RunningWifiNetwork,
        _control_resources: ConnectedControlStatics,
        _outcome: ConnectedStationOutcome,
        _interrupt_drain: oer_esp32s31_ieee80211_runtime::datapath::irq::MacInterruptEpochDrain,
        _hardware: ConnectedHardware,
        _parked_rx: ConnectedParkedRx,
        _aggregate: RadioAmpduStorage,
        _control_observation: Esp32s31ConnectedControlShutdown,
        _security_stop: ConnectedStaSecurityStopReport,
        _sequences: StaTxSequenceCounters,
        _error: oer_esp32s31_ieee80211_sta::tx_epoch::StaTxEpochError,
        _returned_control: ControlTx,
        _material: StaAttemptSecurityMaterial,
    },
}

/// The one map from the ESP32-S31 connected faults to the portable reason
/// the application and HIL see; a new variant does not build until it is
/// classified here.
impl StaFault for ConnectedStationFault<'_, '_> {
    fn reason(&self) -> StaFaultReason {
        use StaFaultCause::{Configuration, Hardware, Resources, RfState, Security};
        use StaFaultPhase::{Start, Teardown};
        let (phase, cause) = match self {
            Self::InvalidConnectedPolicy { .. } => (Start, Configuration),
            Self::InterruptActivation { .. } | Self::InitialStaticResourcesUnavailable { .. } => {
                (Start, Resources)
            }
            Self::EpochStart { .. }
            | Self::OrdinaryTxUnavailable { .. }
            | Self::DriverAssembly { .. } => (Start, Hardware),
            Self::ReplaySetup { .. } | Self::SecurityOwnershipMismatch { .. } => (Start, Security),
            Self::DriverTeardown { .. } | Self::TxRestore { .. } => (Teardown, Hardware),
            Self::SecurityTeardownMismatch { .. } => (Teardown, Security),
            Self::RfAsleep { .. } => (Teardown, RfState),
        };
        StaFaultReason::new(phase, cause)
    }
}

pub enum ConnectedStationRunExit<'state, 'security> {
    Returned(ConnectedStationReturn<'state, 'security>),
    Faulted(ConnectedStationFault<'state, 'security>),
}

/// Exact owners transferred from a successful finite station attempt.
pub type ConnectedStationResources<'state, 'security> = ConnectedServiceResources<
    'security,
    ProductionStationRuntime<'state>,
    ConnectedStationEpoch,
    WifiNetworkResources,
>;

pub(super) fn initialize_ethernet_frame() -> &'static mut [u8] {
    ETHERNET_FRAME.take().as_mut_slice()
}

pub(super) fn initialize_sta_ap_station_rx_batch() -> &'static mut [u8] {
    STA_AP_STATION_RX_BATCH.take().as_mut_slice()
}

/// Materialize the large connected-RX arena once; every later connected
/// epoch receives and returns this exact lease through the station owner.
pub(super) fn initialize_connected_rx_protocol_runtime() -> &'static mut ConnectedRxProtocolStorage
{
    RX_PROTOCOL_RUNTIME.take()
}

/// Bind the one-time A-MPDU descriptor arena before the radio supervisor can
/// enter an active station epoch.
pub(super) fn initialize_connected_static_resources()
-> Result<InitialConnectedInitialization, HtAmpduTxError> {
    let aggregate = crate::radio_resources::initialize_ampdu()?;
    let registers: &'static RadioOwnerArena = REGISTER_ARENA.take();
    Ok(InitialConnectedInitialization {
        resources: InitialConnectedStaticResources {
            registers,
            aggregate: Some(aggregate),
            control: ConnectedControlStatics {
                queue: &*CONTROL_RESOURCES.take(),
                slot: CONNECTED_CONTROL_SLOT.init(ProductionControlSlot::new()),
            },
        },
        #[cfg(feature = "diagnostics")]
        diagnostics: DiagnosticSnapshot { registers },
    })
}

pub(super) const fn connected_config(power: StationPowerMode) -> ConnectedStaConfig {
    ConnectedStaConfig {
        power,
        tx: ConnectedStaTxPolicy {
            rate: ConnectedStaRateConfig {
                high_throughput_enabled: true,
                fallback_legacy_rate: LegacyRate::Ofdm54M,
                fallback_ht_mcs: HtMcs::Mcs7,
                fallback_ht_guard_interval: HtGuardInterval::Short400Ns,
                ht_mcs_override: None,
                // Prefer the qualified 150-Mbit/s HT40 vector when the AP's
                // retained HT Capability IE advertises short GI. The common
                // rate policy downgrades this request to long GI for peers
                // that do not advertise the selected-width capability.
                ht_guard_interval_override: Some(HtGuardInterval::Short400Ns),
                he_mcs_override: None,
                he_guard_interval_and_ltf_override: None,
                he_dcm_override: None,
            },
            completion_timeout: oer_time::Duration::from_micros(250_000),
            aggregate_frame_limit: TX_AMPDU_FRAME_COUNT as u8,
            aggregate_he_txop_limit: HeEdcaTxopLimit::DEFAULT,
            he_trigger_based: None,
        },
        block_ack: ConnectedStaBlockAckPolicy {
            // Request exactly the number of MPDUs the retained TX arena can
            // publish. The peer may negotiate this downward; the aggregate
            // owner then enforces that returned window for every publication.
            tx_block_ack_window: TX_AMPDU_FRAME_COUNT as u16,
            tx_block_ack_negotiation_timeout: oer_time::Duration::from_micros(500_000),
            tx_block_ack_negotiation_attempt_limit: 3,
            // Each pinned network lease is one MPDU. A-MSDU requires a jumbo
            // backing allocation and is not advertised by this 1,648-byte
            // DMA-slot profile.
            tid0_amsdu: false,
            // Keep one negotiated receive window in the live DMA ring and
            // one complete window as service-latency headroom. The hardware
            // agreement geometry remains the vendor-qualified 64 entries;
            // this value is the protocol/reorder limit returned to the peer.
            rx_block_ack_maximum_window: RX_REORDER_WINDOW as u16,
            request_initial_tx_block_ack: true,
        },
        receive: ConnectedStaRxPolicy {
            ingress: RxIngressConfig {
                ring_entry_limit: 1,
                csi_config: 0,
                flags: 0,
            },
            beacon_miss_limit: 10,
            beacon_timeout: oer_espressif_ieee80211_policy::station_link::STATION_INACTIVE_TIME,
        },
    }
}

/// Enter the real connected PAC/Embassy owner graph.
#[allow(
    large_assignments,
    reason = "the connected teardown result is a unique owner graph returned in place; the post-LTO stack gate and runtime stack painting remain authoritative for live stack use"
)]
pub(crate) async fn run_connected<'state, 'security>(
    station_control: &mut StationCommandReceiver<'_, CriticalSectionRawMutex>,
    radio: &'static crate::SharedRadio,
    mac_clock: super::StationMacClock,
    resources: ConnectedStationResources<'state, 'security>,
) -> ConnectedStationRunExit<'state, 'security> {
    let prepared = match prepare_esp32s31_connected_service::<
        TX_AMPDU_FRAME_COUNT,
        RX_REORDER_WINDOW,
        _,
        _,
        _,
    >(resources)
    {
        Ok(prepared) => prepared,
        Err(failure) => {
            let error = failure.error;
            let resources = failure.into_resources();
            return ConnectedStationRunExit::Faulted(
                ConnectedStationFault::InvalidConnectedPolicy {
                    _resources: resources,
                    _error: error,
                },
            );
        }
    };
    let mut started = prepared.start_network(|_runtime, (), _plan| ((), ()));
    let activation = {
        let (runtime, epoch) = started.runtime_and_epoch_mut();
        let (radio, _storage, _board) = runtime.split_mut();
        let (owner, interrupt) = radio.parts_mut();
        let platform = owner.platform_mut();
        let prepared = if interrupt.is_active() {
            match epoch {
                ConnectedEpochResources::Initial { hardware, .. } => {
                    prepare_active_connected_sta_without_power_save(hardware)
                }
                ConnectedEpochResources::Reconnected(reconnected) => {
                    let access = reconnected.hardware_mut().register_access();
                    prepare_active_connected_sta_without_power_save_with_access(&access)
                }
            }
            .map_err(|_| {
                oer_esp32s31_ieee80211_runtime::datapath::irq::MacInterruptEpochActivateError::AlreadyActive
            })
        } else {
            match interrupt.setup_mut() {
                Ok(setup) => match epoch {
                    ConnectedEpochResources::Initial { hardware, .. } => {
                        Ok(setup.prepare_connected_sta_without_power_save(hardware))
                    }
                    ConnectedEpochResources::Reconnected(reconnected) => {
                        let access = reconnected.hardware_mut().register_access();
                        access.try_prepare_connected_sta_without_power_save(setup).map_err(|_| {
                            oer_esp32s31_ieee80211_runtime::datapath::irq::MacInterruptEpochActivateError::AlreadyActive
                        })
                    }
                },
                Err(_) => Err(
                    oer_esp32s31_ieee80211_runtime::datapath::irq::MacInterruptEpochActivateError::AlreadyActive,
                ),
            }
        };
        prepared
            .and_then(|prepared| activate_esp32s31_connected_epoch(interrupt, platform, prepared))
    };
    if let Err(error) = activation {
        diagnostics_event!(
            "open-radio: MAC interrupt activation invariant failed: {error:?}; quarantined"
        );
        return ConnectedStationRunExit::Faulted(ConnectedStationFault::InterruptActivation {
            _started: started,
        });
    }
    let ConnectedNetworkStartedParts {
        runtime,
        epoch,
        stack,
        network: network_runner,
        initial_network_task: stack_runner,
        mut plan,
        installed_security,
        security,
    } = started.into_parts();
    let runtime = runtime.into_parts();
    let (role, interrupt_epoch) = runtime.radio.into_parts();
    let (dma, tx_storage, scan_table, frame, ethernet) = runtime.storage.into_parts();
    let mut board = runtime.board;

    let (start, staged_receiver) = match epoch {
        ConnectedStationEpoch::Initial { hardware, receive } => {
            let (staged_sender, staged_receiver) = STAGED_RX_QUEUE.split();
            let initial = match board.initial_connected.take() {
                Some(initial) => initial,
                None => {
                    return ConnectedStationRunExit::Faulted(
                        ConnectedStationFault::InitialStaticResourcesUnavailable {
                            _runtime: production_station_runtime(
                                role,
                                interrupt_epoch,
                                dma,
                                tx_storage,
                                scan_table,
                                frame,
                                ethernet,
                                board,
                            ),
                            _hardware: hardware,
                            _receive: receive,
                            _stack: stack,
                            _network: network_runner,
                            _initial_network_task: stack_runner,
                            _plan: plan,
                            _installed_security: installed_security,
                            _security: security,
                        },
                    );
                }
            };
            let rx =
                RxEpochResources::new(dma.storage(), &RX_STAGE_POOL, staged_sender, EmbassyClock);
            (
                start_esp32s31_initial_connected_epoch(hardware, receive, initial.with_rx(rx))
                    .await,
                Some(staged_receiver),
            )
        }
        ConnectedStationEpoch::Reconnected(epoch) => (
            start_esp32s31_reconnected_connected_epoch(epoch).await,
            None,
        ),
    };
    let started = match start {
        Ok(started) => started,
        Err(failure) => {
            match &failure {
                ConnectedEpochStartFailure::RegisterPublication { error, .. } => {
                    diagnostics_event!(
                        "open-radio: connected register publication failed: {error:?}; quarantined"
                    );
                }
                ConnectedEpochStartFailure::Receive { phase, error, .. } => {
                    diagnostics_event!(
                        "open-radio: connected RX arm failed phase={phase:?} error={error:?}; quarantined"
                    );
                }
            }
            return ConnectedStationRunExit::Faulted(ConnectedStationFault::EpochStart {
                _runtime: production_station_runtime(
                    role,
                    interrupt_epoch,
                    dma,
                    tx_storage,
                    scan_table,
                    frame,
                    ethernet,
                    board,
                ),
                _failure: failure,
                _stack: stack,
                _network: network_runner,
                _initial_network_task: stack_runner,
                _plan: plan,
                _installed_security: installed_security,
                _security: security,
            });
        }
    };
    let control_tx = match tx_storage.take_control() {
        Ok(control) => control,
        Err(error) => {
            return ConnectedStationRunExit::Faulted(
                ConnectedStationFault::OrdinaryTxUnavailable {
                    _runtime: production_station_runtime(
                        role,
                        interrupt_epoch,
                        dma,
                        tx_storage,
                        scan_table,
                        frame,
                        ethernet,
                        board,
                    ),
                    _started: started,
                    _stack: stack,
                    _network: network_runner,
                    _initial_network_task: stack_runner,
                    _plan: plan,
                    _installed_security: installed_security,
                    _security: security,
                    _error: error,
                },
            );
        }
    };
    let ProductionStationBoardResources {
        access_point_airtime,
        interface,
        connected_datapath,
        rx_protocol_runtime,
        sta_ap_rx_batch,
        initial_connected,
        #[cfg(feature = "diagnostics")]
        diagnostics,
    } = board;
    let (sequences, mut material) = security.into_parts();
    let ConnectedEpochStarted {
        hardware,
        rx,
        aggregate_tx: aggregate,
        control: control_resources,
    } = started;
    let staged_receiver = staged_receiver.unwrap_or_else(|| {
        rx.try_resume_standalone_receiver()
            .expect("reconnected station RX retains its standalone producer")
    });
    let material_is_open = matches!(&material, StaAttemptSecurityMaterial::Open);
    let material_has_connected_wpa2 = matches!(
        &material,
        StaAttemptSecurityMaterial::Personal {
            connected: Some(_),
            ..
        }
    );
    let (tx_security, group_security) = match installed_security {
        StaInstalledSecurity::Open if material_is_open => {
            (ConnectedTxSecurity::Open, ConnectedStaGroupSecurity::Open)
        }
        StaInstalledSecurity::Wpa2Personal {
            pairwise,
            group,
            group_material,
            replay,
            management,
        } if material_has_connected_wpa2 => {
            let (replay_rx, replay_control) = match STA_CCMP_RX_REPLAY.start(replay) {
                Ok(endpoints) => endpoints,
                Err(failure) => {
                    return ConnectedStationRunExit::Faulted(ConnectedStationFault::ReplaySetup {
                        _runtime: production_station_runtime(
                            role,
                            interrupt_epoch,
                            dma,
                            tx_storage,
                            scan_table,
                            frame,
                            ethernet,
                            ProductionStationBoardResources {
                                access_point_airtime,
                                interface,
                                connected_datapath,
                                rx_protocol_runtime,
                                sta_ap_rx_batch,
                                initial_connected,
                                #[cfg(feature = "diagnostics")]
                                diagnostics,
                            },
                        ),
                        _started: ConnectedEpochStarted {
                            hardware,
                            rx,
                            aggregate_tx: aggregate,
                            control: control_resources,
                        },
                        _stack: stack,
                        _network: network_runner,
                        _initial_network_task: stack_runner,
                        _plan: plan,
                        _failure: ConnectedStationReplaySetupFailure::Start {
                            _failure: failure,
                            _pairwise: pairwise,
                            _group: group,
                            _group_material: group_material,
                        },
                        _sequences: sequences,
                        _material: material,
                        _control_tx: control_tx,
                    });
                }
            };
            let tx_security = ConnectedTxSecurity::Wpa2Personal(pairwise);
            let group_security = ConnectedStaGroupSecurity::Wpa2PersonalRekey {
                group,
                material: group_material,
                replay: replay_control,
                management,
            };
            if let Err(failure) = plan.enable_ccmp_rx_replay(replay_rx) {
                return ConnectedStationRunExit::Faulted(ConnectedStationFault::ReplaySetup {
                    _runtime: production_station_runtime(
                        role,
                        interrupt_epoch,
                        dma,
                        tx_storage,
                        scan_table,
                        frame,
                        ethernet,
                        ProductionStationBoardResources {
                            access_point_airtime,
                            interface,
                            connected_datapath,
                            rx_protocol_runtime,
                            sta_ap_rx_batch,
                            initial_connected,
                            #[cfg(feature = "diagnostics")]
                            diagnostics,
                        },
                    ),
                    _started: ConnectedEpochStarted {
                        hardware,
                        rx,
                        aggregate_tx: aggregate,
                        control: control_resources,
                    },
                    _stack: stack,
                    _network: network_runner,
                    _initial_network_task: stack_runner,
                    _plan: plan,
                    _failure: ConnectedStationReplaySetupFailure::Plan {
                        _failure: failure,
                        _tx_security: tx_security,
                        _group_security: group_security,
                    },
                    _sequences: sequences,
                    _material: material,
                    _control_tx: control_tx,
                });
            }
            (tx_security, group_security)
        }
        installed_security => {
            return ConnectedStationRunExit::Faulted(
                ConnectedStationFault::SecurityOwnershipMismatch {
                    _runtime: production_station_runtime(
                        role,
                        interrupt_epoch,
                        dma,
                        tx_storage,
                        scan_table,
                        frame,
                        ethernet,
                        ProductionStationBoardResources {
                            access_point_airtime,
                            interface,
                            connected_datapath,
                            rx_protocol_runtime,
                            sta_ap_rx_batch,
                            initial_connected,
                            #[cfg(feature = "diagnostics")]
                            diagnostics,
                        },
                    ),
                    _started: ConnectedEpochStarted {
                        hardware,
                        rx,
                        aggregate_tx: aggregate,
                        control: control_resources,
                    },
                    _stack: stack,
                    _network: network_runner,
                    _initial_network_task: stack_runner,
                    _plan: plan,
                    _installed_security: installed_security,
                    _sequences: sequences,
                    _material: material,
                    _control_tx: control_tx,
                },
            );
        }
    };
    let negotiated_security = tx_security.mode();
    let mut group_security = Some(group_security);
    #[cfg(feature = "diagnostics")]
    log_rx_ring_topology("started", &rx);
    #[cfg(feature = "diagnostics")]
    let rx = if let Some(observer) = diagnostics.and_then(|hooks| hooks.rx_pipeline) {
        rx.with_pipeline_observer(observer)
    } else {
        rx
    };

    let network_rx = network_runner
        .rx_publisher(oer_esp32s31_ieee80211_runtime::datapath::network::STA_NETWORK_INTERFACE_ID);
    let ConnectedControlStatics {
        queue: control_resources,
        slot: control_slot,
    } = control_resources;
    let (control_publisher, control_receiver) = control_resources.split();
    let rx_sink = EmbassyNetConnectedRxSink::new(network_rx, control_publisher);
    #[cfg(feature = "diagnostics")]
    let rx_sink = {
        let hooks =
            diagnostics.expect("diagnostics build must retain its configured pipeline observer");
        let rx_sink = rx_sink.with_delivery_observer(hooks.rx_delivery);
        if let Some(observer) = hooks.rx_pipeline {
            rx_sink.with_pipeline_observer(observer)
        } else {
            rx_sink
        }
    };
    #[cfg(feature = "diagnostics")]
    let rx_sink = ObservedConnectedRxSink {
        inner: rx_sink,
        observer: diagnostics
            .expect("diagnostics build must retain its configured connected RX observer")
            .connected_rx,
    };
    let (reorder_sender, reorder_receiver) = RX_REORDER_COMMANDS.split();
    let tx_sequences = sequences;
    let power = STATION_POWER_LINK.bind(radio.wifi_coex_view());
    WifiReconnectPolicy::get().association_started();
    let drivers = match ConnectedStaPort::compose(
        plan,
        hardware,
        rx,
        ConnectedStaRxProtocolResources {
            frames: staged_receiver,
            irq: &IRQ_RUNTIME,
            sink: rx_sink,
            mpdu: frame,
            ethernet,
            reorder_commands: reorder_receiver,
            reorder_storage: &RX_REORDER_STORAGE,
            runtime: rx_protocol_runtime,
            reorder_scratch: None,
            clock: EmbassyClock,
            #[cfg(feature = "diagnostics")]
            pipeline_observer: diagnostics.and_then(|hooks| hooks.rx_pipeline),
            #[cfg(feature = "diagnostics")]
            reorder_observer: diagnostics.and_then(|hooks| hooks.rx_reorder),
        },
        ConnectedStaTxResources {
            control: control_tx,
            aggregate,
            security: tx_security,
            sequences: tx_sequences,
            #[cfg(feature = "diagnostics")]
            aggregate_tx_observer: diagnostics.map(|hooks| hooks.aggregate_tx),
            tx_block_ack_status_sink: Some(crate::status::publish_station_tx_block_ack),
            network_domain: ConnectedStaNetworkTxDomain::new(),
        },
        ConnectedStaControlResources {
            slot: control_slot,
            receiver: control_receiver,
            reorder_commands: reorder_sender,
            rx_block_ack: &crate::supervisor::PRODUCTION_RX_BLOCK_ACK,
            tsf_epoch: mac_clock.tsf_epoch(),
            power: Some(power),
        },
    ) {
        Ok(drivers) => drivers,
        Err(composition) => {
            diagnostics_event!("open-radio: connected TX handoff found a live owner; quarantined");
            let failure = ConnectedDriverAssemblyFailure {
                network: network_runner,
                composition,
                map_services: core::convert::identity::<ConnectedDriverServices>
                    as ConnectedServicesMapper,
            };
            return ConnectedStationRunExit::Faulted(ConnectedStationFault::DriverAssembly {
                _fault: ConnectedDriverAssemblyFault {
                    _role: role,
                    _interrupt: interrupt_epoch,
                    _dma: dma,
                    _tx_storage: tx_storage,
                    _scan_table: scan_table,
                    _stack: stack,
                    _initial_network_task: stack_runner,
                    _control_resources: control_resources,
                    _group_security: group_security
                        .take()
                        .expect("pre-control composition retains group security"),
                    _material: material,
                    _interface: interface,
                    _failure: failure,
                },
            });
        }
    };
    let report = drivers.report;
    let mut radio_runner = DatapathRunner::new(
        &IRQ_RUNTIME,
        network_runner,
        oer_esp32s31_ieee80211_runtime::datapath::network::STA_NETWORK_INTERFACE_ID,
        drivers.services,
        EmbassyClock,
    );
    if let StaAttemptSecurityMaterial::Personal { connected, .. } = &mut material {
        let ConnectedStaGroupSecurity::Wpa2PersonalRekey {
            group,
            material: group_material,
            replay,
            management,
        } = group_security
            .take()
            .expect("WPA2 composition retains group security")
        else {
            unreachable!("validated WPA2 composition retains group rekey owners");
        };
        if radio_runner
            .services_mut()
            .control_mut()
            .install_wpa2_security(
                ConnectedWpa2Security::new(
                    connected
                        .take()
                        .expect("installed WPA2 keys retain connected supplicant state"),
                    group,
                    group_material,
                    replay,
                    management,
                ),
                super::tx_entropy,
            )
            .is_err()
        {
            unreachable!("a fresh connected control owner has no WPA2 session");
        }
    }

    // RX materialization published the complete descriptor ring while the
    // vendor stop request remained asserted. All later fallible composition
    // edges are now closed, so the MAC may consume those credits.
    radio_runner
        .services_mut()
        .hardware_mut()
        .resume_mac_runtime();

    let _initial_network_marker = stack_runner;
    diagnostics_event!(
        "open-radio: connected datapath active phy={} tx={}kbps ampdu={}kbps mcs32={:?}",
        report.link.association_phy.name(),
        report.data_tx_rate.nominal_kbps(),
        report.aggregate_tx_rate.nominal_kbps(),
        report.ht_duplicate_tx_selection,
    );
    #[cfg(feature = "diagnostics")]
    // This finite register snapshot is emitted at Info in diagnostics
    // images because rare cold-start beacon loss occurs before a traffic
    // session can request the ordinary diagnostics snapshot. Production
    // images compile the event out entirely.
    log_sta_receive_policy(
        "start",
        radio_runner
            .services()
            .hardware()
            .sta_receive_policy_snapshot(),
    );
    #[cfg(feature = "diagnostics")]
    diagnostics_debug!(
        "open-radio: connected RX statistics: {:?}",
        radio_runner
            .services()
            .hardware()
            .register_access()
            .try_receive_statistics_snapshot()
            .expect("diagnostics snapshot must not overlap another MMIO transaction"),
    );
    #[cfg(feature = "diagnostics")]
    diagnostics_debug!(
        "open-radio: connected RX DMA: {:?}",
        radio_runner.services().hardware().mac_rx_dma_snapshot(),
    );
    crate::status::publish_station_connected(
        u16::from(report.link.association_phy.bandwidth_mhz()),
        crate::status::StationLinkSecurity::new(
            negotiated_security,
            report.link.management_protection,
            report.link.sae,
        ),
    );
    #[cfg(feature = "diagnostics")]
    {
        DIAGNOSTIC_DATA_RATE_KBPS.store(report.data_tx_rate.nominal_kbps(), Ordering::Relaxed);
        DIAGNOSTIC_AGGREGATE_RATE_KBPS
            .store(report.aggregate_tx_rate.nominal_kbps(), Ordering::Relaxed);
        DIAGNOSTIC_LINK_BANDWIDTH_MHZ.store(
            u32::from(report.link.association_phy.bandwidth_mhz()),
            Ordering::Release,
        );
    }

    let mut radio_runner = Some(radio_runner);
    let mut role = role;
    let mut station_rf = StationRf::new(&mut role);
    let (result, requested_command) = await_stack_boundary!(execution::run(
        connected_datapath,
        station_control,
        &mut radio_runner,
        run_station_power_agent(
            &STATION_POWER_LINK,
            radio,
            crate::interrupts::power_irq_runtime(),
            &mut station_rf,
            &mac_clock,
        ),
    ));
    // The stop's air releases and RF wake reach the agent after the
    // datapath returned; perform them before the radio resources leave.
    let power_stop =
        finish_station_power(&STATION_POWER_LINK, radio, &mut station_rf, &mac_clock).await;
    let rf_asleep = station_rf.asleep();
    WifiReconnectPolicy::get().association_lost(radio).await;
    if let Err(failure) = power_stop {
        diagnostics_event!("open-radio: station power agent failed at stop: {failure:?}");
    }
    let platform = role.platform_mut();
    let radio_runner = radio_runner.expect("terminal execution returns its live owner");
    let raw_exit =
        complete_esp32s31_connected_datapath_exit(station_control, result, requested_command);
    let mut stopped = match await_stack_boundary!(quiesce_completed_esp32s31_connected_epoch::<
        _,
        _,
        CriticalSectionRawMutex,
        _,
        _,
    >(
        interrupt_epoch,
        platform,
        radio_runner,
        raw_exit,
        |exit, _runner| {
            #[cfg(feature = "diagnostics")]
            if let ConnectedStationExit::HardwareFailure(error) = &exit {
                // Preserve the fail-closed reason ahead of the larger exit
                // snapshots. A saturated diagnostic writer must not erase
                // the one fact needed to distinguish TX, RX and control
                // ownership failures during HIL triage.
                diagnostics_event!("open-radio: connected hardware failure: {error:?}");
                diagnostics_event!(
                    "open-radio: connected RX address mismatch: {:?}",
                    _runner
                        .services()
                        .rx()
                        .dma()
                        .first_buffer_address_mismatch()
                );
            }
            if let Ok(statistics) = _runner
                .services()
                .hardware()
                .register_access()
                .try_receive_statistics_snapshot()
            {
                crate::exit_evidence::record(statistics);
            }
            #[cfg(feature = "diagnostics")]
            {
                let control = _runner.services().control();
                let beacon = control.beacon_monitor();
                log_sta_receive_policy(
                    "exit",
                    _runner.services().hardware().sta_receive_policy_snapshot(),
                );
                let statistics = _runner
                    .services()
                    .hardware()
                    .register_access()
                    .try_receive_statistics_snapshot()
                    .expect("diagnostics snapshot must not overlap another MMIO transaction");
                // Short lines: the console truncates long ones.
                let primary = statistics.primary;
                diagnostics_event!(
                    "open-radio: connected exit RX mpdu={} end={} data_success={} other_unicast={} fcs_error={} abort={} signal_field={} power_drop={}",
                    primary.mpdu_count,
                    primary.end,
                    primary.data_success,
                    primary.other_unicast,
                    primary.fcs_error,
                    primary.abort,
                    primary.signal_field,
                    primary.power_drop_error,
                );
                diagnostics_event!(
                    "open-radio: connected exit RX buffer_full={} fifo_overflow={} last_unmatched={} bt_block={} ack_irq={} rts_irq={}",
                    primary.buffer_full,
                    primary.fifo_overflow,
                    primary.last_unmatched_error,
                    primary.bt_block_error,
                    primary.ack_interrupt,
                    primary.rts_interrupt,
                );
                diagnostics_event!("open-radio: connected exit RX hang={:?}", statistics.hang,);
                diagnostics_event!(
                    "open-radio: connected exit RX decode={:?}",
                    statistics.decode_errors,
                );
                diagnostics_event!(
                    "open-radio: connected exit RX DMA: {:?}",
                    _runner.services().hardware().mac_rx_dma_snapshot(),
                );
                log_rx_ring_topology("exit", _runner.services().rx().dma());
                // Short lines: the console truncates long ones.
                diagnostics_event!(
                    "open-radio: connected exit beacon lost={} observed={} deadline={:?} now={} last_tsf={:?}",
                    control.beacon_lost(),
                    beacon.map_or(0, |monitor| monitor.observed()),
                    beacon
                        .and_then(|monitor| monitor.deadline())
                        .map(oer_time::Instant::as_micros),
                    embassy_time::Instant::now().as_micros(),
                    beacon
                        .and_then(|monitor| monitor.last_observation())
                        .map(|observation| observation.timestamp_tsf),
                );
                diagnostics_event!(
                    "open-radio: connected exit control last_event={:?}",
                    control.last_event(),
                );
                diagnostics_event!(
                    "open-radio: connected exit control last_tx_failure={:?}",
                    control.last_tx_failure(),
                );
                let power = control.power_management();
                diagnostics_event!(
                    "open-radio: connected exit power state={:?} started={} in_slice={}",
                    power.state(),
                    power.is_started(),
                    power.in_slice(),
                );
                diagnostics_event!(
                    "open-radio: connected exit hardware_beacon_frontier={:?}",
                    control.hardware_beacon_monitor_frontier(),
                );
                diagnostics_event!(
                    "open-radio: connected exit stale_addba_responses={} last_stale_addba_token={:?} security={:?}",
                    control.stale_tx_block_ack_responses(),
                    control.last_stale_tx_block_ack_token(),
                    control.wpa2_security().map(|security| security.evidence()),
                );
            }
            match exit {
                ConnectedStationExit::Disconnected(reason) => {
                    crate::status::publish_station_disconnected(reason);
                    ConnectedStationOutcome::Disconnected(reason)
                }
                ConnectedStationExit::ReconnectRequested { .. } => {
                    ConnectedStationOutcome::ReconnectRequested
                }
                ConnectedStationExit::StationStopped(command) => {
                    ConnectedStationOutcome::StationStopped(command)
                }
                ConnectedStationExit::HardwareFailure(error) => {
                    let _ = error;
                    ConnectedStationOutcome::HardwareFailure
                }
            }
        },
    )) {
        Ok(stopped) => stopped,
        Err(mut pending) => loop {
            diagnostics_event!(
                "open-radio: MAC interrupt quiescence still pending: {:?}",
                pending.error
            );
            Timer::after_millis(1).await;
            match pending.retry_quiesce(platform) {
                Ok(stopped) => break stopped,
                Err(returned) => pending = returned,
            }
        },
    };
    // The teardown moves every owner by value. Outside this poll frame,
    // those moves do not add to the async state machine's stack budget.
    super::outside_poll_frame(move || {
        #[cfg(feature = "diagnostics")]
        DIAGNOSTIC_LINK_BANDWIDTH_MHZ.store(0, Ordering::Release);
        let outcome = stopped.exit;
        diagnostics_event!("open-radio: DATAPATH runner stopped: {outcome:?}");
        let group_security = match &mut material {
            StaAttemptSecurityMaterial::Open => group_security
                .take()
                .expect("Open connected epoch retains its no-key group marker"),
            StaAttemptSecurityMaterial::Personal { connected, .. } => {
                let security = stopped
                    .quiesced
                    .services
                    .control_mut()
                    .take_wpa2_security()
                    .expect("connected WPA2 control returns its association security owner");
                let (returned_connected, group) = security.into_parts();
                *connected = Some(returned_connected);
                ConnectedStaGroupSecurity::Wpa2Personal(group)
            }
        };
        let teardown = match stopped.try_teardown(group_security) {
            Ok(teardown) => teardown,
            Err(failure) => {
                let message = match &failure.error {
                    ConnectedStaTeardownFailure::Control { .. } => {
                        "connected control teardown failed"
                    }
                    ConnectedStaTeardownFailure::Rx { .. } => "connected RX DMA teardown failed",
                    ConnectedStaTeardownFailure::TxActive { .. } => "connected TX remained active",
                };
                diagnostics_event!("open-radio: {message}; quarantined");
                let oer_esp32s31_ieee80211_runtime::roles::station::connected::ConnectedServiceTeardownFailure {
                    exit,
                    interrupt,
                    interrupt_drain,
                    network,
                    error,
                } = failure;
                return ConnectedStationRunExit::Faulted(ConnectedStationFault::DriverTeardown {
                    _role: role,
                    _interrupt: interrupt,
                    _dma: dma,
                    _tx_storage: tx_storage,
                    _scan_table: scan_table,
                    _interface: interface,
                    _sta_ap_rx_batch: sta_ap_rx_batch,
                    _initial_connected: initial_connected,
                    #[cfg(feature = "diagnostics")]
                    _diagnostics: diagnostics,
                    _network: RunningStationNetwork::new(stack, network),
                    _control_resources: control_resources,
                    _outcome: exit,
                    _interrupt_drain: interrupt_drain,
                    _error: CONNECTED_DRIVER_TEARDOWN_FAULT.init(error),
                    _material: material,
                });
            }
        };
        let network_runner = teardown.network;
        let interrupt_epoch = teardown.interrupt;
        let interrupt_drain = teardown.interrupt_drain;
        let teardown = teardown.driver;
        let (parked_rx, stopped_protocol) = teardown.parked_rx.into_parts();
        let parked_rx = ConnectedParkedRx::from_live(parked_rx);
        let shutdown = stopped_protocol.shutdown();
        diagnostics_event!(
            "open-radio: RX protocol stopped queued={} retained={} commands={} active={}",
            shutdown.queued_frames,
            shutdown.retained_frames,
            shutdown.reorder_commands,
            shutdown.active_reorders,
        );
        let (frame, ethernet, rx_protocol_runtime) = stopped_protocol.into_parts();
        let sequences = teardown.sequences;
        let control_resources = ConnectedControlStatics {
            queue: control_resources,
            slot: teardown.released_control,
        };
        if matches!(
            teardown.security,
            ConnectedStaSecurityStopReport::ModeMismatchCleared { .. }
        ) {
            diagnostics_event!(
                "open-radio: connected security teardown observed unlike modes; quarantined"
            );
            return ConnectedStationRunExit::Faulted(
                ConnectedStationFault::SecurityTeardownMismatch {
                    _runtime: production_station_runtime(
                        role,
                        interrupt_epoch,
                        dma,
                        tx_storage,
                        scan_table,
                        frame,
                        ethernet,
                        ProductionStationBoardResources {
                            access_point_airtime,
                            interface,
                            connected_datapath,
                            rx_protocol_runtime,
                            sta_ap_rx_batch,
                            initial_connected,
                            #[cfg(feature = "diagnostics")]
                            diagnostics,
                        },
                    ),
                    _network: RunningStationNetwork::new(stack, network_runner),
                    _control_resources: control_resources,
                    _outcome: outcome,
                    _interrupt_drain: interrupt_drain,
                    _hardware: teardown.hardware,
                    _parked_rx: parked_rx,
                    _tx_resources: teardown.tx_resources,
                    _aggregate: teardown.aggregate,
                    _control_observation: teardown.control,
                    _security_stop: teardown.security,
                    _sequences: sequences,
                    _material: material,
                },
            );
        }
        if let Err(failure) = tx_storage.restore_resources(teardown.tx_resources) {
            diagnostics_event!("open-radio: connected TX return found a live owner; quarantined");
            let (error, returned_control) = failure;
            return ConnectedStationRunExit::Faulted(ConnectedStationFault::TxRestore {
                _runtime: production_station_runtime(
                    role,
                    interrupt_epoch,
                    dma,
                    tx_storage,
                    scan_table,
                    frame,
                    ethernet,
                    ProductionStationBoardResources {
                        access_point_airtime,
                        interface,
                        connected_datapath,
                        rx_protocol_runtime,
                        sta_ap_rx_batch,
                        initial_connected,
                        #[cfg(feature = "diagnostics")]
                        diagnostics,
                    },
                ),
                _network: RunningStationNetwork::new(stack, network_runner),
                _control_resources: control_resources,
                _outcome: outcome,
                _interrupt_drain: interrupt_drain,
                _hardware: teardown.hardware,
                _parked_rx: parked_rx,
                _aggregate: teardown.aggregate,
                _control_observation: teardown.control,
                _security_stop: teardown.security,
                _sequences: sequences,
                _error: error,
                _returned_control: returned_control,
                _material: material,
            });
        }
        let disconnected: ConnectedDisconnectedEpoch = DisconnectedStaEpoch::new(
            RunningStationNetwork::new(stack, network_runner),
            teardown.hardware,
            parked_rx,
            teardown.aggregate,
            control_resources,
        );
        let returned = ConnectedStationReturn {
            disconnected,
            runtime: production_station_runtime(
                role,
                interrupt_epoch,
                dma,
                tx_storage,
                scan_table,
                frame,
                ethernet,
                crate::supervisor::ProductionStationBoardResources {
                    access_point_airtime,
                    interface,
                    connected_datapath,
                    rx_protocol_runtime,
                    sta_ap_rx_batch,
                    initial_connected,
                    #[cfg(feature = "diagnostics")]
                    diagnostics,
                },
            ),
            security: match material {
                StaAttemptSecurityMaterial::Open => StaAttemptSecurity::open(sequences),
                StaAttemptSecurityMaterial::Personal {
                    credentials,
                    supplicant_nonce,
                    message4_protection,
                    ..
                } => StaAttemptSecurity::new(
                    credentials,
                    supplicant_nonce,
                    sequences,
                    message4_protection,
                ),
            },
            outcome,
        };
        // RF that could not wake leaves the Wi-Fi PHY client suspended; the vendor
        // aborts on a failed `esp_phy_enable`, and the owners stay quarantined.
        if rf_asleep {
            return ConnectedStationRunExit::Faulted(ConnectedStationFault::RfAsleep {
                _returned: returned,
            });
        }
        ConnectedStationRunExit::Returned(returned)
    })
}

// Size guards for the connected epoch's owners, interim until the static
// stack bound () checks the frames that move them. The connected control
// lives in `CONNECTED_CONTROL_SLOT`; none of these holds it by value. Each
// limit is the measured size plus 64 bytes.
const fn returned_size<A, B, C, D, F, R>(_: &F) -> usize
where
    F: FnOnce(A, B, C, D) -> R,
{
    core::mem::size_of::<R>()
}
// Measured 3248 bytes; its largest variant holds no connected control.
const _: () = assert!(core::mem::size_of::<ConnectedStationFault<'static, 'static>>() <= 3312);
// Measured 3248 bytes.
const _: () = assert!(core::mem::size_of::<ConnectedStationRunExit<'static, 'static>>() <= 3312);
// Measured 1488 bytes (3816 while it held the control by value).
const _: () = assert!(core::mem::size_of::<ConnectedDriverTeardownFailure>() <= 1552);
// The connected epoch's future: measured 26216 bytes in the largest image
// (`diagnostic-tx-wait`), 8 more since a radio-clock generation became a
// typed `{ epoch, jump }` pair (33024 while it held the control by value).
const _: () = assert!(
    returned_size::<
        &'static mut StationCommandReceiver<'static, CriticalSectionRawMutex>,
        &'static crate::SharedRadio,
        super::StationMacClock,
        ConnectedStationResources<'static, 'static>,
        _,
        _,
    >(&run_connected)
        <= 26280
);
