#![expect(
    clippy::large_enum_variant,
    reason = "no-alloc supervisor states retain concrete reusable and faulted hardware frontiers"
)]
#![expect(
    clippy::manual_async_fn,
    reason = "the epoch runner implementation keeps its trait's borrowed Future contract explicit"
)]
#![expect(
    clippy::result_large_err,
    reason = "supervisor failures return the exact physical and role owners instead of allocating"
)]
#![expect(
    clippy::too_many_arguments,
    reason = "runtime assembly exposes independent affine hardware and board-resource owners"
)]

//! Production ESP32-S31 STA/AP/monitor composition and sole radio owner.
//!
//! The target owns board allocation and application policy. Every radio
//! transition is supplied by a PAC-backed driver or reusable integration
//! owner; no HIL protocol, telemetry or benchmark configuration is linked.

use core::{future::Future, marker::PhantomData, pin::Pin};
use oer_esp32s31_coex::CoexError;

use crate::{
    composition::{
        start::{
            RadioStartConfig, RadioStartFailure, WifiPowerUpFailure, power_up_esp32s31_wifi,
            start_esp32s31_radio,
        },
        supervisor::{
            RadioSupervisorTask, StationSupervisorEpoch, StationSupervisorHooks,
            WifiSupervisorStopped, drive_esp32s31_monitor_role, prepare_esp32s31_radio_supervisor,
            run_esp32s31_station_supervisor_epoch,
        },
    },
    resources::profile::{
        DefaultScanMemory, DefaultWifiMemory,
        ESP32S31_DEFAULT_RX_BUFFER_STORAGE_SIZE as RX_BUFFER_STORAGE_SIZE,
        ESP32S31_DEFAULT_RX_STAGE_CAPACITY as RX_STAGE_CAPACITY,
        ESP32S31_DEFAULT_TX_BUFFER_SIZE as TX_BUFFER_SIZE,
    },
};

pub(super) use crate::resources::profile::{
    ESP32S31_DEFAULT_RX_BUFFER_SIZE as RX_BUFFER_SIZE,
    ESP32S31_DEFAULT_RX_DESCRIPTOR_COUNT as RX_DESCRIPTOR_COUNT,
};

#[cfg(feature = "diagnostics")]
use oer_esp32s31_ieee80211_runtime::roles::station::StationEngineObserver;

use embassy_executor::Spawner;

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;

use esp_hal::rng::{Rng, Trng};

use oer_radio::wifi::{
    AccessPointRequest, AccessPointSecurity, StationDiscovery, StationPowerMode, StationRequest,
    StationSecurity, WIFI_SCAN_RESULT_CAPACITY, WifiAccessPointConfig, WifiConfig,
    WifiRadioRestartRf, WifiScanFailure, WifiScanReport, WifiScanRequest, WifiScanResult,
    WifiServicePlanningError, WifiServiceRequest, WifiStartFailure, WifiStartReport,
    WifiStationConfig, WifiStopReport, WifiSupervisorConfiguration,
};
use oer_radio_supervisor::{
    WifiRoleEpochOutcome, WifiRoleEpochRunner, WifiRoleFrontier, WifiStartKind,
    WifiSupervisorCommand, WifiSupervisorControlResources, WifiSupervisorEndpoint,
    WifiSupervisorResponse, finish_wifi_active_role,
};

use oer_esp32s31_phy::{NoopPhyTargetObserver, PhyTxTargetPowerProfile};

use oer_esp32s31_ieee80211::{
    cold_start::WifiColdStartConfig as Esp32s31WifiStartConfig,
    lower_wifi_channel,
    mac_start::WifiMacStartConfig,
    runtime::{
        WifiReleaseError, WifiReleaseFailure, WifiRoleOwner, WifiStopped,
        materialize_esp32s31_wifi_role,
    },
    tx::ControlTxConfig,
};

use oer_esp32s31_ieee80211_runtime::roles::monitor::{
    MonitorChannelSwitchError, MonitorRadio, MonitorTaskExit, prepare_esp32s31_monitor_task,
};

#[cfg(feature = "diagnostics")]
use oer_ieee80211_sta::station::StaBackoffReason;

use oer_esp32s31_hal::{
    owner::RadioRuntimeOwner,
    root::WifiPartition,
    shared_radio::{CoexAbsent, Coexisting, CommonPower, WifiClient},
};

use oer_esp32s31_ieee80211_ap::{engine::ApEngine, transaction::ApMac, tx::ApTxConfig};

use oer_esp32s31_ieee80211_runtime::{
    datapath::rx::{dma::ReceiveDmaStorage, frontier::ReceiveFrontier},
    roles::{
        access_point::{
            AccessPointControl, AccessPointRxReorder,
            AccessPointStopped as EmbassyAccessPointStopped,
        },
        scan::rx::{RunningScanRx, ScanFrameObserver, ScanRx},
        station::{
            ESP32S31_STATION_PROBE_DESCRIPTOR_CAPACITY, ESP32S31_STATION_PROBE_RATES,
            RadioOwnerRepublish, StationCommandReceiver, StationConfiguration,
            StationConnectedPhase, StationControlResources, StationController, StationDmaResources,
            StationEngine, StationEnginePort, StationExit, StationInitialJoinPhase,
            StationInitialScanExit, StationInitialScanFailures, StationInitialScanPhase,
            StationInitialScanReturned, StationJoinExit, StationJoinOutcome, StationJoinResources,
            StationPrepareFailure, StationRadioResources, StationReconnectedPhase,
            StationRunningScanCompletion, StationRunningScanExit, StationRunningScanPhase,
            StationRuntimeReclaimFailure, StationRuntimeResources, StationScanDecision,
            StationScanPlan, StationScanRequest, StationScanResources, StationServiceOwner,
            StationServicePhase, StationStartResources, StationStoppedPhaseResources,
            StationStorageResources, StationTask, complete_esp32s31_station_initial_scan,
            complete_esp32s31_station_running_scan, epoch::RunningScanEpochParts,
            esp32s31_station_scan_failure_disposition, prepare_esp32s31_station_task,
            run_esp32s31_station_join, run_esp32s31_station_scan,
            try_rebind_esp32s31_station_phase, try_reclaim_esp32s31_station_runtime,
            try_restore_esp32s31_station_phase, tx_epoch::StaTxEpochExt,
        },
    },
};
use oer_esp32s31_phy::RomShortDelay;
use oer_time_embassy::EmbassyClock;

use oer_esp32s31_ieee80211_esp_hal::EspHalWifiPlatform;

use oer_esp32s31_phy::{ConcurrentRfError, tracking::fail_stop::SharedPhyFailStop};

use oer_esp32s31_radio_runtime::{CoexLeaveFailure, RadioPhyError};

use oer_esp32s31_ieee80211_mac::{
    init::activate_promiscuous_receive,
    rx::{RxDmaBufferAddresses, RxRingError},
    tx::TxSlot,
};

