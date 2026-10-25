//! Value-only hardware contracts exposed above the PAC boundary.
//!
//! The defining register domains remain in the restricted PAC, while this
//! module is the only public path used by HAL consumers.  No peripheral owner,
//! register block, raw accessor, or generic MMIO capability is re-exported.

pub use crate::bluetooth::BluetoothMemoryListSelector;
/// The Wi-Fi MAC value types of the HAL's register operations.
pub use crate::ieee80211::registers::{
    ExtraSoftApRxBlockAckEntrySnapshot, MAC_TX_POWER_RATE_COUNT, MacApReceivePolicySnapshot,
    MacAssociationId, MacCcmpKeyIdentity, MacCoexPrioritySnapshot, MacDelaySlot,
    MacExtraSoftApRxBlockAckEntryIndex, MacHe20PeerConfig, MacHe20PeerError,
    MacHeBeamformingReportProfile, MacHeBeamformingReportProfileError, MacHeBssColor,
    MacHeDefaultPacketExtensionDuration, MacHeErSuAckRateProfile, MacHeFecCoding,
    MacHeGuardIntervalAndLtf, MacHeMcs, MacHePacketPaddingDuration, MacHeRate,
    MacHeTbLinkReservation, MacHeTbProgramError, MacHeTbTidLimit, MacHeTid,
    MacHeTriggerRxDiagnostics, MacHeTriggerTxQueueSnapshot, MacHeTxFormat, MacHeTxParameters,
    MacHeTxProgram, MacHtAmpduCompletionObservation, MacHtChannelWidth, MacHtGuardInterval,
    MacHtMcs, MacHtProtectionSpacing, MacHtRate, MacHtTxFormat, MacHtTxParameters, MacHtTxProgram,
    MacInterface, MacInterruptEnableState, MacInterruptEvents, MacInterruptMask,
    MacInterruptObservation, MacInterruptSnapshot, MacItwtClearIndex, MacKeyEntryIndex,
    MacKeyInstallOutcome, MacLegacyRate, MacLegacyTxParameters, MacLegacyTxProgram,
    MacLegacyTxResponse, MacMinimumMpduStartSpacing, MacOrdinaryTxQueueSnapshot,
    MacPartialRuPowerSelector, MacPowerInterruptObservation, MacPowerInterruptSnapshot, MacPti,
    MacRoleReceivePolicy, MacRtcCalibration, MacRxBlockAckEntryIndex,
    MacRxBlockAckStartingSequence, MacRxBlockAckTid, MacRxBlockAckWindow,
    MacRxDecodeErrorStatistics, MacRxDecodeErrorStatisticsDelta, MacRxDmaSnapshot,
    MacRxHangStatistics, MacRxHangStatisticsDelta, MacRxPrimaryStatistics,
    MacRxPrimaryStatisticsDelta, MacRxStatisticsSnapshot, MacSppMode, MacStaApReceivePlan,
    MacStaPolicyMode, MacStaReceivePolicySnapshot, MacTxCompletionObservation, MacTxControlFrame,
    MacTxDetachOutcome, MacTxDetachReason, MacTxPowerPair, MacTxPowerTable, MacTxProtection,
    MacTxPtiCount, MacTxPtiProgram, MacTxQueueDetached, MacTxQueueIndex, MacTxStatisticsSnapshot,
    RxBlockAckEntrySnapshot, StaTbttSchedule, TxBlockAckPayload,
};

pub use crate::phy::restore::{
    BluetoothTxPowerControlPrepareError, BluetoothTxPowerControlRestoreError,
    RxDcoControlPrepareError, RxDcoControlRestoreError, TxDcPwdetLifecycleError,
    TxDcPwdetPrepareError, TxDcPwdetRestoreError, TxIqToneControlPrepareError,
    TxIqToneControlRestoreError,
};

pub use crate::ieee80211::registers::WifiRxCsiControlMask;
pub use crate::ieee80211::station_wake::StaTbttScheduleError;

pub use crate::modem::values::{
    CfrValue, CoexTimerClientValue, CoexTimerClockDividerMinusOne, CoexTimerClockSource,
    CoexTimerPtiValue, ForcedRxGain, PhyForcedPowerIndex,
};
pub use crate::modem::{
    CoexTimerRegister, CoexistenceLowPowerClockObservation, CoexistenceLowPowerClockSource,
};
pub use crate::phy::{
    i2c::PhyAdcRate,
    memory::{PbusMemoryGroupBoundary, PhyGainMemoryEntry, PhyMemoryError},
};
