//! Blobray analyses over `oer-riscv-analysis`: final-image target audits,
//! execution-coverage closures, register-access navigation. No filesystem,
//! project or ISA implementation.
pub mod audit;
pub mod closure;
pub mod navigation;
pub mod registers;
