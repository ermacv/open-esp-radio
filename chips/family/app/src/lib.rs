//! What the family's application entry (`oer`) shares across its chips:
//! the boot's identity and the lowest permitted channel ([`boot`]), the
//! typed channel between an image's task groups ([`channel`]), the
//! network core's switchable
//! diagnostics ([`diagnostics`]). A chip's application
//! crate (`chips/<chip>/app`) builds on them; `oer` re-exports what an image
//! uses.
#![expect(
    clippy::cast_possible_truncation,
    reason = "the pump, the entry and the images"
)]
#![cfg_attr(not(test), no_std)]
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

#[cfg_attr(not(target_arch = "riscv32"), allow(dead_code))]
pub mod boot;
#[cfg_attr(not(target_arch = "riscv32"), allow(dead_code))]
pub mod channel;
pub mod diagnostics;
