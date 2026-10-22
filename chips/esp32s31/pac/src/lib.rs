#![no_std]
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

#[cfg(test)]
extern crate std;

#[cfg(test)]
mod test_memory;

// Outside the production PHY module, which forbids all unsafe code.
#[cfg(all(test, target_os = "linux"))]
#[path = "phy/baseband/txrx_reset_tests.rs"]
mod frontend_reset_tests;

pub(crate) use modem::coex;

pub(crate) use phy::{baseband, cfr};

pub use phy::clock;

pub(crate) use phy::frequency;
// The generated capability catalog is intentionally broader than this crate's
// restricted ownership facade. Some reviewed leaves stay unreachable until an
// owner transition exposes them; do not reopen full-block access to make them
// appear used.
#[allow(
    dead_code,
    reason = "generated capability catalog is wider than the restricted ownership facade"
)]
mod generated;

pub use phy::pbus;
pub mod phy;

pub(crate) use phy::table_memory;

/// Reviewed writable MAC interrupt mask.
///
/// The generated domain deliberately has no public integer constructor:
///
/// ```compile_fail
/// use oer_esp32s31_pac::MacInterruptMask;
///
/// let invented = MacInterruptMask(0xdead_beef);
/// ```
pub use generated::MacInterruptMask;

pub use generated::WifiRxCsiControlMask;
/// Memory layouts the radio hardware reads and writes in RAM: the Wi-Fi DMA
/// descriptors, RX-control metadata and BLE controller structures, generated from the register
/// model's `layouts/` (`docs/hardware-memory.md`).
pub use generated::layouts;

pub use phy::frequency::frequency_cap_correction_word;
pub use phy::table_memory::{pbus_memory_parameter_002_data, pbus_memory_parameter_014_data};

pub use baseband::{RxDcoControlField, TxDcPwdetFields, TxIqToneControlFields};

pub use bluetooth::{
    controller::{
        init::{
            BluetoothControllerHalInitConfig, BluetoothControllerTimeScale, BluetoothHalInitPeriod,
            BluetoothHalInitScale, BluetoothMicrosecondDeltaProjection,
            BluetoothRawTickDeltaProjection,
        },
        time::BluetoothControllerLatchedTime,
    },
    device_table::BluetoothDeviceTablePublished,
    direction_finding::BluetoothDirectionFindingDisabledBaselinePrepared,
    interrupt::{
        BluetoothInterruptOutputPrepared, BluetoothNrtInterruptAcknowledged,
        BluetoothPrimaryFaultSources, BluetoothPrimaryInterruptEpoch,
        BluetoothSchedulerRunInterruptsPrepared,
        retirement::{BluetoothControllerOutputQuiesced, BluetoothControllerOutputReleaseError},
    },
    memory_lists::{
        BluetoothControllerSramAddress, BluetoothControllerSramAddressError,
        BluetoothMemoryListPointerImage, BluetoothMemoryListSelector, BluetoothMemoryListSlot,
    },
    modem_timer::{
        BluetoothLowPowerRuntimeControlObservation, BluetoothModemLpTimerCompareDisposition,
        BluetoothModemLpTimerCounterObservation, BluetoothModemLpTimerEpoch,
        BluetoothModemLpTimerHandlerRegisterObservation, BluetoothModemLpTimerInstant,
        BluetoothModemLpTimerInterruptObservation, BluetoothModemLpTimerRegisters,
    },
    phy::{
        BlePhyEtmRouteDisabled, BluetoothPhyEnvironmentAddress,
        BluetoothPhyEnvironmentAddressError, BluetoothPhyRegisterInitInputs,
    },
    scan::BluetoothScanStartPublished,
    scheduler::{
        BluetoothSchedulerHardwareListHead, BluetoothSchedulerHardwareListHeadEmptyObserved,
        BluetoothSchedulerHardwareListHeadError, BluetoothSchedulerHardwareListHeadPublished,
        BluetoothSchedulerHardwareListHeadRetirementObservation,
        BluetoothSchedulerHardwareListsCleared, BluetoothSchedulerHardwareRunCommandPublished,
        BluetoothSchedulerInsertionCommand, BluetoothSchedulerInsertionCommandStartCleared,
        BluetoothSchedulerRunEventPublished,
        cancellation::{
            BluetoothSchedulerCancellationDisposition, BluetoothSchedulerCancellationIndexed,
            BluetoothSchedulerCancellationReleased, BluetoothSchedulerCancellationRequested,
            BluetoothSchedulerCancellationSourceAcknowledged, BluetoothSchedulerSkipCleared,
            BluetoothSchedulerSkipDisposition, BluetoothSchedulerSkipPublished,
            BluetoothSchedulerSkipRequest, BluetoothSchedulerSkipResult,
        },
        insertion::{
            BluetoothSchedulerExecutionLockDisposition, BluetoothSchedulerExecutionLockPublished,
            BluetoothSchedulerExecutionLockRequest, BluetoothSchedulerExecutionModifyDisposition,
            BluetoothSchedulerExecutionModifyPublished,
        },
        lock_modify::{
            BluetoothSchedulerLockModifyInterruptObservation,
            BluetoothSchedulerLockModifyObservation, BluetoothSchedulerLockModifyPublished,
            BluetoothSchedulerLockModifyRequest, BluetoothSchedulerLockModifyTaskObservation,
        },
        runtime::{
            BluetoothSchedulerBusyObservation, BluetoothSchedulerFinishedHardwareListObserved,
            BluetoothSchedulerFinishedListObservation, BluetoothSchedulerFinishedListPop,
            BluetoothSchedulerHardwareListIndex, BluetoothSchedulerReferenceCleared,
            BluetoothSchedulerReferenceGateObservation, BluetoothSchedulerSoftwareListRemovalIdle,
            BluetoothSchedulerSoftwareListRemovalInterruptStep,
            BluetoothSchedulerSoftwareListRemovalJoin, BluetoothSchedulerSoftwareListRemovalReady,
            BluetoothSchedulerWorkObservation,
        },
        stop::{
            BluetoothSchedulerStopped, BluetoothSchedulerStoppedHeadRetirement,
            BluetoothSchedulerStoppedItem,
        },
    },
};

