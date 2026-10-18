//! Application-facing ESP32-S31 station lifecycle facade.
//!
//! The public facade preserves stable application paths while the implementation
//! separates command publication, one connected epoch and the outer reconnect
//! lifecycle.

#[cfg(target_arch = "riscv32")]
pub mod attempt;
mod backend;
mod command;
mod composer;
pub mod connected;
pub mod control;
pub mod control_mailbox;
pub mod control_slot;
pub mod epoch;
pub mod esp_now_mailbox;
pub mod esp_now_tx;
#[cfg(target_arch = "riscv32")]
mod join;
#[cfg(any(target_arch = "riscv32", test))]
mod join_port;
mod lifecycle;
pub mod network;
pub mod power;
#[cfg(target_arch = "riscv32")]
mod reclaim;
mod resources;
pub mod runtime;
pub mod rx_protocol;
#[cfg(target_arch = "riscv32")]
mod scan;
pub mod teardown;
pub mod tx;
pub mod tx_epoch;
#[cfg(target_arch = "riscv32")]
mod wpa2_port;

#[cfg(target_arch = "riscv32")]
pub use join::{
    StationJoinError, StationJoinOutcome, StationJoinResources, StationJoinReturned,
    run_esp32s31_station_join,
};

pub use backend::StationAttemptRunner;

pub use command::{
    StationCommand, StationCommandReceiver, StationCompletion, StationControlError,
    StationControlResources, StationController,
};

pub use composer::{
    NoopStationEngineObserver, StationConnectedPhase, StationEngine, StationEngineObserver,
    StationEngineOwner, StationEnginePort, StationInitialJoinPhase, StationInitialScanExit,
    StationInitialScanPhase, StationJoinExit, StationReconnectedPhase,
    StationRunningScanCompletion, StationRunningScanExit, StationRunningScanPhase,
    StationServiceOwner, StationServicePhase, StationServicePhaseKind,
    complete_esp32s31_station_running_scan,
};

pub use lifecycle::{
    StationConfiguration, StationExit, StationPrepareFailure, StationReturnedResources,
    StationStartResources, StationStopReason, StationTask, prepare_esp32s31_station_task,
};

pub use self::control::{
    ConnectedControlShutdown, ConnectedWpa2Security, ConnectedWpa2SecurityEvidence,
    ConnectedWpa2SecurityFailure,
};
#[cfg(target_arch = "riscv32")]
pub use oer_esp32s31_hal::ieee80211::arena::RadioOwnerRepublish;
#[cfg(target_arch = "riscv32")]
use oer_esp32s31_ieee80211::runtime::WifiRoleOwner;
#[cfg(target_arch = "riscv32")]
pub use reclaim::{
    StationInterruptEpochState, StationPhaseRebindFailure, StationPhaseReclaimError,
    StationPhaseReclaimFailure, StationPhaseReclaimed, StationPhaseRestoreFailure,
    StationRuntimeReclaimFailure, StationRuntimeReclaimed, StationStoppedPhaseResources,
    try_rebind_esp32s31_station_phase, try_reclaim_esp32s31_station_phase,
    try_reclaim_esp32s31_station_runtime, try_restore_esp32s31_station_phase,
};

pub use resources::{
    StationDmaResources, StationRadioResources, StationRuntimeParts, StationRuntimeResources,
    StationStorageResources,
};

pub use runtime::{
    StaApStationControlError, StaApStationFinishFailure, StaApStationFinishReason,
    StaApStationPrepareFailure, StaApStationPrepared, StaApStationTxError,
    StaApStationTxOwnershipError, StationRoleRuntime, finish_sta_ap_station,
    park_sta_ap_station_role, prepare_sta_ap_station,
};

#[cfg(target_arch = "riscv32")]
pub use scan::{
    ESP32S31_STATION_PROBE_DESCRIPTOR_CAPACITY, ESP32S31_STATION_PROBE_RATES,
    StationInitialScanFailures, StationInitialScanReturned, StationScanDecision,
    StationScanOutcome, StationScanPlan, StationScanRequest, StationScanResources,
    StationScanReturned, complete_esp32s31_station_initial_scan,
    esp32s31_station_scan_failure_disposition, run_esp32s31_station_scan,
};

#[cfg(test)]
mod tests;
