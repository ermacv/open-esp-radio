#![cfg_attr(
    any(test, target_arch = "riscv32"),
    expect(
        clippy::cast_possible_truncation,
        reason = "R2 cleans a probe with the area it reads (tasks/312)"
    )
)]
#![cfg_attr(
    test,
    expect(
        clippy::cast_sign_loss,
        reason = "test code: a cast or a dropped result in a test loses no product's value"
    )
)]
#![cfg_attr(target_arch = "riscv32", no_std, no_main)]
#![forbid(unsafe_code)]
#![cfg_attr(target_arch = "riscv32", feature(type_alias_impl_trait))]

//! The time the protocol layer's AES operations take on the S31: an AES-128
//! block, an RFC 3394 key unwrap, a CCMP-128 seal and a BIP AES-CMAC, each
//! in software and on the AES accelerator waiting on its state flag, on its
//! interrupt, and as the Wi-Fi station's engine waits (timer-woken samples).
//! See the README.

#[cfg(any(target_arch = "riscv32", test))]
mod ops;

#[cfg(target_arch = "riscv32")]
include!("entry.rs");
#[cfg(not(target_arch = "riscv32"))]
fn main() {}
