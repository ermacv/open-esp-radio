//! IEEE 802.15.4 HIL workloads.
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

pub mod peer;
pub mod scenario;
pub mod thread_peer;
pub mod workload;

pub(crate) use oer_hil_workload::Result;

/// The IEEE 802.15.4 family's key in a scenario document.
pub const FAMILY: oer_hil_workload::family::Kind =
    oer_hil_workload::family::Kind::of::<scenario::Ieee802154Scenario>("ieee802154");