pub use cfr::CfrValue;

pub use coex::{COEX_TIMER_COUNT, CoexTimerBankRegisters, CoexTimerRegister};
pub use generated::{ExternalCoexGrantDelay, WifiDescriptorAddressLow};
pub use modem::coex_external::{ExternalCoexPriority, ExternalCoexRole, ExternalCoexWires};

pub use frequency::PhyFrequencyI2cNumberAddresses;

#[doc(hidden)]
pub use ieee802154::mac::{Ieee802154PolledRegisterLease, Ieee802154RegisterLease};

pub use oer_espressif_ieee80211_pac::MacInterface;

pub use generated::{
    BluetoothDeviceTableEntryCount, CoexTimerClientValue, CoexTimerClockDividerMinusOne,
    CoexTimerClockSource, CoexTimerPtiValue, CoexTimerTickInput, MacAssociationId,
    MacExtraSoftApRxBlockAckEntryIndex, MacHeBssColor, MacHeDefaultPacketExtensionDuration,
    MacHePacketPaddingDuration, MacItwtClearIndex, MacKeyEntryIndex, MacMinimumMpduStartSpacing,
    MacPti, MacRxBlockAckEntryIndex, MacRxBlockAckStartingSequence, MacRxBlockAckTid,
    MacRxBlockAckWindow, MacTxPtiCount, MacTxQueueIndex, ModemLowPowerClockDivider,
    PhyForcedPowerIndex, PhyFtmEnableVendorArgument,
};