use oer_esp32s31_ieee80211_sta::{
    attempt::{
        StaAttemptObserver, StaAttemptSecurity, StaAttemptStage, StaAttemptStation, StaIdentity,
        StaPersonalCredentials,
    },
    control_tx::{ControlTransmitter, WifiTxResources},
    hardware::channel::ScanPhy,
    tx_epoch::StaTxEpoch,
};

use oer_ieee80211_mac::{
    channel::WifiChannel,
    scan::{SCAN_RECORD_CAPACITY, ScanObservation, ScanTable},
    station::StaTxSequenceCounters,
};

use oer_ieee80211_ap::AccessPointService;

use oer_ieee80211_runtime::await_stack_boundary;

use oer_ieee80211_softmac::interface::BoundVirtualInterface;

use oer_ieee80211_sta::station::{
    StaAttemptContext, StaAttemptFailure, StaAttemptOutcome, StaFailureDisposition, StaFault,
    StaFaultCause, StaFaultPhase, StaFaultReason, StaNextCandidate,
};

use oer_ieee80211_rsn::frames::RsnGtk;

use static_cell::{ConstStaticCell, StaticCell};

#[cfg(feature = "mac-irq-diagnostics")]
use crate::interrupts::configure_mac_irq_observer;

use crate::{
    NewError, RadioError, SharedRadio, WifiInitialization, WifiStarted, WifiSystem,
    interrupts::{MacInterruptEpoch, mac_interrupt_epoch},
    monitor::{
        CaptureResources, MonitorMemory, MonitorResourcesError, ProductionMonitorBuildFailure,
        ProductionMonitorResources, ProductionMonitorTask, initialize_monitor_resources,
    },
    radio_resources::{
        NetworkRunner, RadioAmpduStorage, RadioNetworkTxBacking, RadioTxBacking,
        RunningWifiNetwork, WifiNetworkResources,
    },
    supervisor::station::{
        ConnectedControlStatics, ConnectedDisconnectedEpoch, ConnectedParkedRx,
        ConnectedReconnectedEpoch, ConnectedRxEpochResources, ConnectedRxProtocolStorage,
        ConnectedStationEpoch, ConnectedStationFault, ConnectedStationOutcome,
        ConnectedStationResources, ConnectedStationRunExit, InitialConnectedStaticResources,
        ProductionAccessPointRxConsumer, ProductionAccessPointRxProducer, access_point_rx_pipeline,
        connected_config, initialize_connected_datapath_mailbox,
        initialize_connected_rx_protocol_runtime, initialize_connected_static_resources,
        initialize_ethernet_frame, initialize_sta_ap_station_rx_batch, run_connected,
    },
};

mod access_point;
#[cfg(feature = "diagnostics")]
mod access_point_observation;
mod concurrent;
mod physical;
mod role_transition;
mod shutdown;
pub(crate) mod station;

/// Zero-copy origin of the physical RX staging pool.
#[cfg(feature = "owned-network")]
pub(crate) fn rx_zero_copy_origin()
-> &'static oer_esp32s31_ieee80211_runtime::datapath::owned::RxZeroCopyOrigin {
    &station::RX_ZERO_COPY_ORIGIN
}

use access_point::{
    ProductionAccessPointPreparationFault, ProductionAccessPointResources,
    ProductionAccessPointTask, ProductionAccessPointTeardownFault,
};

pub(crate) use physical::ProductionRxRing;

use physical::ProductionWifiPhysicalResources;

use role_transition::{
    ProductionStationRoleResources, join_station_activation_resources,
    try_split_wifi_stopped_resources,
};

const MAC_HANDSHAKE_SAMPLE_LIMIT: u32 = 100_000;
const TX_COMPLETION_TIMEOUT: oer_time::Duration = oer_time::Duration::from_millis(250);

pub(super) type RxStorage =
    ReceiveDmaStorage<RX_DESCRIPTOR_COUNT, RX_BUFFER_SIZE, RX_BUFFER_STORAGE_SIZE>;
pub(super) type ControlTx =
    ControlTransmitter<'static, PhyTxTargetPowerProfile, fn() -> u32, MacTimer, TX_BUFFER_SIZE>;
pub(super) type TxStorage = StaTxEpoch<ControlTx>;
/// What the Wi-Fi client carries of the shared radio from its start to the
/// radio stop: its common power proof and its part in coexistence.
pub(super) struct WifiMembership {
    power: CommonPower<WifiClient>,
    coexisting: Coexisting<WifiClient>,
}
pub(super) type ProductionStationRuntime<'state> = StationRuntimeResources<
    'state,
    'static,
    WifiRoleOwner<EspHalWifiPlatform, WifiMembership>,
    MacInterruptEpoch,
    StationDmaResources<'static, RxStorage, RX_DESCRIPTOR_COUNT>,
    &'static mut TxStorage,
    ProductionStationBoardResources,
    SCAN_RECORD_CAPACITY,
>;
// Radio buffers remain in DMA-visible SRAM. The CPU-only scan table is a
// separate ordinary-memory owner, reserved together with these buffers before
// any cell is taken. Queue depths and diagnostic DMA allocations are unchanged.
oer_memory::zeroed_static! {
    /// Placement: the production Wi-Fi DMA owner stays in DMA-visible SRAM.
    static WIFI_MEMORY: DefaultWifiMemory<CriticalSectionRawMutex> =
        zeroed in ".dma.bss.open_radio_station";
}
static SCAN_MEMORY: DefaultScanMemory = DefaultScanMemory::new();

type StationMacClockStorage = oer_esp32s31_ieee80211_runtime::mac_clock::MacClockStorage<
    CriticalSectionRawMutex,
    oer_esp32s31_hal::root::MacLocalTime,
    EmbassyClock,
>;

/// The MAC clock of one radio start: the MAC local time with its relation
/// to the monotonic clock.
pub(crate) type StationMacClock = oer_esp32s31_ieee80211_runtime::mac_clock::MacClockHandle<
    'static,
    CriticalSectionRawMutex,
    oer_esp32s31_hal::root::MacLocalTime,
    EmbassyClock,
>;

/// The MAC clock's memory; every radio start ([`join_shared_radio`]) starts
/// a fresh clock in it.
static MAC_CLOCK: StationMacClockStorage = StationMacClockStorage::new(EmbassyClock);

/// TX and receive owners share this radio-start clock capability while their
/// waits continue through the monotonic timer.
pub(crate) type MacTimer =
    oer_esp32s31_ieee80211_runtime::mac_clock::ReceptionTimer<EmbassyClock, StationMacClock>;

