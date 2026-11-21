//! The typed vendor-comparison scenarios: a chip-neutral engine, the
//! chip's scenarios and the reviewer reports.
//!
//! The engine modules are chip-neutral: authenticated artifacts, Blobray
//! sessions, the comparison harness, observation and state analysis. A
//! chip's module ([`esp32s31`]) installs its [`chip::Chip`] and builds its
//! scenarios on them; chip addresses, pins and reviewed decisions stay
//! there. [`report`] holds the reviewer reports and the command line the
//! `oer-tools-vendor-scenario-cli` binary runs.
#![expect(
    clippy::cast_possible_truncation,
    reason = "host tool: the rule covers the code an image reaches"
)]
#![expect(
    clippy::cast_possible_wrap,
    reason = "host tool: the rule covers the code an image reaches"
)]
#![expect(
    clippy::cast_sign_loss,
    reason = "host tool: the rule covers the code an image reaches"
)]
#![expect(
    clippy::let_underscore_must_use,
    reason = "host tool: the rule covers the code an image reaches"
)]
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
#![expect(
    clippy::arithmetic_side_effects,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::unreachable,
    clippy::unwrap_used,
    reason = "host tool: the ban covers the code an image reaches"
)]
pub mod artifacts;
pub mod chip;
pub mod discovery;
pub mod esp32s31;
pub mod evidence;
pub mod failure;
pub mod findings;
pub mod harness;
pub mod leaf;
pub mod observation;
pub mod phy;
pub mod registers;
pub mod report;
pub mod rule_use;
pub mod session;
pub mod state;

pub use chip::{Chip, Isa, chip, install};
