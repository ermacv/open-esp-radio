//! Bluetooth LE HIL workloads and the Linux Bluetooth fixture they drive.
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

pub mod fixture;
pub mod link;
pub mod scenario;
pub mod workload;

pub(crate) use oer_hil_workload::Result;
pub(crate) use oer_hil_workload::emit_json;

/// The Bluetooth family's key in a scenario document.
pub const FAMILY: oer_hil_workload::family::Kind =
    oer_hil_workload::family::Kind::of::<scenario::BluetoothScenario>("bluetooth");
