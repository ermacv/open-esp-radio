#![no_std]
#![forbid(unsafe_code)]
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

//! The observation events of the IEEE 802.11 cores and runtime.
//!
//! An owner (the station, its data machine, the station service, the
//! ESP-NOW peer table) reports what happened with `oer_trace::observe!` to
//! its `O: Observer`, by default `oer_trace::Noop`. This crate holds the
//! event values: their domain ids, channels and two-word encodings, which
//! the image's collector and the host decoder share with the owners.
//! [`Observation::of`] turns any event an observer receives into one typed
//! value of this vocabulary, so a collector counts or forwards by `match`.
//!
//! The vocabulary is instrumentation, not a product interface: no owner
//! keeps a count of these events, and an image without `cfg(oer_trace)`
//! compiles none of them into its owners.

mod activity;
mod aggregation;
pub use aggregation::{
    AGGREGATION, BaStop, Evidence, ReorderCause, RxBaStarted, RxBaStopped, RxContainment,
    RxReorder, TxRateSelected,
};
mod arp;
mod esp_now;
mod event;
mod observation;
#[cfg(any(test, feature = "test-support"))]
mod recorder;
mod service;
mod station;

pub use activity::{
    ControlDelivered, StationActivity, StationAssociated, StationAuthenticated, StationBeacon,
    TxHardwareTimeout, TxIndeterminate, TxQueueCollision,
};
pub use arp::{ArpAddresses, ArpOperation, ArpReceived, ArpTransmitted};
pub use esp_now::{
    EspNowPeerTableChanged, EspNowPeersCleared, EspNowReplay, EspNowTxPacketNumber,
    PacketNumberOutcome, PeerChange, ReplayOutcome,
};
pub use event::{ARP, ESP_NOW, SERVICE, STATION};
pub use observation::Observation;
#[cfg(any(test, feature = "test-support"))]
pub use recorder::Recorder;
pub use service::{
    FrameDrop, GroupKeyInstalled, PortEventsLost, RxDrop, RxFrameDropped, RxMsduRefusal,
    RxMsduRefused, SecurityDrop, TxMsduTooLong, TxUnsent,
};
pub use station::{
    DataLoss, DataMsduLost, RsnDrop, RsnInputDropped, ScanBssObserved, ScanOutcome, TokenLost,
};

#[cfg(test)]
#[allow(clippy::disallowed_macros, clippy::disallowed_methods)]
mod tests;
