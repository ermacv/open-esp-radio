//! The register checks (`cargo registers check`): rules about handwritten
//! PAC code and about the published registers, owned by the register
//! publication.
//!
//! - [`pac_transactions`]: a handwritten PAC operation is one transaction;
//! - [`shared_words`]: no MMIO word has two uncoordinated writers (radio PAC
//!   and esp-hal).

pub mod pac_transactions;
pub mod shared_words;
