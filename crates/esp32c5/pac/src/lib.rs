//! Closed typed ESP32-C5 radio peripheral access crate.
//!
//! The generated capability catalog comes from the reviewed
//! [register publication](../../../../registers/esp32c5/README.md). Owners
//! that expose its transactions are added per domain.

#![no_std]
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

#[cfg(test)]
extern crate std;

// The generated capability catalog is intentionally broader than this crate's
// restricted ownership facade. Reviewed leaves stay unreachable until an owner
// transition exposes them.
#[allow(
    dead_code,
    reason = "generated capability catalog is wider than the restricted ownership facade"
)]
mod generated;

use oer_esp32c5_pac_raw as svd;

pub(crate) mod ieee802154;
pub(crate) mod modem;
pub(crate) mod ownership;
pub(crate) mod phy;

pub(crate) use ownership::device_fence;

#[doc(hidden)]
pub use ieee802154::mac::{Ieee802154PolledRegisterLease, Ieee802154RegisterLease};

pub use ieee802154::mac::{
    Ieee802154AckTimeoutUnits, Ieee802154CcaMode, Ieee802154DebugCounter, Ieee802154EdCcaSnapshot,
    Ieee802154EdCommand, Ieee802154EdDurationUnits, Ieee802154EdSampleMode, Ieee802154EdSampleRate,
    Ieee802154EtmChannel, Ieee802154EtmRoute, Ieee802154Event, Ieee802154EventEnableState,
    Ieee802154EventMask, Ieee802154EventObservation, Ieee802154EventObservationError,
    Ieee802154FoundationSnapshot, Ieee802154FrequencyCode, Ieee802154MacCommand,
    Ieee802154MacConfigurationReadback, Ieee802154MacControl, Ieee802154MacPolicySnapshot,
    Ieee802154MultipanEnableState, Ieee802154MultipanIndex, Ieee802154ObservedEventState,
    Ieee802154OperationEventEnableObservation, Ieee802154OperationRxAbortEnableObservation,
    Ieee802154PanIdentity, Ieee802154Pti, Ieee802154RouteState, Ieee802154RxAbortEnableSet,
    Ieee802154RxAbortEnableState, Ieee802154RxAbortReason, Ieee802154RxAbortReasonObservation,
    Ieee802154RxStateCode, Ieee802154RxStatus, Ieee802154SecurityPayloadOffset,
    Ieee802154StateSnapshot, Ieee802154Timer0ThresholdWord, Ieee802154Timer0ValueWord,
    Ieee802154Timer1ThresholdWord, Ieee802154Timer1ValueWord, Ieee802154TimerLease,
    Ieee802154TransmitSecurityControl, Ieee802154TxAbortEnableSet, Ieee802154TxAbortReason,
    Ieee802154TxAbortReasonObservation, Ieee802154TxPowerCode, Ieee802154TxSecurityError,
    Ieee802154TxSecurityErrorObservation, Ieee802154TxStateCode, Ieee802154TxStatus,
    Ieee802154ValidationEdDurationState, Ieee802154ValidationEventEnableState,
};

pub use generated::PhyRxEvmParameter;
pub use modem::clock::{Ieee802154ResetObservation, ModemClockDevice, ModemClockRegisters};
pub use modem::etm::Ieee802154EtmChannels;
pub use ownership::{
    Ieee802154InterruptRegisters, Ieee802154InterruptSetup, Ieee802154Partition,
    Ieee802154TaskRegisters, RadioPartitions, WifiInterruptPartition, WifiMacPartition,
};
pub use phy::i2c::{
    PHY_I2C_INITIALIZATION_PAIR_COUNT, PhyI2cAccessError, PhyI2cAddress, PhyI2cBlock,
    PhyI2cClockSelection, PhyI2cConfiguration, PhyI2cConfigurationCommand, PhyI2cHost,
    PhyI2cInitializationInputs, PhyI2cParallelWrite, PhyI2cRcCalibration, PhyI2cRegisters,
    PhyI2cSar2Code,
};
pub use phy::radio::PhyRadioRegisters;
