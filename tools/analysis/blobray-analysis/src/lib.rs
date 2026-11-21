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
pub mod closure;
pub mod navigation;
pub mod registers;
