#![no_std]
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

//! Scheduler-free Embassy platform runtime for Espressif staged-boot
//! applications; a chip feature selects the chip of esp-hal.
//!
//! This crate owns only executor wake-up and timer-queue integration. It does
//! not own radio policy, board electrical parameters, credentials, sockets or
//! a host protocol. Optional timer observations and idle accounting describe only
//! its own execution. Applications supply the ESP-HAL software
//! interrupt and timer capabilities explicitly, with the tokens of their
//! sources in the image's interrupt table, whose entries name
//! `wake_handler` and `timer_interrupt`.

#[cfg(feature = "chip")]
mod executor;
#[cfg(feature = "chip")]
mod time_driver;
#[cfg(any(feature = "chip", test))]
mod timer_queue;

#[cfg(feature = "chip")]
pub use executor::{Executor, wake_handler};
#[cfg(feature = "chip")]
pub use time_driver::{Timer, init, timer_interrupt};

#[cfg(any(feature = "timer-observation", test))]
pub mod timer_observation;

#[cfg(any(feature = "idle-accounting", test))]
pub mod idle_accounting;
