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

//! The Bluetooth LE port's contract (`design/ble-port-contract-2026-11-19.md`):
//! the boundary between a chip's Bluetooth LE port and the sans-IO cores
//! above it.
//!
//! - [`LePort`]: the radio's life, the roles' openings and closings, the
//!   publications of events, the transmit chain, a [`Round`] of what the
//!   hardware finished, the alarm and the facts. No call answers an error.
//! - The rights ([`rights`]): every state a call needs is a right it takes,
//!   branded with the installation and made only by this crate, from the
//!   mint ([`PortPools`]) or by the port's [`PortKey`].
//! - The geometry: the const generics of [`LePort`] and [`PortPools`],
//!   checked against the capabilities at build.
//! - The vocabulary: the clock domain, the request values of every role,
//!   the capabilities with [`Supported`], and the DTM window arithmetic
//!   ([`dtm`]).
//! - [`CipherOffload`]: declared, with no implementor.

mod capabilities;
mod cipher;
pub mod dtm;
mod geometry;
mod port;
mod request;
pub mod rights;
mod supported;

pub use capabilities::{
    LeCapabilities, LeConnectionCapabilities, LeLimits, LePhy, LePhys, LeRadioCapabilities,
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
pub use cipher::{CipherOffload, Direction, Iv, KeySlot, Keyed, SessionKey};
pub use port::{
    Cut, Ended, EventEnd, Executed, Facts, LePort, LeRounds, MoreData, NotExecuted, Pdu, PduLen,
    PduTooLong, Published, Received, Rest, Round, RoundSeal, Signal, StartFailure, StartPoll,
    StopPoll, TxEnd,
};
pub use request::{
    AboveCeiling, AcceptEntries, AdvertisingData, AdvertisingEvent, AdvertisingEventKind,
    AdvertisingFilterPolicy, AdvertisingReception, AdvertisingSet, ChannelSpacing,
    ConnectionAllowances, ConnectionEvent, ConnectionEventKind, ConnectionEventTiming,
    ConnectionPriority, ConnectionSetup, LeBand, LeTxCeiling, Operation, OutsideEpoch, RadioTiming,
    ScanFilterPolicy, ScanType, ScanWindow, ScannerSetup, TestPhy, TestWindow, TxPower,
};
pub use rights::{
    AcceptList, AdvEvent, AdvPlace, Advertiser, ConnEvent, ConnPlace, Connection, EventPlace,
    Install, PduHandle, PortKey, PortPools, Retired, RoleRef, RoleRights, Running, ScanEvent,
    ScanPlace, Scanner, Starting, StopFrom, StopRight, Stopped, Stopping, Test, TestEvent,
    TestPlace, TxPlace,
};
pub use supported::{Capability, Supported, Timed, Unplaced, Untimed};
