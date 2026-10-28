#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::todo,
    clippy::unimplemented,
    clippy::unreachable,
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
        clippy::arithmetic_side_effects,
        clippy::unreachable
    )
)]

//! IEEE 802.15.4 frame and information-element codecs.
//! Values are bytes and already computed fields; this crate owns no clock,
//! security counter, pending policy or radio lifecycle.

#[cfg(test)]
extern crate std;

/// Coordinated Sampled Listening IE encoding.
pub mod csl;
/// Enhanced acknowledgement construction.
pub mod enhanced_ack;
/// Bounded owned and borrowed MAC frames.
pub mod frame;
/// PHR and MAC header inspection.
pub mod header;
/// Thread Link Metrics IE encoding.
pub mod link_metrics;
/// Auxiliary security header encoding.
pub mod security;
/// Thread Time IE encoding from a ready network timestamp.
pub mod time_sync;

pub use csl::{CSL_IE_ID, CSL_IE_TEMPLATE, write_csl_ie};
pub use enhanced_ack::{
    EnhancedAck, EnhancedAckError, EnhancedAckSecurity, KeyIdMode, generate_enhanced_ack,
};
pub use frame::{Frame, FrameError, FrameView, MAX_MAC_FRAME_LEN, MIN_MAC_FRAME_LEN};
pub use header::{AddressMode, FrameAddress, FrameType, FrameVersion, PhrFrame};
pub use link_metrics::{ENH_ACK_PROBING_DATA_CAPACITY, ENH_ACK_PROBING_IE_CAPACITY, ProbingData};
pub use time_sync::write_time_ie;