/// A role's monotonic timer paired with the MAC clock.
pub(crate) const fn reception_timer(mac_clock: StationMacClock) -> MacTimer {
    oer_esp32s31_ieee80211_runtime::mac_clock::ReceptionTimer {
        timer: EmbassyClock,
        reception: mac_clock,
    }
}
// This immutable table binds every descriptor index to the final DMA buffer
// address. RX service validates it on every ownership transfer, so it is
// safety-critical hot state rather than bulk DMA storage. Keeping the 256-byte
// table in a separately named critical section prevents unrelated owner-graph
// layout changes from silently changing the authority used to rearm a buffer.
oer_memory::zeroed_static! {
    /// Placement: the RX DMA address binding stays in internal SRAM.
    static RX_BUFFER_ADDRESSES: oer_memory::zeroed::ZeroedStatic<
        RxDmaBufferAddresses<RX_DESCRIPTOR_COUNT>,
    > = zeroed in ".critical.bss.open_radio_rx_addresses";
}
// These two owners contain runtime-derived DMA and PHY values. StaticCell
// retains their final address while `init_with` below avoids a by-value
// intermediate during construction.
static TX_SLOT_STORAGE: StaticCell<TxSlot<TX_BUFFER_SIZE>> = StaticCell::new();
static TX_STATE: StaticCell<TxStorage> = StaticCell::new();
// Fifteen WPA2 peer state machines exceed the permitted cooperative task
// frame. They are CPU-only state (not DMA descriptors), so a separate normal
// static keeps them out of both task stacks and the DMA-only WIFI_MEMORY arena.
static AP_PEER_STORAGE: ConstStaticCell<oer_ieee80211_ap::AccessPointPeerStorage> =
    ConstStaticCell::new(oer_ieee80211_ap::AccessPointPeerStorage::new());
// Per-peer retry/sequence history is another AP-epoch table. Its address must
// stay stable while RX processing awaits IRQ and network work.
static AP_RX_DISPATCHER: StaticCell<oer_esp32s31_ieee80211_ap::rx::ApRxDispatcher> =
    StaticCell::new();
pub(super) static PRODUCTION_RX_BLOCK_ACK:
    oer_esp32s31_ieee80211_runtime::roles::concurrent::StaApRxBlockAck =
    match oer_esp32s31_ieee80211_runtime::roles::concurrent::StaApRxBlockAck::with_maximum_window(
        crate::resources::profile::ESP32S31_DEFAULT_RX_REORDER_WINDOW as u16,
    ) {
        Ok(sessions) => sessions,
        Err(_) => panic!("the production RX BlockAck window is statically validated"),
    };
// Software packet owners are retained across async polls, but never DMA-addressed.
static AP_TX_STORAGE: StaticCell<
    oer_esp32s31_ieee80211_runtime::roles::access_point::network_tx::AccessPointTxStorage<
        RadioNetworkTxBacking,
    >,
> = StaticCell::new();
static AP_AIRTIME_RESOURCES: StaticCell<AccessPointAirtimeResources> = StaticCell::new();
static AP_RX_REORDER: StaticCell<AccessPointRxReorder<'static, RX_BUFFER_SIZE>> = StaticCell::new();
#[cfg(feature = "diagnostics")]
static AP_OBSERVATION_STORAGE: StaticCell<
    oer_esp32s31_ieee80211_runtime::diagnostics::access_point::AccessPointObservationStorage,
> = StaticCell::new();
// Simultaneous STA+AP cannot borrow the station scan and Ethernet scratch.
// These are role-local CPU buffers; DMA never addresses them directly.
static AP_RX_FRAME: ConstStaticCell<[u8; RX_STAGE_CAPACITY]> =
    ConstStaticCell::new([0; RX_STAGE_CAPACITY]);
static AP_TX_FRAME: ConstStaticCell<[u8; RX_STAGE_CAPACITY]> =
    ConstStaticCell::new([0; RX_STAGE_CAPACITY]);
// Hardware pairwise-key capabilities are stable epoch state. Keeping their
// bounded table static avoids duplicating all 15 tokens in async rollback
// variants while `stop` still clears every hardware slot before returning it.
static AP_PAIRWISE_KEYS: ConstStaticCell<
    oer_esp32s31_ieee80211_ap::security::ApPairwiseKeyStorage,
> = ConstStaticCell::new(oer_esp32s31_ieee80211_ap::security::ApPairwiseKeyStorage::new());

#[derive(Clone, Copy, Debug, Default)]
struct ProductionScanObserver;

impl ScanFrameObserver for ProductionScanObserver {
    fn observe(&mut self, _frame: &[u8], _rssi: i8, _table_outcome: ScanObservation) {}
}

#[derive(Clone, Copy, Debug, Default)]
struct ProductionAttemptObserver;

impl StaAttemptObserver for ProductionAttemptObserver {
    fn stage_started(&mut self, stage: StaAttemptStage) {
        diagnostics_debug!("open-radio: attempt stage={stage:?} state=start");
    }

    fn stage_completed(&mut self, stage: StaAttemptStage) {
        diagnostics_debug!("open-radio: attempt stage={stage:?} state=complete");
    }
}

fn tx_entropy() -> u32 {
    Rng::new().random()
}

type ProductionStationPhase = StationServicePhase<
    RadioRuntimeOwner,
    ScanRx<'static, RX_DESCRIPTOR_COUNT, RX_BUFFER_SIZE, RX_BUFFER_STORAGE_SIZE>,
    ReceiveFrontier<'static, RX_DESCRIPTOR_COUNT, RX_BUFFER_SIZE>,
    WifiNetworkResources,
    ConnectedDisconnectedEpoch,
    ConnectedReconnectedEpoch,
    ProductionConnectedPhase,
>;

struct ProductionConnectedPhase {
    epoch: ConnectedStationEpoch,
    network: WifiNetworkResources,
    station: StaAttemptStation,
    peer: oer_esp32s31_ieee80211_sta::peer::ConnectedStaPeer,
    installed_security: oer_esp32s31_ieee80211_sta::attempt::StaInstalledSecurity,
}

type ProductionStationOwner<'state, 'security> =
    StationServiceOwner<'security, ProductionStationRuntime<'state>, ProductionStationPhase>;

struct ProductionStationFault<'state, 'security> {
    _connected: ConnectedStationFault<'state, 'security>,
    _station: StaAttemptStation,
}

impl StaFault for ProductionStationFault<'_, '_> {
    fn reason(&self) -> StaFaultReason {
        self._connected.reason()
    }
}

type ProductionStationStorage = StationStorageResources<
    'static,
    StationDmaResources<'static, RxStorage, RX_DESCRIPTOR_COUNT>,
    &'static mut TxStorage,
    SCAN_RECORD_CAPACITY,
>;
type ProductionStationStoppedPhase = StationStoppedPhaseResources<
    'static,
    ScanRx<'static, RX_DESCRIPTOR_COUNT, RX_BUFFER_SIZE, RX_BUFFER_STORAGE_SIZE>,
    ReceiveFrontier<'static, RX_DESCRIPTOR_COUNT, RX_BUFFER_SIZE>,
    WifiNetworkResources,
    RunningWifiNetwork,
    ConnectedParkedRx,
    RadioAmpduStorage,
    ConnectedControlStatics,
    ConnectedRxEpochResources,
