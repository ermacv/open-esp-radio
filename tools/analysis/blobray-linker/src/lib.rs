//! Linux adapters of in-process linking: the external ELF linkers behind the
//! application's `LinkerHost` port. The CLI does not link, so it does not
//! depend on this crate.
#![expect(
    clippy::let_underscore_must_use,
    reason = "host tool: the rule covers the code an image reaches"
)]
#![cfg(target_os = "linux")]
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
    clippy::unwrap_used,
    reason = "host tool: the ban covers the code an image reaches"
)]
mod linker;
pub use linker::ElfLinker;

use oer_tools_analysis_blobray_domain::*;

use oer_tools_analysis_riscv_model::*;
use std::{
    fs::File,
    path::Path,
    process::{Child, Command, Stdio},
};

pub fn io(error: std::io::Error) -> Error {
    storage_io(error)
}
