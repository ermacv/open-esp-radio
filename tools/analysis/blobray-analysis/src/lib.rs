//! Blobray analyses over `oer-analysis-riscv-analysis`: execution-coverage closures,
//! register-access navigation. No filesystem,
//! project or ISA implementation.

#![forbid(unsafe_code)]
pub mod closure;
pub mod navigation;
pub mod registers;
