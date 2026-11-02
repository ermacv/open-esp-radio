//! What the family's application entry (`oer`) shares across its chips:
//! the boot's identity and the lowest permitted channel ([`boot`]), the
//! typed channel between an image's task groups ([`channel`]), the IP
//! service's published status ([`net`]) and, with `observe`, the counters
//! the library keeps of its own work ([`observe`]). A chip's application
//! crate (`chips/<chip>/app`) builds on them; `oer` re-exports what an image
//! uses.

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

#[cfg_attr(not(target_arch = "riscv32"), allow(dead_code))]
pub mod boot;
#[cfg_attr(not(target_arch = "riscv32"), allow(dead_code))]
pub mod channel;
pub mod net;
#[cfg(any(test, feature = "observe"))]
pub mod observe;
