//! The LAB service: one process per host owns the stand's queue, its leases
//! and every access to the leased hardware; `cargo fw`, `cargo hil` and
//! `cargo stand` are its clients over one Unix socket (`oer_tools_lab_client`).
//!
//! The service keeps the scheduler of `oer-tools-lab-arbiter`: who is granted
//! what, and which resources must be recovered first. Its drivers
//! ([`service::Drivers`]) run the operations and the recoveries.
#![expect(
    clippy::cast_possible_truncation,
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
    reason = "host tool: the ban covers the code an image reaches"
)]

pub mod router;
pub mod run;
pub mod service;
pub mod stand;
