#![cfg_attr(target_arch = "riscv32", no_std, no_main)]
#![forbid(unsafe_code)]
#![cfg_attr(target_arch = "riscv32", feature(type_alias_impl_trait))]

//! W0: the radio system's first run on the chip. The actor registers the PHY
//! domain for Wi-Fi's `join`, tracks it, prepares the MAC for `track` and
//! closes RF after `leave`; the image logs the calibration path, the
//! registration and tracking durations, the temperature and the longest
//! not-ready runs of the bounded readiness waits, then prints
//! `OER-RESULT: pass w0`. MAC gates stay closed; no BASE or runtime start is
//! published without a ring. Nothing is transmitted.
//!
//! eFuse is only read: the base MAC and its extension word for the
//! calibration identity, and the interface addresses derived from
//! the base MAC.

#[cfg(target_arch = "riscv32")]
include!("entry.rs");
#[cfg(not(target_arch = "riscv32"))]
fn main() {}
