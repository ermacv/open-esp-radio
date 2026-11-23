#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]
#![cfg_attr(
    test,
    allow(
        clippy::disallowed_methods,
        clippy::disallowed_macros,
        clippy::unreachable,
        clippy::arithmetic_side_effects
    )
)]
//! The HIL agent: what every image that speaks the HIL protocol serves
//! besides its handlers, a probe and a test image on the `oer` entry alike.
//!
//! A probe differs from a scenario only in what it is allowed to use; the
//! work between the device and the host is done through this agent. The
//! agent stands below the entry, over the platform runtime's console and
//! records: an image hands [`serve`] its boot identity, what it read before
//! its boot ([`Records`]) and the value a console fault ends it with. It
//! holds:
//!
//! - [`Writer`]: the console's state, which the image places once and only
//!   the console's task touches;
//! - on the chip, [`serve`]: the HIL console over the platform runtime's
//!   USB console, which answers the base module (Hello, boot evidence, link
//!   health) from the image's boot identity and records, calls the image's
//!   handlers ([`Mount`]: what observes, answered in the step that decoded
//!   the request, and what commands, awaited one at a time), and traps on an
//!   illegal instruction once a command's answer that asks for it
//!   ([`After::Trap`], only for a set that serves the fault service) is
//!   written, so that a scenario checks the exception path from the trap to
//!   the next boot's evidence.
//!
//! The boot evidence carries the previous boot's exception, when the
//! image read one, as its post-mortem (`Fault::Exception`): the trap's
//! registers apart from what the exception hook read after the stop.

pub use oer_protocols_hil_agent::console::Writer;
pub use oer_protocols_hil_agent::serve::{self, After, Trap};

#[cfg(target_arch = "riscv32")]
mod esp32s31;
#[cfg(target_arch = "riscv32")]
pub use esp32s31::{Image, Mount, Records, serve};
