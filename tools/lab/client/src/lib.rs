//! A client of the LAB service, the one owner of the stand's leases and
//! hardware: the IPC protocol ([`protocol`]) and the connection a command
//! holds its lease by ([`client`]). `cargo fw`, `cargo hil` and `cargo
//! stand` depend on this crate alone; the service's scheduler and drivers
//! stay out of their builds.
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

pub mod client;
pub mod peer;
pub mod protocol;
