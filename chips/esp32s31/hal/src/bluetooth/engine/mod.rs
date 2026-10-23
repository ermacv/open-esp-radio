//! Controller-SRAM codecs and the CPU/controller handover protocol.
//!
//! One Engine retains the register token and complete placed arena; private
//! components bind the static graphs within it.
//! The driver selects role profiles, phases, channels and timing as detached
//! values; HAL alone encodes fields and checks graph binding anchors.
//! Generative pool/epoch scopes bind affine leases; logical item identities
//! distinguish repeated acquisitions of a slot. Address publication goes
//! through the graph owner and never exports a controller address.
//!
//! The owning engine moves through init, activation, workers, stop and reset.
//! Only verified reset returns the whole allocation; terminal failures retain
//! the registers and arena together in absorbing quarantine. The scheduler's
//! physical protocol and vendor transaction order live here. The P2 task
//! half requests fresh samples and IRQ-bank changes from the stable P3 half
//! through an epoch-scoped static request/reply channel and radio doorbell.

#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]
#![allow(
    dead_code,
    reason = "private controller-memory protocols are exercised by graph and hardware-backend tests"
)]

mod access;
mod finished;
pub use finished::{
    CompletionError, EngineFinishedHardwareListObserved, EngineFinishedListObservation,
    EngineFinishedListPop,
};
mod arena;
mod ble_phy_engine;
pub use access::{EnginePrepareError, EngineReceiveError};
#[cfg(target_arch = "riscv32")]
mod live;
mod owner;
pub use arena::{ArenaError, ArenaMemory, BluetoothArena, ControllerMemory, Engine, EngineFault};
#[cfg(target_arch = "riscv32")]
pub use live::{EngineScheduleError, EngineStopError, ReceivePublicationError};
pub use owner::{EngineInitError, EngineShutDown, EngineShutdownError, EngineShutdownFailure};
mod coexistence;
mod connectable_advertising;
mod direction_finding_workspace;
mod dtm;
mod dtm_image;
mod dtm_rx_result;
mod le_device_table;
mod le_phy_packet;
mod le_rx_chain;
mod le_rx_packet;
mod le_tx_packet;
mod le_tx_power;
pub use le_tx_power::LeTxPower;
mod layout_reference;
mod lease;
pub use lease::{EpochScope, PoolScope, with_epoch, with_epoch_async, with_pool, with_pool_async};
mod advertising_image;
mod legacy_advertising;
mod legacy_advertising_tx_packet;
mod legacy_scanning;
mod peripheral_connection;
mod placement;
mod rx_memory_list;
mod rx_packet_bytes;
mod scanning_image;
pub mod scheduler;
mod scheduler_context;
mod scheduler_item;
mod scheduler_pool;
mod shared_layout;
mod sram_link;
#[cfg(target_arch = "riscv32")]
pub use placement::ChipDma;
pub use placement::ControllerDmaSpace;
#[cfg(not(target_arch = "riscv32"))]
pub use placement::HostDma;
pub(crate) use placement::Placement;

pub use coexistence::{
    AdvertisingCoexistencePriorities, ConnectionCoexistencePriorities,
    LegacyScanCoexistencePriorities, PeripheralConnectionCoexistenceProtection,
    SchedulerItemCoexistencePriority,
};
pub use scheduler_item::{SCHEDULER_ITEM_UNEXECUTED, SchedulerItemCompletionStatus};

pub(crate) use scheduler_pool::SchedulerItemSources;

pub use scheduler_pool::{
    SchedulerAllocationConfig, SchedulerAllocationNumbers, SchedulerItemId, SchedulerItemSpace,
    SchedulerPoolError, SchedulerPoolPlaceError, SchedulerRoleInstance, SchedulerRoleKind,
    SchedulerRolePool, SchedulerRolePoolStorage, SchedulerRoleReleaseFailure,
};

pub(crate) use ble_phy_engine::{BlePhyEngineCpuOwned, BlePhyEngineStorage};

pub use connectable_advertising::{
    LegacyConnectableAdvIndPacketInput, LegacyConnectableAdvertisingError,
    LegacyConnectableAdvertisingMemoryInput, LegacyConnectableAdvertisingOwnAddress,
    LegacyConnectableAdvertisingPduFitError, LegacyConnectableAdvertisingPool,
    LegacyConnectableAdvertisingStorage, LegacyConnectableScanResponsePacketInput,
};

pub(crate) use direction_finding_workspace::DirectionFindingWorkspaceLink;
pub(crate) use direction_finding_workspace::{
    DirectionFindingWorkspaceCpuOwned, DirectionFindingWorkspaceStorage,
};

pub use dtm_image::{
    DtmItemConfiguration, DtmLinkStateReviewedWords, DtmPositionalEventWords, DtmReceiveWait,
    DtmRxHeaderTailProjection, DtmSchedulerFrequency, DtmSchedulerItemReviewedWords,
    DtmTxHeaderHeadProjection,
};

