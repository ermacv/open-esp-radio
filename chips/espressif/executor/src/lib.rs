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
//! staged-boot applications. The executor names no chip: its code is
//! generic over a chip binding `B` (`Binding`), whose platform port the chip
//! HAL implements, and the chip's composition crate declares the binding's
//! statics once with `bind!`: the storage of the port's owned blocks, the
//! one waker vtable and the Embassy time driver's symbols. The reset root
//! supplies the port's owned blocks; interrupt parts are bound through the
//! image's typed interrupt router. The executor owns task execution,
//! wake-up and timer-queue integration.
//!
//! - Each executor's tasks are declared by [`tasks!`] into static slots and
//!   polled by a direct call per task; [`task`] describes the slots, the
//!   ready mask and the binding's one waker.
//! - `Executor<B, SWI>` runs its tasks in thread mode, woken by its software
//!   interrupt, whose entry names `wake_handler::<B, SWI, _>`.
//! - `InterruptExecutor<B, SWI>` polls its tasks in its software interrupt's
//!   handler (`interrupt_handler::<Tasks, _>`), at that interrupt's level: the
//!   radio executor (P2). Signal declarations and request bindings check
//!   their direction against the entries' levels and harts when the image compiles.
//! - `Doorbell<B, SWI, HandlerEntry, ExecutorEntry>` is a software interrupt at the radio interrupts' level
//!   (P3, `FROM_CPU` line 3) that the radio executor rings to have a request
//!   applied there; its handler starts with `DoorbellAcknowledger::acknowledge`. Its line
//!   is a source of the radio handler's part: the ringing half takes the
//!   router's `Bound` proof of that part (`Doorbell::new`).
//! - [`peer`]: a request from a task on one hart to a task on another over
//!   one in-place request slot, both directions notified by raising the
//!   other executor's signal (`FromPeerHart`); wakers never cross harts.
//! - The time driver (`init`, `bind`) gives each executor its own timer
//!   queue and SYSTIMER comparator, whose alarm (`alarm_interrupt::<B, N, _>`) only
//!   acknowledges and pends; one executor writes the SYSTIMER registers. No
//!   lock is taken on any of these paths; the module documentation of
//!   `time_driver` has the details.

pub mod observation;

#[doc(hidden)]
pub use oer_trace as __trace;

#[cfg(feature = "chip")]
mod binding;
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
pub mod peer;
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
pub use software_line::SoftwareInterrupt;
#[cfg(any(feature = "chip", test))]
pub mod task;
#[cfg(feature = "chip")]
mod time_driver;
#[cfg(any(feature = "chip", test))]
mod timer_queue;
#[cfg(any(feature = "chip", test))]
mod urgent_sources;

#[cfg(feature = "chip")]
pub use binding::{Binding, Storage, TimeDriver};
#[cfg(feature = "chip")]
pub use doorbell::{Doorbell, DoorbellAcknowledger, DoorbellLine, LineAcknowledger};
#[cfg(feature = "chip")]
pub use executor::{
    Executor, InterruptContext, InterruptExecutor, ThreadContext, interrupt_handler, ready_peers,
    wake_handler,
};
#[cfg(any(feature = "chip", test))]
pub use task::{AlreadyStarted, Spawner, TaskId, Tasks};
#[cfg(feature = "chip")]
pub use time_driver::{TimeDriverReady, TimeSetup, alarm_interrupt, bind, init};
#[cfg(feature = "chip")]
pub use urgent_sources::{UrgentSource, UrgentSources};

/// What the expansion of [`bind!`] names in this crate.
#[cfg(feature = "chip")]
#[doc(hidden)]
pub mod __bind {
    pub use embassy_time_driver::Driver;
}
