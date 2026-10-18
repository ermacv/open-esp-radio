//! The Wi-Fi fixtures of HIL runs, grouped by provider, and the fixture
//! provider the runner composes ([`provider::PROVIDER`]).
//!
//! `local` (the laptop radio, hostapd and wpa_supplicant, its air monitor)
//! and `openwrt` (the SSH-managed router: AP, client, captures and monitors)
//! own their provider's resources and evidence; `controlled_ap` selects
//! between them and `prepared` owns a scenario's AP lifetime. `host_network`
//! proves the host's route to a station target, `probe_load` drives the
//! probe-request load, and `hostapd` builds the pinned hostapd the Linux
//! fixture installs. `air` types the frames of a monitor capture and
//! `protection` judges how a station follows BSS protection on the air.
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

pub mod air;
mod capture_process;
mod channel;
pub mod controlled_ap;
pub mod host_network;
pub mod local;
pub mod openwrt;
pub mod prepared;
pub mod probe_load;
pub mod protection;
pub mod provider;
pub mod station_fixture;
#[cfg(all(test, unix))]
mod test_support;

pub use oer_hil_lab::Error;
pub(crate) use oer_hil_workload::Result;
