#![cfg_attr(target_arch = "riscv32", no_std, no_main)]
#![forbid(unsafe_code)]
#![cfg_attr(target_arch = "riscv32", feature(type_alias_impl_trait))]

//! A Wi-Fi station of its build profile. Without `OER_WIFI_STATION_PROFILE`
//! the image reports NotConfigured and starts no RF; a configured image
//! joins its profile's network with automatic reconnection.

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
