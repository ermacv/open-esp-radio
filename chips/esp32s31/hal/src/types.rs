//! Value-only hardware contracts exposed above the PAC boundary.
//!
//! The defining register domains remain in the restricted PAC, while this
//! module is the only public path used by HAL consumers.  No peripheral owner,
//! register block, raw accessor, or generic MMIO capability is re-exported.

/// The Wi-Fi MAC value types of the HAL's register operations.
pub use crate::radio::ieee80211::registers::{
    MAC_MINIMUM_TX_POWER_DBM, MAC_TX_ABORT_SETTLE, MAC_TX_POWER_RATE_COUNT, MAC_TX_TIMEOUT_MAX,
    MacAssociationId, MacCcmpKeyIdentity, MacCoexPrioritySnapshot, MacDelaySlot, MacDsssPreamble,
    MacExtraSoftApRxBlockAckEntryIndex, MacHeAssociation, MacHeBeamformingReportProfile,
    MacHeBeamformingReportProfileError, MacHeBssColor, MacHeDefaultPacketExtensionDuration,
    MacHeErSuAckRateProfile, MacHeFecCoding, MacHeMcs, MacHePacketPaddingDuration, MacHePeerConfig,
    MacHeRate, MacHeTbLinkReservation, MacHeTbProgramError, MacHeTbStatistics, MacHeTbTidLimit,
    MacHeTid, MacHeTriggerRxDiagnostics, MacHeTriggerTxQueueSnapshot, MacHeTxFormat,
    MacHeTxParameters, MacHeTxProgram, MacHtAmpduCompletionObservation, MacHtChannelWidth,
    MacHtGuardInterval, MacHtMcs, MacHtProtectionSpacing, MacHtRate, MacHtTxFormat,
    MacHtTxParameters, MacHtTxProgram, MacInterface, MacInterruptCause, MacInterruptEvents,
    MacInterruptMask, MacInterruptObservation, MacInterruptRawSnapshot, MacInterruptSideEvent,
    MacInterruptSnapshot, MacItwtClearIndex, MacKeyEntryIndex, MacKeyInstallOutcome, MacLegacyRate,
    MacLegacyTxParameters, MacLegacyTxProgram, MacLegacyTxResponse, MacMinimumMpduStartSpacing,
    MacOrdinaryTxQueueSnapshot, MacPartialRuPowerSelector, MacPowerInterruptCause,
    MacPowerInterruptObservation, MacPowerInterruptSnapshot, MacPti, MacRoleReceivePolicy,
    MacRtcCalibration, MacRxBlockAckEntryIndex, MacRxBlockAckStartingSequence, MacRxBlockAckTid,
    MacRxBlockAckWindow, MacRxDecodeErrorStatistics, MacRxDecodeErrorStatisticsDelta,
    MacRxHangStatistics, MacRxHangStatisticsDelta, MacRxLane, MacRxPrimaryStatistics,
    MacRxPrimaryStatisticsDelta, MacRxStatisticsSnapshot, MacSppMode, MacStaApReceivePlan,
    MacStaPolicyMode, MacTxAbortCause, MacTxAbortOutcome, MacTxCompletionObservation,
    MacTxControlFrame, MacTxDetachOutcome, MacTxDetachReason, MacTxFlow, MacTxPowerPair,
    MacTxPowerTable, MacTxPrepareFault, MacTxPrepareRefused, MacTxProtection, MacTxPtiCount,
    MacTxPtiProgram, MacTxQueueAborted, MacTxQueueDetached, MacTxQueueIndex, MacTxQueueOwnership,
    MacTxQueuePending, MacTxQueueReleased, MacTxStatisticsSnapshot, RxBlockAckEntrySnapshot,
    StaTbttSchedule, TxBlockAckPayload,
};

pub use crate::radio::phy::restore::{
    BluetoothTxPowerControlPrepareError, BluetoothTxPowerControlRestoreError,
    RxDcoControlPrepareError, RxDcoControlRestoreError, TxDcPwdetLifecycleError,
    TxDcPwdetPrepareError, TxDcPwdetRestoreError, TxIqToneControlPrepareError,
    TxIqToneControlRestoreError,
};

pub use crate::radio::ieee80211::registers::WifiRxCsiControlMask;
pub use crate::radio::ieee80211::station_wake::StaTbttScheduleError;

pub use crate::radio::modem::values::{
    CfrValue, CoexTimerClientValue, CoexTimerClockDividerMinusOne, CoexTimerClockSource,
    CoexTimerPtiValue, ForcedRxGain, PhyForcedPowerIndex,
};
pub use crate::radio::modem::{
    CoexTimerRegister, CoexistenceLowPowerClockObservation, CoexistenceLowPowerClockSource,
};
pub use crate::radio::phy::{
    i2c::PhyAdcRate,
    memory::{PbusMemoryGroupBoundary, PhyGainMemoryEntry, PhyMemoryError},
};
