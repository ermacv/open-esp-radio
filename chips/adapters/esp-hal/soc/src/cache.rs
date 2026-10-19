//! Typed cache diagnostics and explicit PSRAM writeback operations.

#[cfg(feature = "axi-gdma-mem2mem")]
pub(super) mod maintenance;
pub(super) mod performance;