pub use ieee802154::mac::{
    Ieee802154AckTimeoutUnits, Ieee802154CcaMode, Ieee802154DebugCounter, Ieee802154DmaErrorCodes,
    Ieee802154EdCcaSnapshot, Ieee802154EdCommand, Ieee802154EdDurationUnits,
    Ieee802154EdSampleMode, Ieee802154EdSampleRate, Ieee802154EtmChannel, Ieee802154EtmRoute,
    Ieee802154Event, Ieee802154EventEnableState, Ieee802154EventMask, Ieee802154EventObservation,
    Ieee802154EventObservationError, Ieee802154FoundationSnapshot, Ieee802154FrequencyCode,
    Ieee802154MacCommand, Ieee802154MacConfigurationReadback, Ieee802154MacControl,
    Ieee802154MacPolicySnapshot, Ieee802154MultipanEnableState, Ieee802154MultipanIndex,
    Ieee802154ObservedEventState, Ieee802154OperationEventEnableObservation,
    Ieee802154OperationRxAbortEnableObservation, Ieee802154PanIdentity, Ieee802154PowerSequence,
    Ieee802154Pti, Ieee802154RouteState, Ieee802154RxAbortEnableSet, Ieee802154RxAbortEnableState,
    Ieee802154RxAbortReason, Ieee802154RxAbortReasonObservation, Ieee802154RxStateCode,
    Ieee802154RxStatus, Ieee802154SecurityPayloadOffset, Ieee802154StateSnapshot,
    Ieee802154Timer0ThresholdWord, Ieee802154Timer0ValueWord, Ieee802154Timer1ThresholdWord,
    Ieee802154Timer1ValueWord, Ieee802154TimerLease, Ieee802154TransmitSecurityControl,
    Ieee802154TxAbortEnableSet, Ieee802154TxAbortReason, Ieee802154TxAbortReasonObservation,
    Ieee802154TxPowerCode, Ieee802154TxSecurityError, Ieee802154TxSecurityErrorObservation,
    Ieee802154TxStateCode, Ieee802154TxStatus, Ieee802154ValidationEdDurationState,
    Ieee802154ValidationEventEnableState,
};

pub use modem::{
    platform::{
        PlatformClockPowerObservation, PlatformPllSourceBaseline, WifiPowerBaseline,
        WifiPowerRestoreReadback,
    },
    shared_clock::{
        BluetoothLowPowerClockObservation, CoexistenceLowPowerClockObservation,
        CoexistenceLowPowerClockSource, RtcSlowClockSource, SharedModemClockObservation,
        WifiLowPowerClockObservation, WifiLowPowerClockSource,
    },
};

pub use modem::syscon::{
    BLUETOOTH_APB_CLOCKS, BLUETOOTH_CLOCK_COUNT, BLUETOOTH_CONTROLLER_CLOCKS,
    BluetoothClockBaseline, ModemSysconBluetoothClock, ModemSysconBluetoothObservation,
    ModemSysconIeee802154ResetObservation, ModemSysconPowerObservation, WifiBasebandAgcUpdate,
};

use oer_esp32s31_pac_raw as svd;

pub use phy::{
    agc::runtime::ForcedRxGain,
    i2c::{
        BluetoothTxPowerControlRegister, PhyAdcRate, PhyFilterDcapInputs, PhyI2cAccessError,
        PhyI2cCommandMemoryInputs, PhyI2cConfigurationCommand, PhyI2cConfigurationOperation,
        PhyI2cInitializationStageOneInputs, PhyI2cParallelWrite,
    },
};

pub use table_memory::{PbusMemoryGroupBoundary, PhyGainMemoryEntry, PhyMemoryError};

