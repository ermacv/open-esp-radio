#![no_std]
#![forbid(unsafe_code)]

//! Executor-independent ESP32-S31 access-point implementation.
//!
//! This crate joins the portable AP protocol with S31-specific key slots and
//! the role-neutral radio owner. DMA, IRQ routing and deadlines remain owned
//! by the runtime adapter which materializes this role.

#[cfg(test)]
extern crate std;

pub mod ampdu;
pub mod engine;
pub mod hardware;
pub mod profile;
pub mod rx;
pub mod security;
pub mod transaction;
pub mod tx;

pub use oer_ieee80211_ap as protocol;
