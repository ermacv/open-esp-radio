//! Blobray analyses over `oer-tools-analysis-riscv-analysis`: execution-coverage closures,
//! register-access navigation. No filesystem,
//! project or ISA implementation.
#![expect(
    clippy::cast_possible_truncation,
    reason = "host tool: the rule covers the code an image reaches"
)]
#![expect(
    clippy::cast_possible_wrap,
    reason = "host tool: the rule covers the code an image reaches"
)]
#![expect(
    clippy::cast_sign_loss,
    reason = "host tool: the rule covers the code an image reaches"
)]
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
    reason = "host tool: the ban covers the code an image reaches"
)]
pub mod closure;
pub mod navigation;
pub mod registers;
