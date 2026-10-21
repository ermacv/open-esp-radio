//! The register checks (`cargo registers check`): rules about handwritten
//! PAC code and about the published registers, owned by the register
//! publication.
//!
//! - [`pac_transactions`]: a handwritten PAC operation is one transaction;
//! - [`shared_words`]: no MMIO word has two uncoordinated writers (radio PAC
//!   and esp-hal);
//! - [`inventory`]: vendor uses of absent MMIO bits fail; unknown bits and
//!   corpus limits are reported separately (`cargo registers check inventory`).

pub mod inventory;
pub mod pac_transactions;
pub mod shared_words;
