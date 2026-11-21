#![cfg_attr(target_arch = "riscv32", no_std, no_main)]
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

//! The HIL image of the radio-free scenarios: the `oer` AES
//! service with the library's defaults, the HIL base module and the AES
//! service over the USB Serial/JTAG console, without Wi-Fi.

#[cfg(target_arch = "riscv32")]
include!("entry.rs");
#[cfg(not(target_arch = "riscv32"))]
fn main() {}

#[cfg(any(target_arch = "riscv32", test))]
mod services;