pub use dtm_rx_result::{DtmRxResultProjection, DtmRxResultProjectionError, DtmRxRssi};

pub use dtm::{
    BLUETOOTH_DTM_LINK_STATE_BYTES, BLUETOOTH_DTM_MAX_PACKET_CAPACITY,
    BLUETOOTH_DTM_RX_PACKET_BYTES, BLUETOOTH_DTM_RX_PACKET_PREFIX_BYTES,
    BLUETOOTH_DTM_SCHEDULER_ITEM_BYTES, BLUETOOTH_DTM_TX_PACKET_BYTES, DtmError, DtmEventResult,
    DtmPool, DtmPositionalEventSeed, DtmPrepareError, DtmRxRotationError,
    DtmSchedulerItemCompletionStatus, DtmStorage, DtmTxPacketPrepareError,
};

pub use le_device_table::{
    BLUETOOTH_FILTER_ACCEPT_LIST_CAPACITY, LeDeviceTable, LeDeviceTableError, LeDeviceTableStorage,
    LeFilterAcceptListDevice,
};
pub use le_rx_chain::{
    LeRxChain, LeRxChainError, LeRxChainPlaceError, LeRxChainStorage, LeRxSource, LeRxTag,
};

pub use le_rx_packet::{
    BLUETOOTH_LE_RX_PAYLOAD_CAPACITY, LePacketCapturedTime, LeReceivedPdu, LeRxError, LeRxOutcome,
};

pub use le_tx_packet::{
    BLUETOOTH_LE_BUFFER_HEADER_BYTES, BLUETOOTH_LE_TX_PACKET_PREFIX_BYTES, LeTxPacketPrepareError,
    LeTxPacketPreparedLength, LeTxPacketStorage,
};

pub use advertising_image::{
    AdvertisingEventImage, AdvertisingItemImage, LegacyAdvertisingOwnAddress,
    LegacyAdvertisingPduError, LegacyAdvertisingPrimaryChannel, SchedulerEventPriority,
};

pub use legacy_advertising::{
    BLUETOOTH_LEGACY_ADVERTISING_LINK_STATE_BYTES, BLUETOOTH_LEGACY_ADVERTISING_MAX_PAYLOAD_BYTES,
    BLUETOOTH_LEGACY_ADVERTISING_SCHEDULER_ITEM_BYTES,
    BLUETOOTH_LEGACY_ADVERTISING_SCHEDULER_ITEM_CAPACITY,
    BLUETOOTH_LEGACY_ADVERTISING_TX_PACKET_BYTES, LegacyAdvertisingError, LegacyAdvertisingEvent,
    LegacyAdvertisingPool, LegacyAdvertisingStorage,
};

pub use scanning_image::{
    LegacyScanPrimaryChannel, LegacyScanSchedulerWindow, LegacyScanWindowTicks, ScanItemImage,
    ScanItemKind, ScanResetImage,
};

pub use legacy_scanning::{
    BLUETOOTH_LEGACY_SCAN_SCHEDULER_ITEM_COUNT, LegacyScanError, LegacyScanEvent,
    LegacyScanEventTiming, LegacyScanPool, LegacyScanStorage,
};

pub use peripheral_connection::{
    BLUETOOTH_PERIPHERAL_CONNECTION_LINK_STATE_BYTES, BLUETOOTH_PERIPHERAL_CONNECTION_RX_PACKETS,
    BLUETOOTH_PERIPHERAL_CONNECTION_SCHEDULER_ITEM_BYTES,
    BLUETOOTH_PERIPHERAL_CONNECTION_SCHEDULER_ITEM_COUNT,
    BLUETOOTH_PERIPHERAL_CONNECTION_TX_SENTINEL_BYTES,
    PeripheralConnectionCapturedAnchorAvailability, PeripheralConnectionCapturedAnchorTime,
    PeripheralConnectionDataChannel, PeripheralConnectionError, PeripheralConnectionEvent,
    PeripheralConnectionEventResult, PeripheralConnectionEventSpan, PeripheralConnectionFirstEvent,
    PeripheralConnectionIdentity, PeripheralConnectionPool, PeripheralConnectionReceiveTime,
    PeripheralConnectionReceiveWait, PeripheralConnectionRecurringEvent,
    PeripheralConnectionRecurringReceiveWait, PeripheralConnectionSchedulerItemCompletionStatus,
    PeripheralConnectionSchedulerPriority, PeripheralConnectionSchedulerWindow,
    PeripheralConnectionStorage, PeripheralConnectionTransmitPduKind,
};

pub use rx_memory_list::RxMemoryListClass;

pub use scheduler_context::{BLUETOOTH_SCHEDULER_CONTEXT_BYTES, SchedulerContextStorage};

pub(crate) use sram_link::ControllerSramLinkAddress;

#[cfg(test)]
mod test_images;

pub use ble_phy_engine::BlePhyLe1MPacketStartCalibration;
