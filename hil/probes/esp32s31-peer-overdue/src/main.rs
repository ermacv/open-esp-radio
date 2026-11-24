#![cfg_attr(
    target_arch = "riscv32",
    expect(
        clippy::cast_possible_truncation,
        reason = "the probe's first change: deleted, or moved onto HIL (plan, section 6)"
    )
)]
#![cfg_attr(target_arch = "riscv32", no_std, no_main)]
#![forbid(unsafe_code)]
#![cfg_attr(target_arch = "riscv32", feature(type_alias_impl_trait))]
#![cfg_attr(
    target_arch = "riscv32",
    expect(
        clippy::arithmetic_side_effects,
        reason = "the probe's first change: deleted, or moved onto HIL (plan, section 6)"
    )
)]

//! The negative check of watched peer requests on hardware. The boot hart's
//! liveness supervisor watches one channel to hart 1. Once the stand arms
//! the probe, its client posts one request and never takes the answer,
//! while hart 1 serves the request and keeps echoing beats: the supervisor
//! records `PeerUnanswered` (channel 0, the request's deadline, mark 2) and
//! resets. The next boot compares the retained record with what the probe
//! expected and writes the verdict. See the README.

#[cfg(target_arch = "riscv32")]
include!("entry.rs");
#[cfg(not(target_arch = "riscv32"))]
fn main() {}
