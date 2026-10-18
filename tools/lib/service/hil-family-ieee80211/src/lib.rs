//! Wi-Fi HIL workloads: the `[wifi]` scenario table, the station, access
//! point and monitor role workloads and the station traffic workloads, run
//! over the Wi-Fi fixtures (`oer-hil-family-ieee80211-fixture`), judged with
//! the Wi-Fi radio evidence (`oer-hil-family-ieee80211-evidence`) and the
//! network traffic of `oer-hil-net-traffic`.
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

pub mod link;
pub mod scenario;
pub mod workload;

pub(crate) use oer_hil_workload::Result;

/// The Wi-Fi family's key in a scenario document.
pub const FAMILY: oer_hil_workload::family::Kind =
    oer_hil_workload::family::Kind::of::<scenario::WifiScenario>("wifi");
