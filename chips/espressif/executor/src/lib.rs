#![no_std]
#![cfg_attr(test, feature(type_alias_impl_trait))]
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]
#![cfg_attr(not(test), deny(clippy::panic, clippy::expect_used))]
#![deny(
    clippy::unwrap_used,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented
)]

//! Scheduler-free static task executor and time driver for Espressif
//! staged-boot applications. The reset root supplies owned HAL blocks;
//! interrupt parts are bound through the image's typed interrupt router.
//! The executor owns task execution, wake-up and timer-queue integration.
//!
//! - Each executor's tasks are declared by [`tasks!`] into static slots and
//!   polled by a direct call per task; [`task`] describes the slots, the
//!   ready mask and the one waker.
//! - `Executor<SWI>` runs its tasks in thread mode, woken by its software
//!   interrupt, whose entry names `wake_handler::<SWI>`.
//! - `InterruptExecutor<SWI>` polls its tasks in its software interrupt's
//!   handler (`interrupt_handler::<Tasks>`), at that interrupt's level: the
//!   radio executor (P2). Signal declarations and request bindings check
//!   their direction against the entries' levels and harts when the image compiles.
//! - `Doorbell<SWI, HandlerEntry, ExecutorEntry>` is a software interrupt at the radio interrupts' level
//!   (P3, `FROM_CPU` line 3) that the radio executor rings to have a request
//!   applied there; its handler starts with `DoorbellAcknowledger::acknowledge`. Its line
//!   is a source of the radio handler's part: the ringing half takes the
//!   router's `Bound` proof of that part (`Doorbell::new`).
//! - The time driver (`init`, `bind`) gives each executor its own timer
//!   queue and SYSTIMER comparator, whose alarm (`alarm_interrupt::<N>`) only
//!   acknowledges and pends; one executor writes the SYSTIMER registers. No
//!   lock is taken on any of these paths; the module documentation of
//!   `time_driver` has the details.

#[cfg(feature = "chip")]
mod chip;
#[cfg(any(feature = "chip", test))]
mod context;
#[cfg(feature = "chip")]
mod cooperative;
#[cfg(any(feature = "chip", test))]
mod deadline_cell;
#[cfg(feature = "chip")]
mod doorbell;
#[cfg(feature = "chip")]
mod executor;
#[cfg(any(feature = "chip", test))]
mod fault;
#[cfg(any(feature = "chip", test))]
pub use fault::{Failure, Fault};
#[cfg(feature = "chip")]
mod hart_binding;
#[cfg(any(feature = "chip", test))]
mod poll_stamp;
#[cfg(feature = "port-slots")]
pub mod port_slot;
#[cfg(feature = "chip")]
pub use cooperative::Cooperative;
#[cfg(any(feature = "chip", test))]
mod software_line;
#[cfg(feature = "chip")]
pub mod systimer;
#[cfg(any(feature = "chip", test))]
pub mod task;
#[cfg(feature = "chip")]
mod time_driver;
#[cfg(any(feature = "chip", test))]
mod timer_queue;
#[cfg(any(feature = "chip", test))]
mod urgent_sources;

#[cfg(feature = "chip")]
pub use doorbell::{Doorbell, DoorbellAcknowledger, DoorbellLine, LineAcknowledger};
#[cfg(feature = "chip")]
pub use executor::{Executor, InterruptExecutor, interrupt_handler, ready_peers, wake_handler};
#[cfg(any(feature = "chip", test))]
pub use task::{AlreadyStarted, Spawner, TaskId, Tasks};
#[cfg(feature = "chip")]
pub use time_driver::{TimeSetup, alarm_interrupt, bind, init};
#[cfg(feature = "chip")]
pub use urgent_sources::{UrgentSource, UrgentSources};

#[cfg(any(all(oer_profile, feature = "chip"), test))]
pub mod timer_observation;

#[cfg(any(all(oer_profile, feature = "chip"), test))]
pub mod idle_accounting;

#[cfg(any(all(oer_profile, feature = "chip"), test))]
pub mod poll_timing;
