//! The register checks (`cargo registers check`):
//!
//! - [`inventory`]: vendor uses of absent MMIO bits fail; unknown bits and
//!   corpus limits are reported separately (`cargo registers check inventory`).

pub mod inventory;
