#![cfg_attr(target_arch = "riscv32", no_std, no_main)]
#![forbid(unsafe_code)]
#![cfg_attr(target_arch = "riscv32", feature(type_alias_impl_trait))]

//! The smallest staged image: the bootstrap stages it into PSRAM, it takes
//! the reset root, adopts the mapping, reports the panic record the previous
//! boot left, if any, and sends typed diagnostics the host renders as
//! `OER-RESULT: pass boot-smoke`. The boot hart's executor feeds the RTC
//! watchdog the root armed and supervises hart 1, whose own executor answers
//! the boot hart's request, runs an AES known answer on hart 1's timers and
//! echoes its liveness beats.

#[cfg(target_arch = "riscv32")]
include!("entry.rs");
#[cfg(not(target_arch = "riscv32"))]
fn main() {}
