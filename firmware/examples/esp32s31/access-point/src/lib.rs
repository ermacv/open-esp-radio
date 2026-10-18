#![no_std]
//! Application-owned IP services used by the ESP32-S31 AP example.

pub mod dhcp;
pub mod services;

#[cfg(not(feature = "owned-network"))]
compile_error!("owned Xarxa is the only network integration: enable the owned-network feature");
#[cfg(feature = "owned-network")]
extern crate embassy_net_owned as embassy_net;
pub mod network;
