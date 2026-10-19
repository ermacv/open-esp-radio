//! System HIL workloads: boot smoke, watchdog, panic reset and stand recovery.
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

pub mod scenario;
pub mod workload;

pub(crate) use oer_hil_workload::Result;

/// The system family's key in a scenario document.
pub const FAMILY: oer_hil_workload::family::Kind =
    oer_hil_workload::family::Kind::of::<scenario::SystemScenario>("system");
