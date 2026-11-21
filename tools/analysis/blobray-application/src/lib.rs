//! Blobray operations inside the calling process.
//!
//! Callers supply captured executables as bytes, each identified by the
//! SHA-256 of its content; requests name executables, objects and symbols by
//! that identity. No project, store, journal or worker participates, and every
//! result stays with the caller.
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
    clippy::indexing_slicing,
    clippy::unreachable,
    clippy::unwrap_used,
    reason = "host tool: the ban covers the code an image reaches"
)]

use oer_tools_analysis_blobray_artifacts::{MemberCursor, inspect_source};
use oer_tools_analysis_blobray_domain::*;
use oer_tools_analysis_riscv_model::*;
use std::path::Path;

pub mod access_groups;
pub mod call_arguments;
pub mod captured;
mod code_coverage;
mod command_bank;
pub mod data;
mod dependence;
mod devices;
mod execution;
mod execution_coverage;
mod execution_memory;
mod execution_steps;
mod external_calls;
pub mod field;
pub mod in_process;
pub mod library;
pub mod linking;
pub mod mmio_diff;
pub mod references;
mod registers;
pub mod symbols;

pub use execution::EXECUTION_ENVIRONMENT;
pub use oer_tools_analysis_blobray_verification::VERIFIER as EXECUTION_VERIFIER;

pub mod signatures;
