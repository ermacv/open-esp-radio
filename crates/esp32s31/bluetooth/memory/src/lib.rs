//! ESP32-S31 Bluetooth controller-memory boundary.
//!
//! The restricted PAC below this crate owns positional MMIO transactions and
//! compressed-address encoding. This crate owns controller-SRAM layouts and
//! role-to-list routing that are consumed above the register boundary. It is
//! intentionally sparse: static graph location, allocation-time links, the
//! fixed DTM allocator prefix and the first empty-list item-link transform are
//! bound. A matching affine PAC head-publication token can then consume every
//! rollback image into a controller-visible graph. The exact RUN proof then
//! advances it to a running graph. An affine fenced finished-list
//! observation can then drive one volatile semantic item-status read without
//! granting CPU ownership. Matching empty-head and post-unlink removal proofs
//! authorize the reviewed cleanup. RX-success additionally validates the
//! exact two-header private chain and performs its bounded volatile swap/re-arm
//! rotation before CPU ownership returns.

#![no_std]
#![forbid(unsafe_code)]

#[cfg(test)]
extern crate std;

mod ble_phy_engine;
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
mod legacy_advertising;
mod legacy_advertising_event_image;
mod legacy_advertising_tx_packet;
mod legacy_scanning;
mod legacy_scanning_event_image;
mod link_state_event;
mod peripheral_connection;
mod rx_memory_list;
mod scheduler_context;
mod scheduler_item;
mod scheduler_pool;
mod sram_link;

pub use coexistence::{
    AdvertisingCoexistencePriorities, ConnectionCoexistencePriorities,
    LegacyScanCoexistencePriorities, PeripheralConnectionCoexistenceProtection,
    SchedulerItemCoexistencePriority,
};
pub use scheduler_item::{SCHEDULER_ITEM_UNEXECUTED, SchedulerItemCompletionStatus};

#[cfg(not(target_arch = "riscv32"))]
pub use scheduler_pool::SchedulerPoolModelAddress;
pub use scheduler_pool::{
    SchedulerAllocationConfig, SchedulerAllocationNumbers, SchedulerItemId, SchedulerItemSource,
    SchedulerItemSpace, SchedulerPoolBindError, SchedulerPoolError, SchedulerRoleInstance,
    SchedulerRoleKind, SchedulerRolePool, SchedulerRolePoolStorage, SchedulerRoleReleaseFailure,
    SchedulerRoleStorage,
};

#[cfg(not(target_arch = "riscv32"))]
pub use ble_phy_engine::BlePhyEngineModelAddress;

pub use ble_phy_engine::{
    BLUETOOTH_BLE_PHY_ENVIRONMENT_BYTES, BLUETOOTH_BLE_PHY_RESOLVING_LIST_BYTES,
    BlePhyEngineBindError, BlePhyEngineBindFailure, BlePhyEngineBinding, BlePhyEngineCpuOwned,
    BlePhyEngineStorage, BlePhyLe1MPacketStartCalibration,
};

pub use connectable_advertising::{
    LegacyConnectableAdvIndPacketInput, LegacyConnectableAdvertisingError,
    LegacyConnectableAdvertisingMemoryInput, LegacyConnectableAdvertisingOwnAddress,
    LegacyConnectableAdvertisingPduFitError, LegacyConnectableAdvertisingPool,
    LegacyConnectableAdvertisingStorage, LegacyConnectableScanResponsePacketInput,
};

#[cfg(not(target_arch = "riscv32"))]
pub use direction_finding_workspace::DirectionFindingWorkspaceModelAddress;

pub use direction_finding_workspace::{
    BLUETOOTH_DIRECTION_FINDING_WORKSPACE_BYTES, DirectionFindingWorkspaceBindError,
    DirectionFindingWorkspaceBindFailure, DirectionFindingWorkspaceBinding,
    DirectionFindingWorkspaceCpuOwned, DirectionFindingWorkspaceLink,
    DirectionFindingWorkspaceStorage,
};

pub use dtm_event_image::{
    DtmLinkStateReviewedWords, DtmPositionalEventWords, DtmReceiverEventPhase, DtmRole,
    DtmRxHeaderTailProjection, DtmSchedulerItemEventType, DtmSchedulerItemReviewedWords,
    DtmSchedulerReceiverPhy, DtmSchedulerTransmitterPhy, DtmTxHeaderHeadProjection,
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
    BLUETOOTH_FILTER_ACCEPT_LIST_CAPACITY, LeDeviceTable, LeDeviceTableBindError,
    LeDeviceTableError, LeDeviceTablePublication, LeDeviceTableStorage, LeFilterAcceptListDevice,
};
#[cfg(not(target_arch = "riscv32"))]
pub use le_rx_chain::LeRxChainModelAddress;
pub use le_rx_chain::{
    LeRxChain, LeRxChainBindError, LeRxChainError, LeRxChainStorage, LeRxSource, LeRxTag,
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

pub use sram_link::{
    BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_HIGH, BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_LOW,
    ControllerSramLinkAddress, ControllerSramLinkAddressError,
};
