//! JSON wire documents and renderers of the `blobray` CLI.
#![expect(
    clippy::cast_possible_wrap,
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
pub mod access_groups;
pub mod call_arguments;
pub mod jump_table;
pub mod listing;
pub mod mmio;
pub mod partial;
pub mod references;
pub mod wire;
