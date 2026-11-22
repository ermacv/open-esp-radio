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
//! - [`Mailbox`]: the one-deep hand-over of a request from the console to
//!   the image's worker, which answers it in order; a second request while
//!   one waits is refused as busy;
//! - the console's lanes at the agent's depth ([`Lanes`], [`Producer`],
//!   [`LaneDrain`]): the image places them once with the console's
//!   [`Writer`] and gives each producing context its [`Producer`]; a
//!   producing context is a level of the console's hart;
//! - on the chip, [`serve`]: the HIL console over the platform runtime's
//!   USB console, which answers the base module (Hello, boot evidence, link
//!   health) from the image's boot identity and records beside the worker,
//!   and [`answer`], which publishes a response through its context's
//!   producer;
//! - on the chip, [`answer_then_trap`]: the answer of a fault request, after
//!   which the image traps on an illegal instruction, so that a scenario
//!   checks the exception path from the trap to the next boot's evidence.
//!
//! The boot evidence carries the previous boot's exception, when the
//! image read one, as its post-mortem (`Fault::Exception`): the trap's
//! registers apart from what the exception hook read after the stop.

mod mailbox;
pub use mailbox::{Busy, Inbox, Mailbox, Outbox};

pub use oer_protocols_hil_agent::console::{Ends, Writer};

/// The messages a lane holds until the writer takes them.
pub const DEPTH: usize = 8;
/// The image's `K` lanes, one a producing context.
pub type Lanes<const K: usize> = oer_protocols_hil_agent::console::Lanes<K, DEPTH>;
/// A producing context's handle on its lane.
pub type Producer = oer_protocols_hil_agent::console::Producer<'static, DEPTH>;
/// The writer's end of a lane.
pub type LaneDrain = oer_protocols_hil_agent::console::LaneDrain<'static, DEPTH>;

#[cfg(target_arch = "riscv32")]
mod esp32s31;
#[cfg(target_arch = "riscv32")]
pub use esp32s31::{Image, Mount, Records, answer, answer_then_trap, serve};
