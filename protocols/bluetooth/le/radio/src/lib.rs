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

//! Hardware-independent Bluetooth LE radio values.
//!
//! The values a Link Layer controller and a radio backend share: the
//! `LeRadio` clock domain, channels, Link Layer PDUs, role configuration and
//! event geometry in radio-domain microseconds ([`RadioTiming`],
//! [`ConnectionAllowances`]), the capabilities a backend states
//! ([`LeRadioCapabilities`]) and the [`Supported`] proof that a value fits
//! them, and what an event observed ([`EventResult`], [`TestReport`]).
//! Descriptor images, register fields and controller ticks stay in the
//! backend.
//!
//! The LE request/outcome contract and host reference backend live in the
//! sibling `oer-bluetooth-le-port` and `oer-bluetooth-le-model` crates.

#[cfg(test)]
extern crate std;

mod capabilities;
mod channel;
mod outcome;
mod pdu;
mod request;
mod supported;

pub use capabilities::{
    LeCapabilities, LeConnectionCapabilities, LePhy, LePhys, LeRadioCapabilities,
    LegacyAdvertisingTypes, LinkAcknowledgement, NoCapabilities,
};
pub use channel::{
    AdvertisingChannel, AdvertisingChannels, ChannelError, DataChannel, TestChannel,
};
pub use oer_time::{NonZeroRadioDuration, RadioDuration, WindowError};

/// The clock domain of the LE radio: its instants never mix with another
/// radio's.
pub enum LeRadio {}

/// An instant on the LE radio's clock.
pub type LeInstant = oer_time::RadioInstant<LeRadio>;

/// A window reserved on the LE radio's clock.
pub type LeWindow = oer_time::RadioWindow<LeRadio>;
pub use outcome::{EventResult, ReceivedPdu, TestReport};
pub use pdu::{AdvertisingPdu, DataPdu, DataPduKind, PduError, TestPayloadType};
pub use request::{
    AboveCeiling, AcceptListChange, AcceptListDevice, AccessAddress, AdvertisingConfiguration,
    AdvertisingEvent, AdvertisingEventKind, AdvertisingReception, AdvertisingSetId,
    ConnectionAllowances, ConnectionConfiguration, ConnectionEvent, ConnectionEventKind,
    ConnectionEventTiming, ConnectionId, CrcInit, LeBand, LeTxCeiling, Operation, OutsideEpoch,
    RadioTiming, ScanFilterPolicy, ScanType, ScanWindow, ScannerConfiguration, ScannerId, TestPhy,
    TestReceive, TestTransmit, TxPower,
};
pub use supported::{Capability, Supported, Timed, Unplaced, Untimed};
