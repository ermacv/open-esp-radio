#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros,
    clippy::disallowed_methods
)]

//! The Bluetooth LE radio's vocabulary: its clock domain, the request values
//! of every role, the capabilities, the outcomes and the DTM window arithmetic.

mod capabilities;
pub mod dtm;
mod outcome;
mod request;
mod supported;

pub use capabilities::{
    LeCapabilities, LeConnectionCapabilities, LePhy, LePhys, LeRadioCapabilities,
    LegacyAdvertisingTypes, LinkAcknowledgement, NoCapabilities,
};
pub use oer_portable_time::{NonZeroRadioDuration, RadioDuration, WindowError};

/// The clock domain of the LE radio: its instants never mix with another
/// radio's.
pub enum LeRadio {}

/// An instant on the LE radio's clock.
pub type LeInstant = oer_portable_time::RadioInstant<LeRadio>;

/// A window reserved on the LE radio's clock.
pub type LeWindow = oer_portable_time::RadioWindow<LeRadio>;
pub use outcome::{EventResult, ReceivedPdu, TestReport};
pub use request::{
    AboveCeiling, AcceptListChange, AcceptListDevice, AdvertisingConfiguration, AdvertisingEvent,
    AdvertisingEventKind, AdvertisingReception, AdvertisingSetId, ConnectionAllowances,
    ConnectionConfiguration, ConnectionEvent, ConnectionEventKind, ConnectionEventTiming,
    ConnectionId, LeBand, LeTxCeiling, Operation, OutsideEpoch, RadioTiming, ScanFilterPolicy,
    ScanType, ScanWindow, ScannerConfiguration, ScannerId, TestPhy, TestReceive, TestTransmit,
    TxPower,
};
pub use supported::{Capability, Supported, Timed, Unplaced, Untimed};
