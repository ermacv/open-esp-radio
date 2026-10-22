//! Controller registers, placed arena and the CPU/controller handover protocol.
//!
//! `Engine` retains both the register token and `BluetoothArena`. Physical
//! links, phased layout access, list publications and completion return stay
//! here. Driver role builders supply detached values through typed operations.
//! Generative pool/epoch scopes bind affine instance leases; item identities
//! additionally distinguish acquisitions of a reused slot.
//!
//! The scheduler's hardware protocol lock and vendor transaction order are
//! retained. Stable interrupt-owner lending is internal to HAL under the
//! current Sources exception.

#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]
#![cfg_attr(
    not(target_arch = "riscv32"),
    allow(
        dead_code,
        reason = "native builds exercise the physical protocol through its test backends"
    )
)]

mod arena;
mod ble_phy_engine;
pub use arena::{ArenaError, BluetoothArena, Engine, EngineBindFailure, EngineRegisters};
#[cfg(target_arch = "riscv32")]
mod live;
#[cfg(target_arch = "riscv32")]
pub use live::{CompletionError, EngineScheduleError, ReceivePublicationError};
mod coexistence;
mod connectable_advertising;
mod direction_finding_workspace;
mod dtm;
mod dtm_event_image;
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
mod legacy_advertising;
mod legacy_advertising_event_image;
mod legacy_advertising_tx_packet;
mod legacy_scanning;
mod legacy_scanning_event_image;
mod peripheral_connection;
mod placement;
mod rx_memory_list;
mod rx_packet_bytes;
pub mod scheduler;
mod scheduler_context;
mod scheduler_item;
mod scheduler_pool;
mod shared_layout;
mod sram_link;
#[cfg(target_arch = "riscv32")]
pub use placement::ChipDma;
#[cfg(not(target_arch = "riscv32"))]
pub use placement::HostDma;
pub use placement::{ControllerDmaSpace, Placement};

pub use coexistence::{
    AdvertisingCoexistencePriorities, ConnectionCoexistencePriorities,
    LegacyScanCoexistencePriorities, PeripheralConnectionCoexistenceProtection,
    SchedulerItemCoexistencePriority,
};
pub use scheduler_item::{SCHEDULER_ITEM_UNEXECUTED, SchedulerItemCompletionStatus};

pub use scheduler_pool::{
    SchedulerAllocationConfig, SchedulerAllocationNumbers, SchedulerItemId, SchedulerItemSpace,
    SchedulerPoolError, SchedulerPoolPlaceError, SchedulerRoleInstance, SchedulerRoleKind,
    SchedulerRolePool, SchedulerRolePoolStorage, SchedulerRoleReleaseFailure,
};

pub use ble_phy_engine::{
    BLUETOOTH_BLE_PHY_ENVIRONMENT_BYTES, BLUETOOTH_BLE_PHY_RESOLVING_LIST_BYTES,
    BlePhyEngineCpuOwned, BlePhyEngineStorage, BlePhyLe1MPacketStartCalibration,
};

pub use connectable_advertising::{
    LegacyConnectableAdvIndPacketInput, LegacyConnectableAdvertisingError,
    LegacyConnectableAdvertisingMemoryInput, LegacyConnectableAdvertisingOwnAddress,
    LegacyConnectableAdvertisingPduFitError, LegacyConnectableAdvertisingPool,
    LegacyConnectableAdvertisingStorage, LegacyConnectableScanResponsePacketInput,
};

pub use direction_finding_workspace::{
    BLUETOOTH_DIRECTION_FINDING_WORKSPACE_BYTES, DirectionFindingWorkspaceCpuOwned,
    DirectionFindingWorkspaceLink, DirectionFindingWorkspaceStorage,
};

pub use dtm_event_image::{
    DtmLinkStateReviewedWords, DtmPositionalEventWords, DtmReceiverEventPhase, DtmRole,
    DtmRxHeaderTailProjection, DtmSchedulerFrequency, DtmSchedulerItemEventType,
    DtmSchedulerItemReviewedWords, DtmSchedulerReceiverPhy, DtmSchedulerTransmitterPhy,
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

pub use legacy_advertising_event_image::{
    LegacyAdvertisingPduError, LegacyAdvertisingPrimaryChannel, LegacyAdvertisingPrimaryChannelPlan,
};

pub use legacy_advertising::{
    BLUETOOTH_LEGACY_ADVERTISING_LINK_STATE_BYTES, BLUETOOTH_LEGACY_ADVERTISING_MAX_PAYLOAD_BYTES,
    BLUETOOTH_LEGACY_ADVERTISING_SCHEDULER_ITEM_BYTES,
    BLUETOOTH_LEGACY_ADVERTISING_SCHEDULER_ITEM_CAPACITY,
    BLUETOOTH_LEGACY_ADVERTISING_TX_PACKET_BYTES, LegacyAdvertisingError, LegacyAdvertisingEvent,
    LegacyAdvertisingPool, LegacyAdvertisingStorage,
};

pub use legacy_scanning_event_image::{
    LegacyScanFilterPolicy, LegacyScanPrimaryChannel, LegacyScanResetConfig,
    LegacyScanSchedulerWindow, LegacyScanStartSelection, LegacyScanType, LegacyScanWindowTicks,
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
pub(crate) use scheduler_pool::SchedulerRoleStorage;
