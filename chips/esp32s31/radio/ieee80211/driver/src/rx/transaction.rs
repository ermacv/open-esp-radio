//! One synchronous physical RX completion, staging and recycle transaction.
//!
//! All persistent owners remain with the caller. This layer borrows their
//! exact ring, storage and pool while an adapter owns publication and timing.

mod admission;
mod hooks;
mod observation;
mod publisher;
mod service;

pub use admission::*;
pub use hooks::{Hooks, Phase};
pub use observation::{Discard, ServiceObservation};
pub use publisher::Publisher;
pub use service::service;

/// Existing cumulative counters borrowed for exactly one service call.
/// Updates remain at their original commit point, including later errors.
pub struct Counters<'a> {
    pub descriptors: &'a mut u64,
    pub units: &'a mut u64,
    pub bytes: &'a mut u64,
}