>;

/// Exact role-local graph parked while role-neutral Wi-Fi is stopped.
///
/// The register route and Embassy wake domains remain here rather than being
/// reacquired from board singletons. `phase` and `security` describe an exact
/// resume of the stopped service; converting them into a fresh station request
/// is a separate, still-explicit normalization transaction.
struct ProductionWifiReusableResources<'security> {
    storage: ProductionStationStorage,
    board: ProductionStationBoardResources,
    phase: ProductionStationStoppedPhase,
    security: StaAttemptSecurity<'security>,
}

/// Physical radio frontier between logical Wi-Fi roles.
///
/// `Cold` exists only until the first IRQ-driven role activates the MAC
/// route. `Live` then carries that exact route, register owner and runtime
/// context across STA/AP/monitor/scan cutovers. A logical role stop therefore
/// cannot accidentally mask a still-running MAC or remap its interrupt.
enum ProductionWifiOwner {
    Cold(WifiStopped<EspHalWifiPlatform, WifiMembership>),
    Live {
        owner: WifiRoleOwner<EspHalWifiPlatform, WifiMembership>,
        registers: RadioRuntimeOwner,
        interrupts: MacInterruptEpoch,
    },
}

impl ProductionWifiOwner {
    fn current_channel(&self) -> WifiChannel {
        match self {
            Self::Cold(wifi) => wifi.current_channel(),
            Self::Live { owner, .. } => owner.current_channel(),
        }
    }
}

struct ProductionWifiMaterialized<L> {
    owner: WifiRoleOwner<EspHalWifiPlatform, WifiMembership>,
    registers: RadioRuntimeOwner,
    interrupts: MacInterruptEpoch,
    resources: L,
}

fn materialize_production_wifi<L>(
    wifi: ProductionWifiOwner,
    resources: L,
) -> ProductionWifiMaterialized<L> {
    match wifi {
        ProductionWifiOwner::Cold(wifi) => {
            let materialized = materialize_esp32s31_wifi_role(wifi, resources);
            ProductionWifiMaterialized {
                owner: materialized.owner,
                registers: materialized.registers,
                interrupts: mac_interrupt_epoch(materialized.interrupt_setup),
                resources: materialized.resources,
            }
        }
        ProductionWifiOwner::Live {
            owner,
            registers,
            interrupts,
        } => ProductionWifiMaterialized {
            owner,
            registers,
            interrupts,
            resources,
        },
    }
}

fn park_production_wifi(
    owner: WifiRoleOwner<EspHalWifiPlatform, WifiMembership>,
    registers: RadioRuntimeOwner,
    interrupts: MacInterruptEpoch,
) -> ProductionWifiOwner {
    if interrupts.is_active() {
        return ProductionWifiOwner::Live {
            owner,
            registers,
            interrupts,
        };
    }
    match interrupts.try_into_inactive_parts() {
        Ok((_route, setup, _mac_runtime, _power_runtime)) => {
            ProductionWifiOwner::Cold(owner.into_stopped(registers, setup, ()).wifi)
        }
        Err(interrupts) => ProductionWifiOwner::Live {
            owner,
            registers,
            interrupts,
        },
    }
}

struct ProductionStationStopped<'security> {
    wifi: ProductionWifiOwner,
    resources: ProductionWifiReusableResources<'security>,
}

struct ProductionWifiFreshResources {
    dma: StationDmaResources<'static, RxStorage, RX_DESCRIPTOR_COUNT>,
    rx_ring: Option<ProductionRxRing>,
    tx: ProductionOrdinaryTxResources,
    scan_table: &'static mut ScanTable,
    scan_frame: &'static mut [u8],
    ethernet: &'static mut [u8],
    network: WifiNetworkResources,
    board: ProductionStationBoardResources,
    station_address: [u8; 6],
}

