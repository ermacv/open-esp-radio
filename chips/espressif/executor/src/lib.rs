#![no_std]
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

//! Scheduler-free Embassy platform runtime for Espressif staged-boot
//! applications; a chip feature selects the chip of esp-hal.
//!
//! This crate owns only executor wake-up and timer-queue integration. It does
//! not own radio policy, board electrical parameters, credentials, sockets or
//! a host protocol. Timer observations, idle accounting and poll timing,
//! compiled for an image that declares the profile class (`cfg(oer_profile)`),
//! describe only its own execution. Applications supply the ESP-HAL
//! software interrupt capabilities and the system timer explicitly, with the
//! tokens of their sources in the image's interrupt table.
//!
//! - `Executor<SWI>` runs in thread mode, woken by its software interrupt,
//!   whose entry names `wake_handler::<SWI>`.
//! - `InterruptExecutor<SWI>` polls in its software interrupt's handler
//!   (`interrupt_handler::<SWI>`), at that interrupt's level: the radio
//!   executor (P2). Starting it takes a `LevelOrder`, which checks when the
//!   image compiles that the radio interrupts preempt it and it preempts the
//!   application.
//! - `Doorbell<SWI>` is a software interrupt at the radio interrupts' level
//!   (P3, `FROM_CPU` line 3) that the radio executor rings to have a request
//!   applied there; its handler starts with `DoorbellAcknowledger::acknowledge`. Making it
//!   takes a `SameLevel`, which checks when the image compiles that it shares
//!   the radio interrupt's level and core.
//! - The time driver (`init`, `bind`) gives each executor its own timer
//!   queue and SYSTIMER comparator, whose alarm (`alarm_interrupt::<N>`) only
//!   acknowledges and pends; one executor writes the SYSTIMER registers. No
//!   lock is taken on any of these paths; the module documentation of
//!   `time_driver` has the details.

#[cfg(any(feature = "chip", test))]
mod deadline_cell;
#[cfg(feature = "chip")]
mod doorbell;
#[cfg(feature = "chip")]
mod executor;
#[cfg(any(feature = "chip", test))]
mod level;
#[cfg(feature = "chip")]
mod time_driver;
#[cfg(any(feature = "chip", test))]
mod timer_queue;

#[cfg(feature = "chip")]
pub use doorbell::{Doorbell, DoorbellAcknowledger};
#[cfg(feature = "chip")]
pub use executor::{
    Executor, InterruptExecutor, LevelOrder, SameLevel, interrupt_handler, wake_handler,
};
#[cfg(feature = "chip")]
pub use time_driver::{alarm_interrupt, bind, init};

#[cfg(any(all(oer_profile, feature = "chip"), test))]
pub mod timer_observation;

#[cfg(any(all(oer_profile, feature = "chip"), test))]
pub mod idle_accounting;

#[cfg(any(all(oer_profile, feature = "chip"), test))]
pub mod poll_timing;
