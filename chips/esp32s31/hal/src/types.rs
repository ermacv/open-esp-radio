//! Value-only hardware contracts exposed above the PAC boundary.
//!
//! The defining register domains remain in the restricted PAC, while this
//! module is the only public path used by HAL consumers.  No peripheral owner,
//! register block, raw accessor, or generic MMIO capability is re-exported.

/// The Wi-Fi MAC value types of the HAL's register operations.
pub use crate::radio::ieee80211::registers::{
    MAC_MINIMUM_TX_POWER_DBM, MAC_TX_ABORT_SETTLE, MAC_TX_POWER_RATE_COUNT, MAC_TX_TIMEOUT_MAX,
    MacAssociationId, MacCcmpKeyIdentity, MacCoexPrioritySnapshot, MacCryptoInterface,
    MacDelaySlot, MacDsssPreamble, MacExtraSoftApRxBlockAckEntryIndex, MacHeAssociation,
    MacHeBeamformingReportProfile, MacHeBeamformingReportProfileError, MacHeBssColor,
    MacHeDefaultPacketExtensionDuration, MacHeFecCoding, MacHeMcs, MacHeMpduLength,
    MacHePacketPaddingDuration, MacHePeerConfig, MacHeRate, MacHeTbLinkReservation,
    MacHeTbProgramError, MacHeTbStatistics, MacHeTbTidLimit, MacHeTid, MacHeTriggerTxQueueSnapshot,
    MacHeTxFormat, MacHeTxParameters, MacHeTxProgram, MacHtAmpduCompletionObservation,
    MacHtChannelWidth, MacHtGuardInterval, MacHtMcs, MacHtProtectionSpacing, MacHtRate,
    MacHtTxFormat, MacHtTxParameters, MacHtTxProgram, MacInterface, MacInterruptCause,
    MacInterruptEvents, MacInterruptMask, MacInterruptObservation, MacInterruptRawSnapshot,
    MacInterruptSideEvent, MacInterruptSnapshot, MacItwtClearIndex, MacKeyEntryIndex,
    MacKeyInstallOutcome, MacLegacyRate, MacLegacyTxParameters, MacLegacyTxProgram,
    MacLegacyTxResponse, MacMinimumMpduStartSpacing, MacOrdinaryTxQueueSnapshot,
    MacPartialRuPowerSelector, MacPowerInterruptCause, MacPowerInterruptObservation,
    MacPowerInterruptSnapshot, MacPti, MacRoleReceivePolicy, MacRtcCalibration,
    MacRxBlockAckEntryIndex, MacRxBlockAckStartingSequence, MacRxBlockAckTid, MacRxBlockAckWindow,
    MacRxDecodeErrorStatistics, MacRxDecodeErrorStatisticsDelta, MacRxHangStatistics,
    MacRxHangStatisticsDelta, MacRxLane, MacRxPrimaryStatistics, MacRxPrimaryStatisticsDelta,
    MacRxStatisticsSnapshot, MacSppMode, MacStaApReceivePlan, MacStaPolicyMode, MacTxAbortCause,
    MacTxAbortOutcome, MacTxCompletionObservation, MacTxControlFrame, MacTxDetachOutcome,
    MacTxDetachReason, MacTxFlow, MacTxPowerPair, MacTxPowerTable, MacTxPrepareFault,
    MacTxPrepareRefused, MacTxProtection, MacTxPtiCount, MacTxPtiProgram, MacTxQueueAborted,
    MacTxQueueDetached, MacTxQueueIndex, MacTxQueueOwnership, MacTxQueuePending,
    MacTxQueueReleased, MacTxStatisticsSnapshot, RxBlockAckEntrySnapshot, StaTbttSchedule,
    TxBlockAckPayload,
};

/// The generated names the HAL's public functions and fields take, under
/// the HAL's own names where the PAC's are generic.
pub use oer_chips_esp32s31_pac::{
    fe::estimator_control::{
        MeasurementEnable as PhyEstimatorMeasurement, StartEnable as PhyEstimatorStart,
    },
    fe::frequency_control::HardwareFrequencyDisable as PhyFrequencyControl,
    fe::rx_gain_dc_control::Bits5_6Opaque as PhyRxGainDcCalibration,
    fe::status_clock_force::Bits8_11Opaque as PhyForceTxRxPhase,
    fe::table_memory_index_source::ForceDigitalGainEnable as PhyForcedDigitalGain,
    fe::tone_path_0_control::ToneEnableOrArm as PhyTone,
    i2c_ana_mst::ana_conf0::BbpllCalMode as PhyBbpllCalibration,
    wifi_bb::tx_power_track_control_0::TrackEnable as PhyTxPowerTracking,
    wifi_bb_brx::dictionaries::LowRate as PhyLowRate,
    wifi_mac::control::PowerSaveTxBlock as MacPowerSaveTxBlock,
    wifi_mac::dictionaries::{
        BfReportDcm as MacHeBeamformingReportDcm, BfReportErsu as MacHeBeamformingReportErsu,
        ErsuAckRate as MacHeErsuAckRate, TriggerBasedTid as MacHeTriggerBasedTid,
    },
    wifi_mac::ersu_and_vht_control::{
        AutoAckAllowErsu as MacHeAutoAckErsu, TbCareCcaAndNav as MacHeTbCareCcaAndNav,
    },
    wifi_mac::he_default_control::{
        UlMuDataDisable as MacHeUplinkMultiUserDataDisable,
        UlMuDisable as MacHeUplinkMultiUserDisable,
    },
    wifi_mac::he_su_signal_a1::GiLtf as MacHeGiLtf,
    wifi_mac::length_control::RtsRate as MacRtsRate,
    wifi_mac::multi_bssid_control::{
        BssColorEnable as MacHeBssColorEnable, PartialBssColorEnable as MacHePartialBssColorEnable,
    },
    wifi_mac::plcp1::Rate as MacPlcpRate,
    wifi_mac::pti::Txop as MacTxop,
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
    CoexTimerClientValue, CoexTimerClockDividerMinusOne, CoexTimerClockSource, CoexTimerPtiValue,
    PhyForcedPowerIndex,
};
pub use crate::radio::modem::{
    CoexTimerRegister, CoexistenceLowPowerClockObservation, CoexistenceLowPowerClockSource,
};
pub use crate::radio::phy::{
    baseband::ops::TxIqMismatchPolarity,
    frequency::ops::FrequencyRegisterMode,
    i2c::PhyAdcRate,
    memory::{PbusMemoryGroupBoundary, PhyGainMemoryEntry, PhyMemoryError},
    pbus::PhyClockPair,
};
