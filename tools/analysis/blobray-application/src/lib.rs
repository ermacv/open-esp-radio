//! Blobray operations inside the calling process.
//!
//! Callers supply captured executables as bytes, each identified by the
//! SHA-256 of its content; requests name executables, objects and symbols by
//! that identity. No project, store, journal or worker participates, and every
//! result stays with the caller.

#![forbid(unsafe_code)]

use oer_analysis_blobray_artifacts::{MemberCursor, inspect_source};
use oer_analysis_blobray_domain::*;
use oer_analysis_riscv_model::*;
use std::path::Path;

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
pub mod in_process;
pub mod library;
pub mod linking;
pub mod mmio_diff;
mod registers;

pub use execution::EXECUTION_ENVIRONMENT;
pub use oer_analysis_blobray_verification::VERIFIER as EXECUTION_VERIFIER;

pub mod signatures;
