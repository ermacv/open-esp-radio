//! Concrete RV32 execution over `oer-tool-analysis-riscv-lift`'s decoding and lifting; no
//! I/O authority.
mod execution;
pub use execution::{RiscvExecutor, Rv32imacExecutor};
use oer_tool_analysis_blobray_domain::*;
use oer_tool_analysis_riscv_decode::{Extensions, Inst, Instruction};
use oer_tool_analysis_riscv_lift::{RiscvDecoder, decode_instruction};
use oer_tool_analysis_riscv_model::*;