/// The one ordinary descriptor before and after its first role epoch.
///
/// The slot is initialized exactly once. Later STA or AP epochs receive the
/// same reusable TX state; neither role allocates a shadow descriptor.
enum ProductionOrdinaryTxResources {
    Uninitialized(Pin<&'static mut TxSlot<TX_BUFFER_SIZE>>),
    Epoch(&'static mut TxStorage),
}

enum ProductionWifiStoppedResources {
    Fresh(ProductionWifiFreshResources),
    Returned(ProductionWifiReusableResources<'static>),
}

type ProductionRadioResources = (
    ProductionWifiPhysicalResources,
    ProductionStationRoleResources,
    ProductionAccessPointResources,
    ProductionMonitorResources,
);

/// Wi-Fi could not join the shared radio; the Wi-Fi owners it reached.
enum JoinFailure {
    Prepare {
        error: RadioPhyError,
        _partition: WifiPartition,
        _platform: EspHalWifiPlatform,
        _coex: CoexAbsent<WifiClient>,
    },
    PowerUp {
        _failure: WifiPowerUpFailure<EspHalWifiPlatform>,
        _coex: CoexLeft,
    },
    Start {
        _failure: RadioStartFailure<EspHalWifiPlatform>,
        _power: CommonPower<WifiClient>,
        _coex: CoexLeft,
    },
}

/// Coexistence left after a failed start, or still on.
type CoexLeft = Result<CoexAbsent<WifiClient>, CoexLeaveFailure<Coexisting<WifiClient>, CoexError>>;

enum ProductionRadioLifecycleFault {
    Join {
        _failure: JoinFailure,
        _resources: Option<ProductionRadioResources>,
    },
    ShutdownQuiesce {
        _failure: shutdown::Failure,
    },
    Release {
        _failure: WifiReleaseFailure<EspHalWifiPlatform>,
        _coex: CoexAbsent<WifiClient>,
        _resources: ProductionRadioResources,
    },
    RfClose {
        _error: ConcurrentRfError,
        _partition: WifiPartition,
        _platform: EspHalWifiPlatform,
        _coex: CoexAbsent<WifiClient>,
        _resources: ProductionRadioResources,
    },
    CoexDisable {
        _error: CoexError,
        _membership: WifiMembership,
        _wifi: WifiStopped<EspHalWifiPlatform, ()>,
        _resources: ProductionRadioResources,
    },
}

// A Wi-Fi bring-up or restart failure is terminal for this boot. Keep its
// exact owner graph outside the supervisor task frame so every ordinary role
// epoch does not pay for the largest start failure variant.
static RADIO_LIFECYCLE_FAULT: StaticCell<ProductionRadioLifecycleFault> = StaticCell::new();

/// Quiescent production frontier with physical hardware and each role's
/// storage represented by independent owners.
#[allow(clippy::result_large_err)]
type ProductionSupervisorStopped = WifiSupervisorStopped<
    ProductionWifiOwner,
    ProductionWifiPhysicalResources,
    ProductionStationRoleResources,
    ProductionAccessPointResources,
    ProductionMonitorResources,
>;

static RADIO_SUPERVISOR_CONTROL: WifiSupervisorControlResources<
    CriticalSectionRawMutex,
    RadioError,
> = WifiSupervisorControlResources::new();

struct ProductionWifiEpochRunner {
    radio: &'static SharedRadio,
    /// The MAC local time and its relation to the monotonic clock.
    /// The MAC clock of the current radio start.
    mac_clock: StationMacClock,
    watchdog: &'static crate::WatchdogConfig,
    trng: Trng,
    station_control: &'static StationControlResources<CriticalSectionRawMutex>,
    monitor_capture: &'static CaptureResources,
    radio_start: RadioStartConfig,
    rts_length_threshold: Option<crate::RtsLengthThreshold>,
}

/// Eternal supervisor for the Core0 radio ownership domain. Controlled child
/// execution returns the exact owner through the local datapath rendezvous
/// before quiescence; PAC, DMA and ISR capabilities never become independent
/// owners in another executor or a shared command channel.
pub struct SystemRunner {
    supervisor: RadioSupervisorTask<'static, CriticalSectionRawMutex, ProductionWifiEpochRunner>,
    connected_datapath: &'static station::ConnectedDatapathMailbox,
    radio: &'static SharedRadio,
}

impl SystemRunner {
    /// Run the Wi-Fi supervisor, with Wi-Fi's coexistence start callback as
    /// its own task.
    pub async fn run(self, spawner: Spawner) -> ! {
        self.connected_datapath.bind(spawner);
        spawner.spawn(
            wifi_coex_start_task(self.radio)
                .expect("the Wi-Fi coexistence start task is spawned once"),
        );
        await_stack_boundary!(self.supervisor.run())
    }
}

/// Wi-Fi's coexistence start callback for the lifetime of the radio.
#[embassy_executor::task]
async fn wifi_coex_start_task(radio: &'static SharedRadio) {
    oer_esp32s31_ieee80211_runtime::coex::run_wifi_coex_start(radio).await
}

enum ProductionStationReclaimFault<'security> {
    Runtime {
        _failure: StationRuntimeReclaimFailure<ProductionStationOwner<'static, 'security>>,
    },
    InterruptInvariant {
        _registers: RadioRuntimeOwner,
        _role: WifiRoleOwner<EspHalWifiPlatform, WifiMembership>,
        _interrupt: MacInterruptEpoch,
        _storage: ProductionStationStorage,
        _board: ProductionStationBoardResources,
        _phase: ProductionStationStoppedPhase,
        _security: StaAttemptSecurity<'security>,
        _selected_channel: Option<WifiChannel>,
    },
}

enum ProductionWifiFault {
    PairedStationPhase {
        _owner: ProductionStationOwner<'static, 'static>,
        _runner: ProductionStationRunner<'static, 'static>,
    },
    PairedConnected {
        _fault: ConnectedStationFault<'static, 'static>,
        _station: StaAttemptStation,
        _access_point: ProductionAccessPointResources,
        _monitor: ProductionMonitorResources,
    },
    PairedChannelMismatch {
        _started: crate::supervisor::station::ConnectedNetworkStarted<'static, 'static>,
        _station: StaAttemptStation,
        _access_point: ProductionAccessPointResources,
        _monitor: ProductionMonitorResources,
    },
    PairedSecurityMismatch {
        _started: crate::supervisor::station::ConnectedNetworkStarted<'static, 'static>,
        _station: StaAttemptStation,
        _access_point: ProductionAccessPointResources,
        _monitor: ProductionMonitorResources,
        _access_point_request: AccessPointRequest,
    },
    PairedReclaim {
        _station: ProductionStationReclaimFault<'static>,
        _access_point: ProductionAccessPointResources,
        _monitor: ProductionMonitorResources,
    },
    PairedStopped {
        _stopped: ProductionSupervisorStopped,
    },
    StandaloneScanInitialRx {
        _owner: WifiRoleOwner<EspHalWifiPlatform, WifiMembership>,
        _registers: RadioRuntimeOwner,
        _interrupts: MacInterruptEpoch,
        _physical: ProductionWifiPhysicalResources,
        _station: ProductionStationRoleResources,
        _access_point: ProductionAccessPointResources,
        _monitor: ProductionMonitorResources,
        _error: RxRingError,
    },
    StandaloneScanReturn {
        _fault: ProductionStandaloneScanReturnFault,
    },
    AccessPointPreparation {
        _fault: ProductionAccessPointPreparationFault,
    },
    AccessPointRuntime {
        _task: ProductionAccessPointTask,
    },
    AccessPointTeardown {
        _fault: ProductionAccessPointTeardownFault,
    },
    Station {
        _fault: ProductionStationFault<'static, 'static>,
        _runner: ProductionStationRunner<'static, 'static>,
    },
    Reclaim {
        _station: ProductionStationReclaimFault<'static>,
        _runner: ProductionStationRunner<'static, 'static>,
    },
    TaskPreparation {
        _failure: StationPrepareFailure<
            ProductionStationOwner<'static, 'static>,
            ProductionStationRunner<'static, 'static>,
        >,
    },
    Resume {
        _fault: ProductionStationResumeFault,
    },
    InitialRx {
        _fault: ProductionInitialRxFault,
    },
    MonitorBuild {
        _failure: ProductionMonitorBuildFailure,
        _physical: ProductionWifiPhysicalResources,
        _station: ProductionStationRoleResources,
    },
    MonitorChannel {
        _error: MonitorChannelSwitchError,
        _task: ProductionMonitorTask,
        _physical: ProductionWifiPhysicalResources,
        _station: ProductionStationRoleResources,
    },
    MonitorRuntime {
        _task: ProductionMonitorTask,
        _physical: ProductionWifiPhysicalResources,
        _station: ProductionStationRoleResources,
    },
    StoppedOwner {
        _wifi: ProductionWifiOwner,
        _resources: ProductionWifiStoppedResources,
        _access_point: ProductionAccessPointResources,
        _monitor: ProductionMonitorResources,
    },
}

impl ProductionWifiFault {
    /// The portable reason of a fault that ends the station role, whether it
    /// ran alone or beside the access point; `None` for a fault of another
    /// role. Every faulted role epoch passes here before its owners are
    /// quarantined, so this is the one place the station link learns it.
    fn station_fault_reason(&self) -> Option<StaFaultReason> {
        use StaFaultCause::{Configuration, Hardware, Resources, Security};
        use StaFaultPhase::{Connected, Start, Teardown};
        let (phase, cause) = match self {
            Self::Station { _fault: fault, .. } => return Some(fault.reason()),
            Self::PairedConnected { _fault: fault, .. } => return Some(fault.reason()),
            Self::TaskPreparation { .. } => (Start, Resources),
            Self::InitialRx { .. } | Self::Resume { .. } | Self::PairedStationPhase { .. } => {
                (Start, Hardware)
            }
            Self::PairedChannelMismatch { .. } => (Start, Configuration),
            Self::PairedSecurityMismatch { .. } => (Start, Security),
            Self::PairedStopped { .. } => (Connected, Hardware),
            Self::Reclaim { .. } | Self::PairedReclaim { .. } => (Teardown, Hardware),
            Self::StoppedOwner { .. } => (Teardown, Resources),
            Self::StandaloneScanInitialRx { .. }
            | Self::StandaloneScanReturn { .. }
            | Self::AccessPointPreparation { .. }
            | Self::AccessPointRuntime { .. }
            | Self::AccessPointTeardown { .. }
            | Self::MonitorBuild { .. }
            | Self::MonitorChannel { .. }
            | Self::MonitorRuntime { .. } => return None,
        };
        Some(StaFaultReason::new(phase, cause))
    }
}

enum ProductionStandaloneScanReturnFault {
    TxRestore {
        _owner: WifiRoleOwner<EspHalWifiPlatform, WifiMembership>,
        _registers: RadioRuntimeOwner,
        _interrupts: MacInterruptEpoch,
        _dma: StationDmaResources<'static, RxStorage, RX_DESCRIPTOR_COUNT>,
        _receive: ScanRx<'static, RX_DESCRIPTOR_COUNT, RX_BUFFER_SIZE, RX_BUFFER_STORAGE_SIZE>,
        _tx_epoch: &'static mut TxStorage,
        _aggregate_tx: RadioAmpduStorage,
        _station: ProductionStationRoleResources,
        _access_point: ProductionAccessPointResources,
        _monitor: ProductionMonitorResources,
        _error: oer_esp32s31_ieee80211_sta::tx_epoch::StaTxEpochError,
        _returned_control: ControlTx,
    },
    ReceiveNotLive {
        _owner: WifiRoleOwner<EspHalWifiPlatform, WifiMembership>,
        _registers: RadioRuntimeOwner,
        _interrupts: MacInterruptEpoch,
        _dma: StationDmaResources<'static, RxStorage, RX_DESCRIPTOR_COUNT>,
        _receive: ScanRx<'static, RX_DESCRIPTOR_COUNT, RX_BUFFER_SIZE, RX_BUFFER_STORAGE_SIZE>,
        _tx_epoch: &'static mut TxStorage,
        _aggregate_tx: RadioAmpduStorage,
        _station: ProductionStationRoleResources,
        _access_point: ProductionAccessPointResources,
        _monitor: ProductionMonitorResources,
    },
}

struct ProductionInitialRxFault {
    _error: RxRingError,
    _owner: WifiRoleOwner<EspHalWifiPlatform, WifiMembership>,
    _registers: RadioRuntimeOwner,
    _interrupts: MacInterruptEpoch,
    _dma: StationDmaResources<'static, RxStorage, RX_DESCRIPTOR_COUNT>,
    _tx_storage: &'static mut TxStorage,
    _scan_table: &'static mut ScanTable,
    _scan_frame: &'static mut [u8],
    _ethernet: &'static mut [u8],
    _network: WifiNetworkResources,
    _board: ProductionStationBoardResources,
    _station_address: [u8; 6],
    _security: StaAttemptSecurity<'static>,
}

struct ProductionStationResumeFault {
    _owner: WifiRoleOwner<EspHalWifiPlatform, WifiMembership>,
    _registers: RadioRuntimeOwner,
    _interrupts: MacInterruptEpoch,
    _storage: ProductionStationStorage,
    _board: ProductionStationBoardResources,
    _phase: ProductionStationStoppedPhase,
    _previous_security: StaAttemptSecurity<'static>,
    _requested_security: StaAttemptSecurity<'static>,
}

/// Recover role-neutral Wi-Fi only from a clean finite station exit.
///
/// Any contradiction retains the exact owner set in a fault frontier. It
/// never acknowledges stop by manufacturing PAC, DMA or interrupt capabilities.
fn try_reclaim_production_station<'security>(
    owner: ProductionStationOwner<'static, 'security>,
) -> Result<ProductionStationStopped<'security>, ProductionStationReclaimFault<'security>> {
    let reclaimed = match try_reclaim_esp32s31_station_runtime(owner) {
        Ok(reclaimed) => reclaimed,
        Err(failure) => {
            return Err(ProductionStationReclaimFault::Runtime { _failure: failure });
        }
    };
    let (registers, mut role, interrupt, storage, board, phase, security, primary_channel) =
        reclaimed.into_parts();
    let selected_channel = primary_channel;
    if let Some(channel) = selected_channel {
        role.set_current_channel(channel);
    }
    let wifi = if interrupt.is_active() {
        ProductionWifiOwner::Live {
            owner: role,
            registers,
            interrupts: interrupt,
        }
    } else {
        let (_route, interrupt_setup, _mac_runtime, _power_runtime) =
            match interrupt.try_into_inactive_parts() {
                Ok(parts) => parts,
                Err(interrupt) => {
                    return Err(ProductionStationReclaimFault::InterruptInvariant {
                        _registers: registers,
                        _role: role,
                        _interrupt: interrupt,
                        _storage: storage,
                        _board: board,
                        _phase: phase,
                        _security: security,
                        _selected_channel: selected_channel,
                    });
                }
            };
        ProductionWifiOwner::Cold(role.into_stopped(registers, interrupt_setup, ()).wifi)
    };
    Ok(ProductionStationStopped {
        wifi,
        resources: ProductionWifiReusableResources {
            storage,
            board,
            phase,
            security,
        },
    })
}

// The board moves one affine reference through role transitions, not the
// model callbacks or ledger. Both stay together in stable CPU storage.
pub(super) struct AccessPointAirtimeResources {
    configuration:
        oer_esp32s31_ieee80211_runtime::roles::access_point::network_tx::AccessPointAirtimeConfiguration,
    storage: oer_esp32s31_ieee80211_runtime::roles::access_point::network_tx::AccessPointAirtimeStorage,
}

pub(super) struct ProductionStationBoardResources {
    pub(super) access_point_airtime: Option<&'static mut AccessPointAirtimeResources>,
    pub(super) interface: BoundVirtualInterface,
    pub(super) connected_datapath: &'static station::ConnectedDatapathMailbox,
    pub(super) rx_protocol_runtime: &'static mut ConnectedRxProtocolStorage,
    pub(super) sta_ap_rx_batch: &'static mut [u8],
    pub(super) initial_connected: Option<InitialConnectedStaticResources>,
    #[cfg(feature = "diagnostics")]
    pub(super) diagnostics: Option<crate::DiagnosticObservers>,
}

mod station_epoch;

use station_epoch::{
    ProductionStationExit, ProductionStationMode, ProductionStationRunner,
    production_station_runtime, restore_production_station_frontier,
};

mod role_dispatch;

/// Bring Wi-Fi up as a client of the shared radio and materialize the public
/// controller, persistent network device and sole owner-holding runner. This
/// function does not start a Wi-Fi role and does not construct an IP stack.
///
/// The first client to join the shared radio registers its PHY domain; the
/// returned [`WifiInitialization::phy`] reports what this bring-up did.
#[allow(
    large_assignments,
    reason = "radio start returns one unique typed owner graph; the post-LTO stack gate and runtime stack painting check its stack use"
)]
pub async fn new(
    radio: &'static SharedRadio,
    partition: WifiPartition,
    coex: CoexAbsent<WifiClient>,
    platform: EspHalWifiPlatform,
    trng: Trng,
    config: crate::RadioConfig,
) -> Result<WifiStarted, NewError> {
    diagnostics_event!("open-radio: Wi-Fi start on the shared radio");

    let crate::RadioConfig {
        #[cfg(feature = "rx-ownership-observation")]
        rx_ownership_observer,
        watchdog,
        access_point_airtime,
        station_mac,
        access_point_mac,
        initial_channel,
        maximum_tx_power_quarter_dbm,
        rts_length_threshold,
        #[cfg(feature = "connected-datapath-cycle-telemetry")]
        connected_datapath_poll_observer,
        #[cfg(feature = "diagnostics")]
        diagnostics,
    } = config;
    #[cfg(feature = "mac-irq-diagnostics")]
    if let Some(hooks) = diagnostics {
        configure_mac_irq_observer(hooks.mac_irq);
    }
    let mut wifi_start = Esp32s31WifiStartConfig::new(initial_channel);
    if let Some(maximum) = maximum_tx_power_quarter_dbm {
        wifi_start = wifi_start.with_maximum_tx_power_quarter_dbm(maximum);
    }
    let radio_start = RadioStartConfig::new(
        wifi_start,
        WifiMacStartConfig::new(MAC_HANDSHAKE_SAMPLE_LIMIT, station_mac, access_point_mac),
    );
    let protection = watchdog.startup();
    let started = await_stack_boundary!(join_shared_radio(
        radio,
        partition,
        platform,
        coex,
        radio_start
    ));
    crate::WatchdogConfig::complete(protection);
    let (phy, wifi, mac_clock) = started.map_err(|failure| {
        let error = match &failure {
            JoinFailure::PowerUp { .. } => NewError::RadioStart,
            JoinFailure::Prepare { error, .. } => NewError::Phy(*error),
            JoinFailure::Start { .. } => NewError::RadioStart,
        };
        RADIO_LIFECYCLE_FAULT.init(ProductionRadioLifecycleFault::Join {
            _failure: failure,
            _resources: None,
        });
        error
    })?;
    let station_interface = WifiConfig::station(WifiStationConfig::new(station_mac))
        .validate(oer_esp32s31_ieee80211_mac::capabilities::ESP32S31_MAC_SERVICE_CAPABILITIES)
        .map_err(|_| NewError::StationRole)?
        .station()
        .ok_or(NewError::StationRole)?;
    let station_address = station_interface.interface.address;
    let initialization = WifiInitialization {
        phy,
        start: wifi.start_report(),
        transition: wifi.transition_report(),
    };
    diagnostics_event!(
        "open-radio: Wi-Fi ready on the shared radio, initial_tracking={} tracking_inhibited={}",
        initialization.start.wifi.initial_tracking.is_some(),
        initialization
            .start
            .wifi
            .initial_tracking
            .is_some_and(|outcome| outcome.tracking_inhibited)
    );

    let memory = match WIFI_MEMORY.claim(&SCAN_MEMORY) {
        Ok(memory) => memory,
        Err(_error) => return Err(NewError::StationMemoryInUse),
    };
    #[cfg(feature = "rx-ownership-observation")]
    if let Some(observer) = rx_ownership_observer {
        memory.rx_dma.set_ownership_observer(observer);
    }
    let rx_storage: &'static RxStorage = memory.rx_dma;
    let buffer_addresses = RX_BUFFER_ADDRESSES.take();
    let monitor_memory =
        MonitorMemory::new(rx_storage, buffer_addresses).map_err(|_| NewError::RxDmaLayout)?;
    let descriptor_base = monitor_memory.descriptor_base();
    let tx_dma = oer_esp32s31_ieee80211_dma::tx_storage::TxDmaStorage::pin_static(memory.tx_dma)
        .map_err(|_| NewError::TxDmaLayout)?;
    let tx_slot = Pin::static_mut(TX_SLOT_STORAGE.init_with(|| TxSlot::from_dma(tx_dma)));
    diagnostics_event!(
        "open-radio: MAC ready handshake_samples={} interrupt_mask={:?}",
        initialization.start.mac.handshake_samples,
        initialization.transition.cold_interrupt_mask
    );

    let scan_table = memory.scan_table;
    scan_table.clear();
    let scan_frame = memory.scan_frame;

    let initial_connected = match initialize_connected_static_resources() {
        Ok(resources) => resources,
        Err(_error) => return Err(NewError::ConnectedResources),
    };
    #[cfg(feature = "diagnostics")]
    let diagnostics_snapshot = initial_connected.diagnostics;
    let initial_connected = initial_connected.resources;
    let (network_devices, station_network) =
        crate::radio_resources::initialize_network(station_address, access_point_mac.bytes());
    let monitor = initialize_monitor_resources(monitor_memory)
        .map_err(|MonitorResourcesError::InUse| NewError::MonitorResources)?;
    let connected_datapath = initialize_connected_datapath_mailbox(
        #[cfg(feature = "connected-datapath-cycle-telemetry")]
        connected_datapath_poll_observer,
    );
    let initial_resources = ProductionWifiStoppedResources::Fresh(ProductionWifiFreshResources {
        dma: StationDmaResources::new(
            monitor_memory.storage(),
            descriptor_base,
            monitor_memory.buffer_addresses(),
        ),
        rx_ring: None,
        tx: ProductionOrdinaryTxResources::Uninitialized(tx_slot),
        scan_table,
        scan_frame,
        ethernet: initialize_ethernet_frame(),
        network: station_network,
        board: ProductionStationBoardResources {
            access_point_airtime: access_point_airtime.map(|configuration| AP_AIRTIME_RESOURCES.init_with(|| AccessPointAirtimeResources {
                storage: oer_esp32s31_ieee80211_runtime::roles::access_point::network_tx::AccessPointAirtimeStorage::new(configuration.quantum_micros).with_observer(configuration.observer),
                configuration,
            })),
            interface: station_interface,
            connected_datapath,
            rx_protocol_runtime: initialize_connected_rx_protocol_runtime(),
            sta_ap_rx_batch: initialize_sta_ap_station_rx_batch(),
            initial_connected: Some(initial_connected),
            #[cfg(feature = "diagnostics")]
            diagnostics,
        },
        station_address,
    });
    let (physical, station) = match try_split_wifi_stopped_resources(initial_resources) {
        Ok(resources) => resources,
        Err(_) => unreachable!("fresh Wi-Fi resources are always splittable"),
    };
    let stopped = WifiSupervisorStopped::new(
        ProductionWifiOwner::Cold(wifi),
        physical,
        station,
        ProductionAccessPointResources {
            tx_storage: AP_TX_STORAGE.init_with(oer_esp32s31_ieee80211_runtime::roles::access_point::network_tx::AccessPointTxStorage::new),
            address: access_point_mac.bytes(),
            beacon: memory.ap_beacon,
            rx_frame: AP_RX_FRAME.take().as_mut_slice(),
            tx_frame: AP_TX_FRAME.take().as_mut_slice(),
            peer_storage: AP_PEER_STORAGE.take(),
            pairwise_storage: AP_PAIRWISE_KEYS.take(),
            rx_dispatcher: AP_RX_DISPATCHER.init_with(|| {
                oer_esp32s31_ieee80211_ap::rx::ApRxDispatcher::new(
                    oer_esp32s31_ieee80211_ap::rx::ApRxConfig {
                        access_point: access_point_mac.bytes(),
                        ingress: oer_esp32s31_ieee80211_mac::rx::RxIngressConfig {
                            ring_entry_limit: 1,
                            csi_config: 0,
                            flags: 0,
                        },
                        security:
                            oer_ieee80211_mac::security::LinkProtection::Ccmp,
                    },
                )
            }),
            rx_block_ack: &PRODUCTION_RX_BLOCK_ACK,
            rx_reorder: AP_RX_REORDER.init_with(AccessPointRxReorder::new),
            rx_reorder_storage: &crate::supervisor::station::RX_REORDER_STORAGE,
            #[cfg(feature = "diagnostics")]
            observation_storage: AP_OBSERVATION_STORAGE.init_with(Default::default),
        },
        monitor.role,
    );
    let configuration = WifiSupervisorConfiguration::new(
        oer_esp32s31_ieee80211_mac::capabilities::ESP32S31_MAC_SERVICE_CAPABILITIES,
    )
    .with_station(WifiStationConfig::new(station_mac))
    .with_access_point(WifiAccessPointConfig::new(access_point_mac))
    .with_standalone_scan()
    .with_standalone_monitor();
    let (controller, supervisor) = match prepare_esp32s31_radio_supervisor(
        &RADIO_SUPERVISOR_CONTROL,
        configuration,
        ProductionWifiEpochRunner {
            radio,
            mac_clock,
            watchdog,
            trng,
            // The control storage itself is reusable after a clean station
            // epoch. Only its static reference is acquired once; each epoch
            // leases fresh controller/task endpoints through `split()`.
            station_control: memory.station_control,
            monitor_capture: monitor.capture,
            radio_start,
            rts_length_threshold,
        },
        stopped,
    ) {
        Ok(prepared) => prepared,
        Err(_failure) => return Err(NewError::SupervisorInUse),
    };
    Ok(WifiStarted {
        wifi: WifiSystem::new(
            controller.into_wifi(),
            network_devices,
            monitor.frames,
            #[cfg(feature = "diagnostics")]
            diagnostics_snapshot,
        ),
        initialization,
        runner: SystemRunner {
            supervisor,
            connected_datapath,
            radio,
        },
    })
}

