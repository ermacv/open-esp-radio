#![cfg_attr(target_arch = "riscv32", no_std, no_main)]
#![forbid(unsafe_code)]
#![cfg_attr(target_arch = "riscv32", feature(type_alias_impl_trait))]

//! Takes the reset root and requests and releases each platform clock once
//! through the chip's platform clock owner. The host renders its typed
//! verdict as `OER-RESULT: pass radio-root`. It powers no RF, PHY or MAC.
//! Its one executor feeds the RTC watchdog the root armed.

#[cfg(target_arch = "riscv32")]
include!("entry.rs");
#[cfg(not(target_arch = "riscv32"))]
fn main() {}
