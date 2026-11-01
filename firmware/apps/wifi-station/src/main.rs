#![cfg_attr(target_arch = "riscv32", no_std, no_main)]
#![forbid(unsafe_code)]

//! A Wi-Fi station of its build profile, on the application entry
//! (`oer-espressif`). Without `OER_WIFI_STATION_PROFILE` the image reports
//! NotConfigured and starts no RF; a configured image joins its profile's
//! network with automatic reconnection, takes an IPv4 address by DHCP and
//! exchanges one line over TCP with the gateway's echo port.

#[cfg(all(target_arch = "riscv32", oer_wifi_station_configured))]
include!("entry.rs");
#[cfg(all(target_arch = "riscv32", not(oer_wifi_station_configured)))]
include!("not_configured.rs");
#[cfg(not(target_arch = "riscv32"))]
fn main() {}

#[cfg(all(test, not(target_arch = "riscv32")))]
#[path = "../build/profile.rs"]
mod profile_rules;
#[cfg(all(test, not(target_arch = "riscv32")))]
mod tests;
