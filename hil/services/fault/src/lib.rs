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
//! The service also refuses one publication, to check the station's restart
//! on hardware: the chip must run on, and [`GetRestarts`] counts it.
//!
//! Only test images mount it.

use postcard_schema::Schema;
use serde::{Deserialize, Serialize};

oer_protocols_hil_protocol::messages! {
    endpoint RaiseException = "fault/raise-exception" => Raising;
    topic Raising = "fault/raising";
    endpoint RefusePublication = "fault/refuse-publication" => Refusing;
    topic Refusing = "fault/refusing";
    endpoint GetRestarts = "fault/restarts" => Restarts;
    topic Restarts = "fault/restarts-report";
}

/// Raise an illegal-instruction exception on the image's application
/// hart once the answer is written.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct RaiseException;

/// The answer to [`RaiseException`], written before the trap: the boot
/// ends after it.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Raising;

/// Refuse the station's next publication.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct RefusePublication;

/// The answer to [`RefusePublication`].
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Refusing;

/// Ask how often the station restarted in this boot.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct GetRestarts;

/// The longest restart cause the answer carries.
pub const MAX_CAUSE: usize = 64;

/// The answer to [`GetRestarts`]: how often the station restarted in this
/// boot and the last restart's cause, as its console line would read.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum Restarts {
    Counted {
        count: u32,
        cause: heapless::String<MAX_CAUSE>,
    },
    /// A restart was being recorded: ask again.
    Recording,
}

oer_protocols_hil_agent::service! {
    /// The fault service's requests.
    pub enum Request (sessions = false, handler = Handler) {
        RaiseException(RaiseException) => raise_exception,
        RefusePublication(RefusePublication) => refuse_publication,
        GetRestarts(GetRestarts) => get_restarts,
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;

    #[test]
    fn a_restart_cause_fits_the_console_line_kept() {
        let cause = heapless::String::<MAX_CAUSE>::try_from("wifi-restarted Port/Unpublished/Key")
            .expect("the injected cause fits");
        assert!(cause.len() < MAX_CAUSE);
    }

    #[test]
    fn the_service_names_its_endpoint_and_answer_by_distinct_keys() {
        let keys = <Request as oer_protocols_hil_agent::base::Service>::KEYS;
        assert_eq!(keys.len(), 6);
        for (index, key) in keys.iter().enumerate() {
            assert!(!keys[..index].contains(key));
        }
    }
}
