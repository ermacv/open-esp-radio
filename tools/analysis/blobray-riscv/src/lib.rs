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
mod execution;
pub use execution::{RiscvExecutor, Rv32imacExecutor};
use oer_tools_analysis_blobray_domain::*;
use oer_tools_analysis_riscv_decode::{Extensions, Inst, Instruction};
use oer_tools_analysis_riscv_lift::{RiscvDecoder, decode_instruction};
use oer_tools_analysis_riscv_model::*;
