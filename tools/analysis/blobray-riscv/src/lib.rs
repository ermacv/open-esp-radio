//! Concrete RV32 execution over `oer-tools-analysis-riscv-lift`'s decoding and lifting; no
//! I/O authority.
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
mod execution;
pub use execution::{RiscvExecutor, Rv32imacExecutor};
use oer_tools_analysis_blobray_domain::*;
use oer_tools_analysis_riscv_decode::{Extensions, Inst, Instruction};
use oer_tools_analysis_riscv_lift::{RiscvDecoder, decode_instruction};
use oer_tools_analysis_riscv_model::*;
