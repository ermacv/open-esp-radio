#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented,
    clippy::indexing_slicing
)]
//! The HIL agent on the `oer-espressif` application entry.
//!
//! A HIL test image builds its drivers and services through the same
//! `oer_espressif::main!` boot as an example of the same function and adds
//! only the `console` feature, overrides of the library's defaults and the
//! handlers of its services. This crate holds what every such image serves
//! besides its handlers:
//!
//! - [`Mailbox`]: the one-deep hand-over of a request from the console to
//!   the image's worker, which answers it in order; a second request while
//!   one waits is refused as busy;
//! - on the chip, [`serve`]: the HIL console over the library's USB
//!   console, which answers the base module (Hello, boot evidence, link
//!   health) from the library's boot identity and evidence beside the
//!   worker, and [`answer`], which publishes a response;
//! - on the chip, [`answer_then_trap`]: the answer of a fault request, after
//!   which the image traps on an illegal instruction, so that a scenario
//!   checks the exception path from the trap to the next boot's evidence.
//!
//! The boot evidence carries the previous boot's exception, when the
//! library read one, as its post-mortem (`Fault::Exception`): the trap's
//! registers apart from what the exception hook read after the stop.

mod mailbox;
pub use mailbox::{Busy, Inbox, Mailbox, Outbox};

#[cfg(target_arch = "riscv32")]
mod esp32s31;
#[cfg(target_arch = "riscv32")]
pub use esp32s31::{Image, answer, answer_then_trap, serve};
