#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros,
    clippy::disallowed_methods
)]
#![cfg_attr(
    test,
    allow(
        clippy::disallowed_macros,
        clippy::disallowed_methods,
        clippy::arithmetic_side_effects
    )
)]

//! LE DTM reference backend and a separate deterministic radio environment.
//!
//! The environment owns virtual time and propagates generated test packets
//! between independent radios. The registered P2 backend owns commands and
//! terminal reservations; P1 only admits work and dequeues produced terminals.
//! No executor, real-time waits or global state are used.
//!
//! ```
//! use core::num::NonZeroU64;
//! use oer_bluetooth_le_model::Environment;
//! use oer_time::{Duration, Timer};
//! let air = Environment::new(NonZeroU64::new(1_000).expect("profile period"));
//! let start = air.now();
//! air.advance(Duration::from_micros(10_000)).expect("virtual time fits");
//! assert_eq!(air.now().checked_duration_since(start), Some(Duration::from_micros(10_000)));
//! assert_eq!(air.active_radios(), 0);
//! ```
extern crate alloc;

#[cfg(test)]
extern crate std;

mod air;
mod capacity;
/// Executable reference fixture for the portable suite.
#[cfg(feature = "test-support")]
pub mod conformance;
mod lifecycle;
mod queues;
mod storage;
pub use air::{Environment, Loss, Stimulus};
pub use lifecycle::{Backend, Frontend, Invocation, Resources, Vacant, View};
pub use queues::{ModelPhase, Queues};

use oer_bluetooth_le_port::{LeCapabilities, LePhys, LeRadioCapabilities};
/// DTM 1M-only profile. No advertising, scanning or connection capability is
/// inferred from a test receiver.
pub enum ModelCaps {}
impl LeCapabilities for ModelCaps {
    const CAPS: LeRadioCapabilities = LeRadioCapabilities {
        direct_test_mode: true,
        test_phys: LePhys::LE_1M,
        min_tx_power_dbm: -40,
        ..LeRadioCapabilities::NONE
    };
}
/// Opaque backend causes, passed through portable code unchanged.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Fault {
    /// Ordinary disable completed the stream.
    Disabled,
    /// Explicit fault stimulus in the environment.
    Injected,
    /// An event sink rejected a terminal; every owner stays in quarantine.
    RejectedTerminal,
    /// Installation generation cannot advance.
    EpochExhausted,
    /// Queue backing has not received an installation's capacity mint.
    UnpreparedQueues,
}

#[cfg(test)]
mod tests;