/// Bring Wi-Fi up on the shared radio under one arbiter lease: prepare the
/// shared PHY (registration, or wake of closed RF), enable coexistence,
/// which `wifi_hw_start` enables right after `esp_phy_enable`, then enter
/// common power and start Wi-Fi's PHY client and MAC. After a full
/// common-power cycle Wi-Fi works again only when the RF wake and
/// coexistence precede the repeated power entry (). The MAC clock starts
/// fresh on the partition's MAC local time. A failure returns the Wi-Fi
/// owners it reached, after the shared-PHY terminal policy was applied to
/// it.
async fn join_shared_radio(
    radio: &'static SharedRadio,
    partition: WifiPartition,
    platform: EspHalWifiPlatform,
    coex: CoexAbsent<WifiClient>,
    radio_start: RadioStartConfig,
) -> Result<
    (
        oer_esp32s31_radio_runtime::RadioPhyPrepared,
        WifiStopped<EspHalWifiPlatform, WifiMembership>,
        StationMacClock,
    ),
    JoinFailure,
> {
    let mac_clock = MAC_CLOCK.start(partition.mac_local_time());
    let mut guard = radio.lock().await;
    let phy = match await_stack_boundary!(guard.prepare_phy()) {
        Ok(phy) => phy,
        Err(error) => {
            let failure = JoinFailure::Prepare {
                error,
                _partition: partition,
                _platform: platform,
                _coex: coex,
            };
            crate::lifecycle_policy::enforce(
                &failure,
                SharedPhyFailStop::from_ambiguous_lifecycle(error.started()),
            );
            return Err(failure);
        }
    };
    let coexisting = guard.enable_coex(coex);
    let (lease, _, clocks) = guard.parts();
    let (initialized, power) = match power_up_esp32s31_wifi(lease, clocks, partition, platform) {
        Ok(powered) => powered,
        Err(failure) => {
            let failure = JoinFailure::PowerUp {
                _failure: failure,
                _coex: guard.disable_coex(coexisting),
            };
            crate::lifecycle_policy::enforce(
                &failure,
                SharedPhyFailStop::from_ambiguous_lifecycle(false),
            );
            return Err(failure);
        }
    };
    let (lease, radio_platform, _) = guard.parts();
    match await_stack_boundary!(start_esp32s31_radio::<_, _, RomShortDelay, _>(
        lease,
        radio_platform,
        initialized,
        radio_start,
        NoopPhyTargetObserver,
        &EmbassyClock,
    )) {
        Ok(wifi) => Ok((
            phy,
            wifi.with_membership(WifiMembership { power, coexisting }),
            mac_clock,
        )),
        Err(failure) => {
            let ambiguous = failure.phy_hardware_ambiguous();
            let failure = JoinFailure::Start {
                _failure: failure,
                _power: power,
                _coex: guard.disable_coex(coexisting),
            };
            crate::lifecycle_policy::enforce(
                &failure,
                SharedPhyFailStop::from_ambiguous_lifecycle(ambiguous),
            );
            Err(failure)
        }
    }
}

/// Run a synchronous owner transfer in its own stack frame.
///
/// Fat LTO inlines every by-value owner move of a long async function into
/// its one poll frame, whose stack budget the runtime audit bounds. A
/// non-inlined call gives the transfer a frame that exists only while it runs.
#[inline(never)]
pub(super) fn outside_poll_frame<R>(transfer: impl FnOnce() -> R) -> R {
    transfer()
}
