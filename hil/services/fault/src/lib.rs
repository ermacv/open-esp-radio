#![no_std]
#![forbid(unsafe_code)]
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
//! The HIL fault service: the host has an image raise a CPU exception, to
//! check its exception path end to end. The image answers first; once the
//! answer is written it traps on an illegal instruction, records the
//! exception and resets, and the next boot's evidence holds the record
//! (`base::BootEvidence::post_mortem`).
//!
//! Only test images mount it: a request ends the boot that takes it.

use postcard_schema::Schema;
use serde::{Deserialize, Serialize};

oer_hil_protocol::messages! {
    endpoint RaiseException = "fault/raise-exception" => Raising;
    topic Raising = "fault/raising";
}

/// Raise an illegal-instruction exception on the image's application
/// hart once the answer is written.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct RaiseException;

/// The answer to [`RaiseException`], written before the trap: the boot
/// ends after it.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Raising;

oer_hil_agent::service! {
    /// The fault service's requests.
    pub enum Request (sessions = false, handler = Handler) {
        RaiseException(RaiseException) => raise_exception,
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;

    #[test]
    fn the_service_names_its_endpoint_and_answer_by_distinct_keys() {
        let keys = <Request as oer_hil_agent::base::Service>::KEYS;
        assert_eq!(keys.len(), 2);
        assert_ne!(keys[0], keys[1]);
    }
}
