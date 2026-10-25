#![no_std]
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

#[cfg(test)]
extern crate std;

#[cfg(test)]
mod soc_tests;

#[cfg(test)]
mod test_memory;

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

/// Every peripheral of the vendor SVD the curated model does not describe,
/// as typed register blocks with the chip's interrupt sources
/// (`soc::Interrupt`); the PAC root claims them at once.
pub use generated::soc;

/// What the pinned ESP-IDF headers take away from the vendor SVD's API.
///
/// SOC_ETM's event status is write-one-to-clear (`soc_etm_reg.h:2597`): it is
/// sampled and acknowledged,
///
/// ```
/// use oer_esp32s31_pac::soc::soc_etm::SocEtm;
/// fn acknowledge(etm: &mut SocEtm<oer_mmio::Host<'_>>) {
///     let sample = etm.sample_evt_st0();
///     etm.acknowledge_evt_st0(sample);
/// }
/// ```
///
/// never modified:
///
/// ```compile_fail
/// use oer_esp32s31_pac::soc::soc_etm::SocEtm;
/// fn modify(etm: &mut SocEtm<oer_mmio::Host<'_>>) {
///     etm.modify_evt_st0(|_| {});
/// }
/// ```
#[cfg(doctest)]
pub struct SocHeaderSemantics;

pub use phy::pbus;
pub mod phy;

/// A field of at most eight bits read back, as the byte the shared
/// IEEE 802.15.4 value types take.
pub(crate) const fn field_byte<const WIDTH: u32>(bits: oer_mmio::Bits<WIDTH>) -> u8 {
    const { assert!(WIDTH <= 8, "the field fits a byte") };
    bits.get().to_le_bytes()[0]
}

/// A field of at most sixteen bits read back, as a halfword.
pub(crate) const fn field_halfword<const WIDTH: u32>(bits: oer_mmio::Bits<WIDTH>) -> u16 {
    const { assert!(WIDTH <= 16, "the field fits a halfword") };
    let [low, high, _, _] = bits.get().to_le_bytes();
    u16::from_le_bytes([low, high])
}

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
    Ieee802154Pti, Ieee802154RxAbortEnableSet, Ieee802154RxAbortEnableState,
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

pub use phy::{
    agc::runtime::ForcedRxGain,
    i2c::{
        BluetoothTxPowerControlRegister, PhyAdcRate, PhyFilterDcapInputs, PhyI2cAccessError,
        PhyI2cCommandMemoryInputs, PhyI2cConfigurationCommand, PhyI2cConfigurationOperation,
        PhyI2cInitializationStageOneInputs, PhyI2cParallelWrite,
    },
};

pub use table_memory::{PbusMemoryGroupBoundary, PhyGainMemoryEntry, PhyMemoryError};

pub mod boot;
pub mod ownership;
pub mod platform;
pub mod pll_power;

pub use modem::clock_device::ModemClockDevice;
pub use ownership::BLUETOOTH_MAIN_XTAL_LOW_POWER_DIVIDER;

pub use ownership::{
    BluetoothControllerPartition, BluetoothInterruptRegisters, BluetoothInterruptSetup,
    BluetoothTaskRegisters, CoexistencePartition, Ieee802154InterruptRegisters,
    Ieee802154InterruptSetup, Ieee802154Partition, Ieee802154TaskRegisters, RadioHandoff,
    RadioPartitions, RadioPhyRegisters, SharedRadioPartition, SharedRadioParts,
    SharedRadioRegisters, WifiInterruptPartition, WifiMacPartition,
};

/// The Wi-Fi MAC's generated blocks: the HAL's Wi-Fi register owners hold
/// them and drive them through their field accessors.
pub use generated::peripheral_ownership::WifiMacPeripherals;
pub use generated::{
    shared_radio_init_control, wifi_mac_antenna_init, wifi_mac_aux_tsf_control,
    wifi_mac_beacon_ie_crc, wifi_mac_beamforming_feedback_test, wifi_mac_beamforming_report,
    wifi_mac_block_ack, wifi_mac_bssid_policy, wifi_mac_coex_init, wifi_mac_coex_runtime,
    wifi_mac_cold_handshake, wifi_mac_control, wifi_mac_core_enable, wifi_mac_crypto_control,
    wifi_mac_csi_dump, wifi_mac_diagnostic_statistics, wifi_mac_he_buffer_status,
    wifi_mac_he_color_collision, wifi_mac_he_init_prefix, wifi_mac_he_init_suffix,
    wifi_mac_he_mu_edca_timer, wifi_mac_he_obss_narrow_band_ru, wifi_mac_he_tb_diagnostics,
    wifi_mac_he_tb_statistics, wifi_mac_he_trigger_rx_diagnostics, wifi_mac_interface_address,
    wifi_mac_internal_tx_block_ack, wifi_mac_interrupt, wifi_mac_key_table,
    wifi_mac_last_rx_buffer, wifi_mac_local_time, wifi_mac_power_interrupt,
    wifi_mac_regdma_control, wifi_mac_rtc_timer_update, wifi_mac_rx_bssid_list,
    wifi_mac_rx_csi_control, wifi_mac_rx_custom_type, wifi_mac_rx_dma, wifi_mac_rx_end_state,
    wifi_mac_rx_filter, wifi_mac_rx_hang_statistics, wifi_mac_rx_power_save,
    wifi_mac_rx_runtime_results, wifi_mac_rx_statistics, wifi_mac_sta_beacon_filter,
    wifi_mac_sta_tbtt_target, wifi_mac_tsf_load, wifi_mac_tsf_status, wifi_mac_tsf_timer,
    wifi_mac_tx_common, wifi_mac_tx_completion, wifi_mac_tx_power_init,
    wifi_mac_tx_queue_control, wifi_mac_tx_queue_vector, wifi_mac_tx_statistics,
    wifi_mac_txrx_callbacks, wifi_mac_txrx_prefix, wifi_mac_txrx_suffix, wifi_phy_rate_oracle,
};

pub(crate) use ownership::device_fence;
pub(crate) mod bluetooth;
pub(crate) mod ieee802154;
pub(crate) mod modem;
/// Owner acquisition and register transactions of isolated probe images.
#[cfg(oer_probe)]
#[doc(hidden)]
pub mod validation;
pub mod watchdog;

// Generated analog-I2C identities, fields and transport routes.
include!("analog_generated.rs");