pub use wifi::mac::{
    block_ack::{
        ExtraSoftApRxBlockAckEntrySnapshot, InternalTxBlockAckSnapshot, RxBlockAckEntrySnapshot,
        TxBlockAckDiagnosticSnapshot, TxBlockAckPayload,
    },
    coex::init::MacCoexPrioritySnapshot,
    crypto::{MacCcmpKeyIdentity, MacKeyInstallOutcome, MacSppMode},
    hal_init_tail::MacRtcCalibration,
    he::{
        beamforming::{
            MacHeBeamformingReportProfile, MacHeBeamformingReportProfileError,
            MacHeErSuAckRateProfile,
        },
        init_suffix::MacHeTxMpduLengthLink,
        ofdma::{
            MacBeamformingAverageSnr, MacHeBeamformingConfigurationSnapshot,
            MacHeBeamformingDiagnostics, MacHeBufferStatusSnapshot, MacHeCustomReceiveType,
            MacHeEdcaQueueConfiguration, MacHeMuEdcaTimerSnapshot, MacHeQueueSchedulingSnapshot,
            MacHeReceiveConfigurationSnapshot, MacHeRxPowerSaveSnapshot, MacHeTbLinkReservation,
            MacHeTbProgramError, MacHeTbTidLimit, MacHeTid, MacHeTriggerQueueConfiguration,
            MacHeTriggerRxDiagnostics, MacHeTriggerTxQueueSnapshot,
        },
        peer::{MacHe20PeerConfig, MacHe20PeerError},
        tb::{MacHeTbStatistics, MacHeTbTxDiagnostics},
    },
    interrupt::{
        ConnectedStaWithoutPowerSavePrepared, MacInterruptEnableState, MacInterruptRegisters,
        MacInterruptSetup, MacPowerInterruptRegisters,
    },
    rx::{
        dma::{MacRxDmaSnapshot, MacRxNextDescriptorObservation},
        policy::{
            MacApReceivePolicySnapshot, MacRoleReceivePolicy, MacStaApReceivePlan,
            MacStaPolicyMode, MacStaReceivePolicySnapshot,
        },
        statistics::{
            MacHeColorCollisionSnapshot, MacRxDecodeErrorStatistics,
            MacRxDecodeErrorStatisticsDelta, MacRxHangStatistics, MacRxHangStatisticsDelta,
            MacRxPrimaryStatistics, MacRxPrimaryStatisticsDelta, MacRxStatisticsSnapshot,
        },
    },
    tsf::StaTbttSchedule,
    tx::{
        MacHeFecCoding, MacHeGuardIntervalAndLtf, MacHeMcs, MacHeRate, MacHeTxFormat,
        MacHeTxParameters, MacHeTxProgram, MacHtAmpduCompletionObservation, MacHtChannelWidth,
        MacHtGuardInterval, MacHtMcs, MacHtProtectionSpacing, MacHtRate, MacHtTxFormat,
        MacHtTxParameters, MacHtTxProgram, MacLegacyRate, MacLegacyTxParameters,
        MacLegacyTxProgram, MacLegacyTxResponse, MacOrdinaryTxQueueSnapshot,
        MacTxCompletionObservation, MacTxControlFrame, MacTxDetachOutcome, MacTxDetachReason,
        MacTxProtection, MacTxPtiProgram, MacTxQueueDetached,
        power_init::{
            MAC_TX_POWER_RATE_COUNT, MacPartialRuPowerSelector, MacTxPowerIndex, MacTxPowerPair,
            MacTxPowerTable,
        },
        statistics::MacTxStatisticsSnapshot,
    },
    txrx_init::MacDelaySlot,
};
pub mod ownership;

pub use modem::clock_device::ModemClockDevice;
pub use ownership::BLUETOOTH_MAIN_XTAL_LOW_POWER_DIVIDER;

pub use ownership::{
    BluetoothControllerPartition, BluetoothInterruptRegisters, BluetoothInterruptSetup,
    BluetoothTaskRegisters, CoexistencePartition, Ieee802154InterruptRegisters,
    Ieee802154InterruptSetup, Ieee802154Partition, Ieee802154TaskRegisters, MacInterruptEvents,
    MacInterruptObservation, MacInterruptSnapshot, MacLocalTime, MacPowerInterruptObservation,
    MacPowerInterruptSnapshot, RadioPartitions, RadioPhyRegisters, SharedRadioPartition,
    SharedRadioParts, SharedRadioRegisters, WifiMacPartition, WifiRadioRegisters,
    WifiRxDmaRegisters,
};

pub(crate) use ownership::device_fence;
pub(crate) mod bluetooth;
pub(crate) mod ieee802154;
pub(crate) mod modem;
/// Owner acquisition and register transactions of isolated probe images.
#[cfg(oer_probe)]
#[doc(hidden)]
pub mod validation;
pub(crate) mod wifi;

// Generated analog-I2C identities, fields and transport routes.
include!("analog_generated.rs");
