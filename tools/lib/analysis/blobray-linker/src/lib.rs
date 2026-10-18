//! Linux adapters of in-process linking: the external ELF linkers behind the
//! application's `LinkerHost` port. The CLI does not link, so it does not
//! depend on this crate.
#![cfg(target_os = "linux")]
mod linker;
pub use linker::ElfLinker;

use blobray_domain::*;

use oer_riscv_model::*;
use std::{
    fs::File,
    path::Path,
    process::{Child, Command, Stdio},
};

pub fn io(error: std::io::Error) -> Error {
    storage_io(error)
}
