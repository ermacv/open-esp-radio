//! Value-only hardware contracts exposed above the PAC boundary.
//!
//! The defining register domains remain in the restricted PAC, while this
//! module is the only public path used by HAL consumers.  No peripheral owner,
//! register block, raw accessor, or generic MMIO capability is re-exported.

pub use oer_esp32s31_pac::{
    CfrValue, CoexTimerClientValue, CoexTimerClockDividerMinusOne,
    CoexTimerClockSource, CoexTimerPtiValue, CoexTimerRegister,
    CoexistenceLowPowerClockObservation, CoexistenceLowPowerClockSource,
    ExtraSoftApRxBlockAckEntrySnapshot, ForcedRxGain, MAC_TX_POWER_RATE_COUNT,
    MacApReceivePolicySnapshot, MacAssociationId, MacCcmpKeyIdentity, MacCoexPrioritySnapshot,
    MacDelaySlot, MacExtraSoftApRxBlockAckEntryIndex, MacHe20PeerConfig, MacHe20PeerError,
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
    PbusMemoryGroupBoundary, PhyAdcRate, PhyForcedPowerIndex, PhyFtmEnableVendorArgument,
    PhyGainMemoryEntry, PhyMemoryError, RxBlockAckEntrySnapshot, StaTbttSchedule,
    TxBlockAckPayload,
};

pub use crate::phy::restore::{
    BluetoothTxPowerControlPrepareError, BluetoothTxPowerControlRestoreError,
    RxDcoControlPrepareError, RxDcoControlRestoreError, TxDcPwdetLifecycleError,
    TxDcPwdetPrepareError, TxDcPwdetRestoreError, TxIqToneControlPrepareError,
    TxIqToneControlRestoreError,
};

pub use crate::ieee80211::station_wake::StaTbttScheduleError;
pub use oer_esp32s31_pac::WifiRxCsiControlMask;
